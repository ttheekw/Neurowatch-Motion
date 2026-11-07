//! A console-free sign-in launcher; the daemon keeps its ordinary CLI.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let executable = std::env::current_exe()?.with_file_name("nw-motion.exe");
    Command::new(executable)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This launcher is only used by the Windows installer.");
    std::process::exit(1);
}
