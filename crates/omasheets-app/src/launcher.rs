//! Supervise the native service until the last native grid window closes.
use crate::Result;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub fn executable(variable: &str, name: &str) -> Result<PathBuf> {
    let executable = |p: &Path| p.is_file() && p.metadata().is_ok_and(|m| m.mode() & 0o111 != 0);
    if let Some(value) = std::env::var_os(variable) {
        let p = PathBuf::from(value);
        return if executable(&p) {
            std::fs::canonicalize(p).map_err(|e| e.to_string())
        } else {
            Err(format!("{variable} is not executable"))
        };
    }
    if let Ok(own) = std::env::current_exe() {
        let p = own.with_file_name(name);
        if executable(&p) {
            return Ok(p);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let p = directory.join(name);
            if executable(&p) {
                return std::fs::canonicalize(p).map_err(|e| e.to_string());
            }
        }
    }
    Err(format!("{name} is not installed"))
}
fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let m = file.metadata().map_err(|e| e.to_string())?;
    if !m.is_file() || m.uid() != unsafe { libc::getuid() } || m.mode() & 0o777 != 0o600 {
        return Err("Invalid service lock".into());
    }
    Ok(file)
}
fn flock(file: &File, op: i32) -> Result<()> {
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), op) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e.to_string());
        }
    }
}
fn ready(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}
fn stop(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        for _ in 0..100 {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}
fn ensure(runtime: &Path, directory: &Path) -> Result<Option<Child>> {
    let mutex = lock(&directory.join("grid-service.lock"))?;
    flock(&mutex, libc::LOCK_EX)?;
    let socket = directory.join("native.sock");
    if ready(&socket) {
        return Ok(None);
    }
    let mut command = Command::new(executable("OMASHEETS_NATIVE_SERVICE", "omasheets-service")?);
    command
        .args(["serve", "--runtime-dir"])
        .arg(runtime)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    for _ in 0..100 {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if ready(&socket) {
            return Ok(Some(child));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop(&mut child);
    Err("Native service did not become ready".into())
}
pub fn host(source: Option<&Path>) -> Result<i32> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or("XDG_RUNTIME_DIR must be absolute")?;
    let directory = runtime.join("omasheets");
    if !directory.exists() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .or_else(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .map_err(|e| e.to_string())?;
    }
    crate::agent::directory()?;
    let grid = executable("OMASHEETS_GRID", "omasheets-grid")?;
    let lease = lock(&directory.join("grid-clients.lock"))?;
    flock(&lease, libc::LOCK_SH)?;
    let mut service = ensure(&runtime, &directory)?;
    let result = {
        let mut command = Command::new(grid);
        command.env_remove("OMASHEETS_DOCUMENT");
        if let Some(path) = source {
            command.arg(path).env("OMASHEETS_DOCUMENT", path);
        }
        command
            .status()
            .map(|s| s.code().unwrap_or(1))
            .map_err(|e| e.to_string())
    };
    flock(&lease, libc::LOCK_UN)?;
    if let Some(child) = service.as_mut() {
        flock(&lease, libc::LOCK_EX)?;
        // Take the start lock as well: cleanup must not race a new service.
        let mutex = lock(&directory.join("grid-service.lock"))?;
        flock(&mutex, libc::LOCK_EX)?;
        stop(child);
        if !ready(&directory.join("native.sock")) {
            for name in ["native.sock", "native.token"] {
                let _ = std::fs::remove_file(directory.join(name));
            }
        }
    }
    result
}
pub fn launch(source: Option<&Path>) -> Result<u32> {
    let source = source
        .map(|p| -> Result<PathBuf> {
            let p = p.canonicalize().map_err(|e| e.to_string())?;
            if !p.is_file()
                || p.extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    != Some("omasheets".into())
            {
                return Err("Native launcher requires a regular .omasheets document".into());
            }
            Ok(p)
        })
        .transpose()?;
    executable("OMASHEETS_GRID", "omasheets-grid")?;
    executable("OMASHEETS_NATIVE_SERVICE", "omasheets-service")?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg("--native-host");
    if let Some(p) = source {
        command.arg(p);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Only async-signal-safe libc before exec. All allocation happened above.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command.spawn().map(|c| c.id()).map_err(|e| e.to_string())
}
