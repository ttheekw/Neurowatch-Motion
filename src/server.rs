//! The daemon's side of `nw-analytics-protocol`: listen, and serve each
//! NeuroWatch node that connects (a session) with one worker per bound camera.
use crate::access::{Access, Admission, Denied};
use crate::manifest::manifest;
use crate::settings;
use crate::worker::{Input, Options, Sender, Worker};
use nw_analytics_protocol::{
    address::Address,
    error_code,
    frame::{read_frame, write_frame, Frame},
    messages::{
        Accepted, Attach, CameraHint, ConfigError, Configure, Detach, Gap, Hello, Refusal,
        StreamFormat, ValidateConfig, Validation,
    },
    method,
    rpc::{params, Message, RpcError},
    PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Cameras one session may bind (software decoding of sub streams).
pub const MAX_BINDINGS: usize = 1024;

/// A connection's write half, shared by the session and its workers.
struct Out<W: Write + Send> {
    writer: Mutex<BufWriter<W>>,
}

impl<W: Write + Send + 'static> Sender for Out<W> {
    fn send(&self, message: Message) {
        if let Ok(mut writer) = self.writer.lock() {
            let written = write_frame(&mut *writer, &Frame::Message(message.to_value()))
                .and_then(|_| writer.flush());
            if let Err(error) = written {
                tracing::debug!(%error, "Could not write to NeuroWatch");
            }
        }
    }
}

impl Capabilities {
    fn descriptor(
        &self,
        offer: &nw_analytics_protocol::decoded::LocalityChallenge,
    ) -> Option<std::fs::File> {
        #[cfg(unix)]
        {
            self.inbox.as_ref()?.take(&offer.ring_id)
        }
        #[cfg(windows)]
        {
            self.pipe.as_ref()?.descriptor(offer.handle?).ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = offer;
            None
        }
    }
}

/// Serve one NeuroWatch node until it disconnects.
pub fn serve<R, W>(reader: R, writer: W, options: Options)
where
    R: Read,
    W: Write + Send + 'static,
{
    serve_inner(reader, writer, options, Capabilities::default());
}

type Closer = Arc<dyn Fn() + Send + Sync>;

fn closer(close: impl Fn() + Send + Sync + 'static) -> Option<Closer> {
    Some(Arc::new(close))
}

#[derive(Default)]
struct Capabilities {
    identity: Option<Arc<nw_analytics_sdk::identity::SigningKey>>,
    channel_binding: String,
    /// The daemon's access codes (shared by every connection).
    access: Arc<Access>,
    /// Closes the connection from another thread (unblocks its reader).
    close: Option<Closer>,
    #[cfg(unix)]
    inbox: Option<nw_analytics_sdk::local::DescriptorInbox>,
    #[cfg(windows)]
    pipe: Option<nw_analytics_sdk::pipe::Pipe>,
}

#[cfg(unix)]
pub fn serve_local(socket: std::os::unix::net::UnixStream, options: Options) -> io::Result<()> {
    serve_local_with_access(socket, options, Arc::default())
}

/// [`serve_local`], admitting the node only with one of `access`'s codes.
#[cfg(unix)]
pub fn serve_local_with_access(
    socket: std::os::unix::net::UnixStream,
    options: Options,
    access: Arc<Access>,
) -> io::Result<()> {
    let (reader, inbox) = nw_analytics_sdk::local::LocalReader::new(socket.try_clone()?);
    let close = shutdown_on_close(&socket);
    serve_inner(
        reader,
        socket,
        options,
        Capabilities {
            inbox: Some(inbox),
            close,
            access,
            ..Capabilities::default()
        },
    );
    Ok(())
}

fn serve_inner<R: Read, W: Write + Send + 'static>(
    reader: R,
    writer: W,
    options: Options,
    capabilities: Capabilities,
) {
    let identity = match capabilities.identity.clone() {
        Some(key) => key,
        None => match nw_analytics_sdk::identity::random_key() {
            Ok(key) => Arc::new(key),
            Err(error) => {
                tracing::error!(%error, "Could not generate session identity");
                return;
            }
        },
    };
    let out: Arc<dyn Sender> = Arc::new(Out {
        writer: Mutex::new(BufWriter::new(writer)),
    });
    let watch = Arc::new(Watch {
        start: Instant::now(),
        heard_ms: AtomicU64::new(0),
        ended: AtomicBool::new(false),
        admission: Mutex::new(None),
        authorized: AtomicBool::new(false),
    });
    if let Err(error) = watch.spawn(options.silence_limit, capabilities.close.clone()) {
        tracing::error!(%error, "Could not watch the session");
        return;
    }
    let mut session = Session {
        out,
        events: Arc::new(AtomicU64::new(0)),
        workers: HashMap::new(),
        greeted: false,
        capabilities,
        local: false,
        identity,
        challenge: String::new(),
        signed: None,
        authorized: false,
        authorized_node: None,
        watch: Arc::clone(&watch),
        web: nw_analytics_sdk::web::Service::default(),
    };
    let mut reader = BufReader::new(reader);
    loop {
        let frame = read_frame(&mut reader);
        if watch.ended.load(Ordering::Acquire) {
            break;
        }
        watch.heard();
        match frame {
            Ok(Frame::Message(value)) => match Message::from_value(value) {
                Ok(message) => session.message(message),
                Err(error) => {
                    tracing::warn!(%error, "NeuroWatch sent an invalid message; closing");
                    break;
                }
            },
            Ok(Frame::Media(unit)) => {
                if let Some(worker) = session.workers.get(&unit.binding) {
                    worker.offer(Input::Unit(unit));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => {
                tracing::warn!(%error, "Connection to NeuroWatch failed");
                break;
            }
        }
    }
    watch.end();
    watch.signed_in(); // no longer waiting, whatever happened
    let bound = session.workers.len();
    session.workers.clear();
    tracing::info!(bound, "NeuroWatch disconnected");
}

/// A session's watchdog: NeuroWatch pings every 5 s, so a connection silent
/// for [`Options::silence_limit`] is gone (a lost network can leave a read
/// waiting for minutes). It also ends a connection whose access code was
/// revoked. Either way the place under the code is given back at once.
struct Watch {
    start: Instant,
    heard_ms: AtomicU64,
    ended: AtomicBool,
    admission: Mutex<Option<Admission>>,
    /// Signed in (`authenticate` accepted).
    authorized: AtomicBool,
}

/// Sessions served at once, sessions not signed in yet, and how long one may
/// take to sign in. Without them anyone who could reach the address kept as
/// many sessions as they liked by pinging, until file handles or threads ran
/// out and an accept error ended the daemon (vulnerability checklist R12).
const MAX_SESSIONS: usize = 256;
const MAX_UNAUTHENTICATED: usize = 32;
const AUTHENTICATE_WITHIN: Duration = Duration::from_secs(15);
static SESSIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static UNAUTHENTICATED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn release(counter: &std::sync::atomic::AtomicUsize) {
    let mut now = counter.load(Ordering::Acquire);
    while now > 0 {
        match counter.compare_exchange(now, now - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return,
            Err(seen) => now = seen,
        }
    }
}

impl Watch {
    fn heard(&self) {
        self.heard_ms
            .store(self.start.elapsed().as_millis() as u64, Ordering::Release);
    }

    /// Signed in: no longer counted as waiting to.
    fn signed_in(&self) {
        if !self.authorized.swap(true, Ordering::AcqRel) {
            release(&UNAUTHENTICATED);
        }
    }

    fn end(&self) {
        self.ended.store(true, Ordering::Release);
        if let Ok(mut admission) = self.admission.lock() {
            admission.take();
        }
    }

    fn spawn(self: &Arc<Self>, silence: Duration, close: Option<Closer>) -> io::Result<()> {
        let watch = Arc::clone(self);
        let tick = (silence / 4).clamp(Duration::from_millis(10), Duration::from_secs(1));
        std::thread::Builder::new()
            .name("session-watch".into())
            .spawn(move || loop {
                std::thread::sleep(tick);
                if watch.ended.load(Ordering::Acquire) {
                    return;
                }
                let heard = Duration::from_millis(watch.heard_ms.load(Ordering::Acquire));
                let silent = watch.start.elapsed().saturating_sub(heard);
                let revoked = watch
                    .admission
                    .lock()
                    .is_ok_and(|a| a.as_ref().is_some_and(Admission::revoked));
                let late = !watch.authorized.load(Ordering::Acquire)
                    && watch.start.elapsed() >= AUTHENTICATE_WITHIN;
                if silent < silence && !revoked && !late {
                    continue;
                }
                if revoked {
                    tracing::info!("Access code revoked; closing the connection");
                } else if late {
                    tracing::info!("The peer did not sign in; closing the connection");
                } else {
                    tracing::info!(
                        silent_s = silent.as_secs(),
                        "NeuroWatch went silent; closing the connection"
                    );
                }
                watch.end();
                if let Some(close) = &close {
                    close();
                }
                return;
            })
            .map(|_| ())
    }
}

struct Session {
    out: Arc<dyn Sender>,
    /// Event numbers, unique within the session.
    events: Arc<AtomicU64>,
    workers: HashMap<u32, Worker>,
    greeted: bool,
    capabilities: Capabilities,
    local: bool,
    identity: Arc<nw_analytics_sdk::identity::SigningKey>,
    challenge: String,
    signed: Option<nw_analytics_protocol::auth::PluginIdentity>,
    authorized: bool,
    authorized_node: Option<String>,
    /// Liveness and this node's place under its access code.
    watch: Arc<Watch>,
    web: nw_analytics_sdk::web::Service,
}

impl Session {
    fn message(&mut self, message: Message) {
        match message {
            Message::Request { id, method, params } => {
                let result = self.request(&method, params);
                self.out.send(Message::reply(id, result));
            }
            Message::Notification { method, params } => self.notification(&method, params),
            Message::Response { .. } => {}
        }
    }

    fn request(&mut self, name: &str, values: Value) -> Result<Value, RpcError> {
        if name != method::HELLO && !self.greeted {
            return Err(RpcError::new(error_code::INVALID_PARAMS, "hello first"));
        }
        if ![method::HELLO, method::AUTHENTICATE, method::PING].contains(&name) && !self.authorized
        {
            return Err(RpcError::new(
                error_code::REJECTED,
                "Authenticate this node first",
            ));
        }
        match name {
            method::AUTHENTICATE => {
                let auth = params(values)?;
                let identity = self
                    .signed
                    .as_ref()
                    .ok_or_else(|| RpcError::new(error_code::REJECTED, "hello first"))?;
                if !nw_analytics_sdk::identity::verify_authenticate(
                    &auth,
                    &self.challenge,
                    identity,
                ) {
                    return Err(RpcError::new(
                        error_code::REJECTED,
                        "Invalid node signature",
                    ));
                }
                let admitted = self.watch.admission.lock().is_ok_and(|a| a.is_some());
                if !admitted {
                    let admission = self.admit(auth.credential.as_deref())?;
                    if let Some(name) = admission.code_name() {
                        tracing::info!(code = %name, "NeuroWatch admitted");
                    }
                    if let Ok(mut held) = self.watch.admission.lock() {
                        *held = Some(admission);
                    }
                }
                self.authorized = true;
                self.watch.signed_in();
                self.authorized_node = Some(auth.public_key);
                Ok(json!({"authorized":true}))
            }
            "web.request" | "web.read" => {
                let node = self
                    .authorized_node
                    .as_deref()
                    .ok_or_else(|| RpcError::new(error_code::REJECTED, "Authenticate first"))?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |time| time.as_secs() as i64);
                self.web.dispatch(
                    name,
                    values,
                    node,
                    &nw_analytics_sdk::identity::public_key(&self.identity),
                    now,
                    |request| {
                        if request.method != "GET" && request.method != "HEAD" {
                            return Ok(nw_analytics_sdk::web::HttpResponse {
                                status: 405,
                                content_type: "text/plain".into(),
                                body: b"Use the recorder's binding settings".to_vec(),
                            });
                        }
                        if request.path == "/" {
                            return Ok(nw_analytics_sdk::web::HttpResponse::html(include_str!(
                                "../web/index.html"
                            )));
                        }
                        if request.path == "/api/cameras" {
                            return Ok(nw_analytics_sdk::web::HttpResponse::json(
                                &json!({"cameras":request.user.cameras}),
                            ));
                        }
                        Ok(nw_analytics_sdk::web::HttpResponse {
                            status: 404,
                            content_type: "text/plain".into(),
                            body: b"Not found".to_vec(),
                        })
                    },
                )
            }
            method::HELLO => {
                if self.greeted {
                    return Err(RpcError::new(
                        error_code::INVALID_PARAMS,
                        "hello already received",
                    ));
                }
                let hello: Hello = params(values)?;
                if hello.protocol != PROTOCOL_VERSION {
                    return Err(RpcError::new(
                        error_code::UNSUPPORTED_PROTOCOL,
                        format!("This plugin speaks protocol {PROTOCOL_VERSION}"),
                    ));
                }
                tracing::info!(node = %hello.node.name, version = %hello.node.version, "NeuroWatch connected");
                if nw_analytics_sdk::identity::unhex::<32>(&hello.challenge).is_none() {
                    return Err(RpcError::new(
                        error_code::INVALID_PARAMS,
                        "A fresh 32-byte challenge is required",
                    ));
                }
                self.challenge = hello.challenge.clone();
                self.greeted = true;
                let mut manifest = manifest();
                if let Some(offer) = hello.locality.as_ref() {
                    manifest.locality_proof = self
                        .capabilities
                        .descriptor(offer)
                        .and_then(|file| nw_analytics_sdk::locality::answer(file, offer).ok());
                }
                self.local = manifest.locality_proof.is_some();
                if !self.local {
                    manifest.media.decoded = None;
                }
                nw_analytics_sdk::identity::sign_manifest(
                    &mut manifest,
                    &self.identity,
                    &self.challenge,
                    &self.capabilities.channel_binding,
                )
                .map_err(|_| RpcError::new(error_code::INTERNAL, "Could not sign the manifest"))?;
                self.signed = manifest.identity.clone();
                Ok(to_value(manifest))
            }
            method::PING => Ok(Value::Null),
            method::ATTACH => self.attach(params(values)?),
            method::CONFIGURE => {
                let configure: Configure = params(values)?;
                let worker = self.workers.get(&configure.binding).ok_or_else(unknown)?;
                let settings = settings::validate(&configure.config).map_err(invalid_config)?;
                let accepted = accepted(&settings);
                worker.offer(Input::Settings(settings));
                Ok(to_value(accepted))
            }
            method::VALIDATE_CONFIG => {
                let validate: ValidateConfig = params(values)?;
                Ok(to_value(match settings::validate(&validate.config) {
                    Ok(_) => Validation {
                        ok: true,
                        errors: vec![],
                    },
                    Err(message) => Validation {
                        ok: false,
                        errors: vec![ConfigError {
                            path: String::new(),
                            message,
                        }],
                    },
                }))
            }
            method::DETACH => {
                let detach: Detach = params(values)?;
                // Dropping the worker ends its open event.
                self.workers.remove(&detach.binding).ok_or_else(unknown)?;
                tracing::info!(binding = detach.binding, reason = %detach.reason, "Camera unbound");
                Ok(Value::Null)
            }
            _ => Err(RpcError::new(
                error_code::METHOD_NOT_FOUND,
                "Method not found",
            )),
        }
    }

    /// The access-code check (no list: every node), before anything is served.
    fn admit(&self, credential: Option<&str>) -> Result<Admission, RpcError> {
        self.capabilities.access.admit(credential).map_err(|denied| {
            tracing::info!(?denied, "NeuroWatch node refused");
            match denied {
                Denied::Code => refuse(
                    "ACCESS_CODE",
                    "This plugin needs a valid access code. Enter it under Plugins → Vendor Credential…".into(),
                ),
                Denied::Full { max_connections } => refuse(
                    "CONNECTION_LIMIT",
                    format!(
                        "This access code is already in use by {max_connections} NeuroWatch \
                         recorder(s), its limit"
                    ),
                ),
            }
        })
    }

    fn attach(&mut self, attach: Attach) -> Result<Value, RpcError> {
        if self.workers.contains_key(&attach.binding) {
            return Err(RpcError::new(
                error_code::INVALID_PARAMS,
                "binding already attached",
            ));
        }
        if self.workers.len() >= MAX_BINDINGS {
            return Err(refuse(
                "NO_CAPACITY",
                format!("This motion plugin analyses at most {MAX_BINDINGS} cameras"),
            ));
        }
        let settings = settings::validate(&attach.config).map_err(invalid_config)?;
        let accepted = accepted(&settings);
        let worker = Worker::start(
            attach.binding,
            attach.camera.id.clone(),
            settings,
            Arc::clone(&self.events),
            Arc::clone(&self.out),
        )
        .map_err(|error| refuse("HARDWARE", format!("Could not start analysis: {error}")))?;
        self.workers.insert(attach.binding, worker);
        tracing::info!(binding = attach.binding, camera = %attach.camera.id, name = %attach.camera.name, "Camera bound");
        Ok(to_value(accepted))
    }

    fn notification(&mut self, name: &str, values: Value) {
        if !self.authorized {
            return;
        }
        match name {
            method::DECODED_AVAILABLE if self.local => {
                if let Ok(offer) =
                    params::<nw_analytics_protocol::decoded::DecodedAvailable>(values)
                {
                    if let (Some(worker), Some(file)) = (
                        self.workers.get(&offer.binding),
                        self.capabilities.descriptor(&offer.ring),
                    ) {
                        if let Ok(ring) = nw_analytics_sdk::ring::RingReader::open(
                            file,
                            &offer.ring.ring_id,
                            offer.ring.bytes,
                        ) {
                            if ring.layout() == offer.layout {
                                worker.offer(Input::Decoded(ring));
                            }
                        }
                    }
                }
            }
            method::DECODED_WITHDRAWN => {
                if let Ok(offer) =
                    params::<nw_analytics_protocol::decoded::DecodedWithdrawn>(values)
                {
                    if let Some(worker) = self.workers.get(&offer.binding) {
                        worker.offer(Input::Withdrawn);
                    }
                }
            }
            method::STREAM_FORMAT => {
                if let Ok(format) = params::<StreamFormat>(values) {
                    if let Some(worker) = self.workers.get(&format.binding) {
                        worker.offer(Input::Format(format));
                    }
                }
            }
            method::GAP => {
                if let Ok(gap) = params::<Gap>(values) {
                    if let Some(worker) = self.workers.get(&gap.binding) {
                        worker.lost();
                    }
                }
            }
            method::CAMERA_HINT => {
                if let Ok(hint) = params::<CameraHint>(values) {
                    if let Some(worker) = self.workers.get(&hint.binding) {
                        worker.offer(Input::Moving(hint.moving));
                    }
                }
            }
            other => tracing::debug!(method = %other, "Notification ignored"),
        }
    }
}

/// Low CPU: NeuroWatch sends keyframes only, so delta units never even
/// cross the socket.
fn accepted(settings: &settings::MotionSettings) -> Accepted {
    Accepted {
        stream: None,
        keyframes_only: Some(settings.low_cpu),
    }
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn unknown() -> RpcError {
    RpcError::new(error_code::UNKNOWN_BINDING, "Unknown binding")
}

fn invalid_config(message: String) -> RpcError {
    RpcError::new(error_code::INVALID_CONFIG, message.clone())
        .with_data(json!({"errors": [{"path": "", "message": message}]}))
}

fn refuse(code: &str, message: String) -> RpcError {
    RpcError::new(error_code::REJECTED, message.clone()).with_data(to_value(Refusal {
        code: code.into(),
        message,
        retry_after_s: None,
        url: None,
    }))
}

/// Listen on `address` and serve every connection on its own thread.
pub fn listen(address: &Address, options: Options) -> anyhow::Result<()> {
    let key = nw_analytics_sdk::identity::random_key()?;
    listen_with_identity(address, options, Arc::new(key), Arc::default())
}

pub fn listen_with_identity(
    address: &Address,
    options: Options,
    identity: Arc<nw_analytics_sdk::identity::SigningKey>,
    access: Arc<Access>,
) -> anyhow::Result<()> {
    match address {
        Address::Tls { host, port } => {
            use std::net::ToSocketAddrs;
            let address = (host.as_str(), *port)
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| anyhow::anyhow!("Plugin hostname has no address"))?;
            let listener = nw_analytics_sdk::tls::Listener::bind(address, &identity)?;
            loop {
                let io = match listener.accept() {
                    Ok(io) => io,
                    Err(error) => {
                        accept_failed(&error);
                        continue;
                    }
                };
                let identity = Arc::clone(&identity);
                let access = Arc::clone(&access);
                spawn(move || {
                    serve_inner(
                        io.clone(),
                        io.clone(),
                        options,
                        Capabilities {
                            identity: Some(identity),
                            channel_binding: io.channel_binding().into(),
                            access,
                            close: {
                                let io = io.clone();
                                closer(move || io.shutdown())
                            },
                            ..Capabilities::default()
                        },
                    )
                })?;
            }
        }
        Address::Quic { host, port } => {
            use std::net::ToSocketAddrs;
            let address = (host.as_str(), *port)
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| anyhow::anyhow!("Plugin hostname has no address"))?;
            let listener = nw_analytics_sdk::quic::Listener::bind(address, &identity)?;
            loop {
                let io = match listener.accept() {
                    Ok(io) => io,
                    Err(error) => {
                        accept_failed(&error);
                        continue;
                    }
                };
                let identity = Arc::clone(&identity);
                let access = Arc::clone(&access);
                spawn(move || {
                    serve_inner(
                        io.clone(),
                        io.clone(),
                        options,
                        Capabilities {
                            identity: Some(identity),
                            channel_binding: io.channel_binding().into(),
                            access,
                            close: {
                                let io = io.clone();
                                closer(move || io.shutdown())
                            },
                            ..Capabilities::default()
                        },
                    )
                })?;
            }
        }
        #[cfg(unix)]
        Address::Unix(path) => {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            if let Some(dir) = path.parent() {
                // Every folder made here is private: the default socket sits
                // under NeuroWatch's own `neurowatch/`, which NeuroWatch
                // refuses to use unless only this user can open it.
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir)?;
                // Only this user may reach the socket (as NeuroWatch's own).
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
            if path.exists() {
                if std::os::unix::net::UnixStream::connect(path).is_ok() {
                    anyhow::bail!("another daemon is listening on {}", path.display());
                }
                std::fs::remove_file(path)?;
            }
            let listener = std::os::unix::net::UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            tracing::info!(%address, "Listening for NeuroWatch");
            for conn in listener.incoming() {
                let conn = match conn {
                    Ok(conn) => conn,
                    Err(error) => {
                        accept_failed(&error);
                        continue;
                    }
                };
                let identity = Arc::clone(&identity);
                let access = Arc::clone(&access);
                spawn(move || {
                    if let Ok(reader) = conn.try_clone() {
                        let (reader, inbox) = nw_analytics_sdk::local::LocalReader::new(reader);
                        let close = shutdown_on_close(&conn);
                        serve_inner(
                            reader,
                            conn,
                            options,
                            Capabilities {
                                inbox: Some(inbox),
                                identity: Some(identity),
                                access,
                                close,
                                ..Capabilities::default()
                            },
                        );
                    }
                })?;
            }
        }
        #[cfg(not(unix))]
        Address::Unix(_) => {
            anyhow::bail!("Unix sockets are not available here; use tcp:127.0.0.1:<port>")
        }
        #[cfg(windows)]
        Address::Pipe(name) => {
            let mut listener = nw_analytics_sdk::pipe::Listener::bind(name)?;
            tracing::info!(%address, "Listening for NeuroWatch");
            loop {
                let pipe = match listener.accept() {
                    Ok(pipe) => pipe,
                    Err(error) => {
                        accept_failed(&error);
                        continue;
                    }
                };
                let identity = Arc::clone(&identity);
                let access = Arc::clone(&access);
                spawn(move || {
                    serve_inner(
                        pipe.clone(),
                        pipe.clone(),
                        options,
                        Capabilities {
                            close: {
                                let pipe = pipe.clone();
                                closer(move || pipe.shutdown())
                            },
                            pipe: Some(pipe),
                            identity: Some(identity),
                            access,
                            ..Capabilities::default()
                        },
                    )
                })?;
            }
        }
        #[cfg(not(windows))]
        Address::Pipe(_) => anyhow::bail!("Windows named pipes are unavailable here"),
        Address::Tcp(socket) => {
            let listener = std::net::TcpListener::bind(socket)?;
            tracing::info!(%address, "Listening for NeuroWatch");
            for conn in listener.incoming() {
                let conn = match conn {
                    Ok(conn) => conn,
                    Err(error) => {
                        accept_failed(&error);
                        continue;
                    }
                };
                let _ = conn.set_nodelay(true);
                let Ok(reader) = conn.try_clone() else {
                    continue;
                };
                let close = shutdown_on_tcp_close(&conn);
                let identity = Arc::clone(&identity);
                let access = Arc::clone(&access);
                spawn(move || {
                    serve_inner(
                        reader,
                        conn,
                        options,
                        Capabilities {
                            identity: Some(identity),
                            access,
                            close,
                            ..Capabilities::default()
                        },
                    )
                })?;
            }
        }
    }
    Ok(())
}

/// Closes a socket from the watchdog: `shutdown` ends a blocked read.
#[cfg(unix)]
fn shutdown_on_close(socket: &std::os::unix::net::UnixStream) -> Option<Closer> {
    let socket = socket.try_clone().ok()?;
    closer(move || {
        let _ = socket.shutdown(std::net::Shutdown::Both);
    })
}

fn shutdown_on_tcp_close(socket: &std::net::TcpStream) -> Option<Closer> {
    let socket = socket.try_clone().ok()?;
    closer(move || {
        let _ = socket.shutdown(std::net::Shutdown::Both);
    })
}

/// Serve a connection on its own thread, unless the daemon already serves
/// as many as it may (the connection is then closed). Never an error: a
/// failed spawn drops this connection and the listener goes on.
fn spawn(serve: impl FnOnce() + Send + 'static) -> io::Result<()> {
    if SESSIONS.load(Ordering::Acquire) >= MAX_SESSIONS
        || UNAUTHENTICATED.load(Ordering::Acquire) >= MAX_UNAUTHENTICATED
    {
        tracing::warn!("Too many connections; refusing one");
        return Ok(());
    }
    SESSIONS.fetch_add(1, Ordering::AcqRel);
    UNAUTHENTICATED.fetch_add(1, Ordering::AcqRel);
    let started = std::thread::Builder::new()
        .name("session".into())
        .spawn(move || {
            serve();
            release(&SESSIONS);
        });
    if let Err(error) = started {
        tracing::warn!(%error, "Could not start a session; refusing the connection");
        release(&SESSIONS);
        release(&UNAUTHENTICATED);
    }
    Ok(())
}

/// An accept that failed (out of file handles, a peer gone): wait a little
/// and go on. Returning the error ended the daemon for every recorder.
fn accept_failed(error: &dyn std::fmt::Display) {
    tracing::warn!(%error, "Accepting a connection failed; going on");
    std::thread::sleep(Duration::from_millis(200));
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    /// Vulnerability checklist R12: a peer that never signs in is closed.
    #[test]
    fn a_session_that_does_not_sign_in_is_closed() {
        let closed = Arc::new(AtomicBool::new(false));
        let watch = Arc::new(Watch {
            start: Instant::now()
                .checked_sub(AUTHENTICATE_WITHIN + Duration::from_secs(1))
                .expect("a moment ago"),
            heard_ms: AtomicU64::new(u64::MAX / 2),
            ended: AtomicBool::new(false),
            admission: Mutex::new(None),
            authorized: AtomicBool::new(false),
        });
        let flag = Arc::clone(&closed);
        watch
            .spawn(
                Duration::from_secs(60),
                closer(move || flag.store(true, Ordering::Release)),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !closed.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(closed.load(Ordering::Acquire), "closed for not signing in");
    }

    /// Vulnerability checklist R12: past the limits a connection is refused,
    /// not served, and the listener is not ended by it.
    #[test]
    fn connections_past_the_limit_are_refused() {
        let before = UNAUTHENTICATED.swap(MAX_UNAUTHENTICATED, Ordering::AcqRel);
        let served = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&served);
        let outcome = spawn(move || flag.store(true, Ordering::Release));
        UNAUTHENTICATED.store(before, Ordering::Release);
        assert!(outcome.is_ok(), "never an error for the listener");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!served.load(Ordering::Acquire), "refused");
    }
}
