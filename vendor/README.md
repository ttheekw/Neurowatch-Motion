# Analytics library snapshots

`nw-analytics-protocol` and `nw-analytics-sdk` contain the library sources needed
to build this daemon without another checkout. They originate from the local
NeuroWatch working tree based on commit
`8fcdcc7bddb36b5bf9a333cd2ffb9c95308d994d`; the analytics crates were unpublished
additions at the time of this snapshot. They use the repository's Apache-2.0 license.

Only each crate's `Cargo.toml` and library `src/*.rs` files are included. The
unrelated Python bridge executable and its Python integration fixtures are omitted.
The manifests resolve NeuroWatch workspace inheritance to explicit package and
dependency values; the library sources are unchanged. `sources.sha256` records
the included Rust sources so updates can be compared with their upstream copies.

To update, copy the library sources from a compatible NeuroWatch checkout,
review dependency changes, regenerate `sources.sha256`, and run
`cargo test --workspace --locked`. Keep these snapshots compatible with the
recorder's analytics protocol. `neurowatch.ref` separately pins the upstream
GStreamer packaging scripts used for Linux releases.
