//! Private state, stable workbook identities and durable no-clobber copies.
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_ns: i128,
    pub sha256: String,
}
fn key(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
fn regular(path: &Path) -> Result<(File, fs::Metadata)> {
    let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() {
        return Err("workbook must be a regular file, not a link or device".into());
    }
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| e.to_string())?;
    let opened = f.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file() || key(&before) != key(&opened) {
        return Err("workbook changed while it was opened".into());
    }
    Ok((f, opened))
}
pub fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn valid_id(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn private_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path).map_err(|e| e.to_string())?;
    }
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } {
        return Err("private directory is not owned by this user".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}
pub fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
fn transfer(source: &Path, mut output: Option<&mut File>) -> Result<FileIdentity> {
    let (mut input, before) = regular(source)?;
    let expected = key(&before);
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut chunk = vec![0u8; 1024 * 1024];
    loop {
        let n = input.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if key(&input.metadata().map_err(|e| e.to_string())?) != expected {
            return Err("workbook changed while preparing the Calc job".into());
        }
        if let Some(out) = output.as_deref_mut() {
            out.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
        }
        hash.update(&chunk[..n]);
        count += n as u64;
    }
    if count != before.len()
        || key(&input.metadata().map_err(|e| e.to_string())?) != expected
        || key(&fs::symlink_metadata(source)
            .map_err(|_| "workbook changed while preparing the Calc job")?)
            != expected
    {
        return Err("workbook changed while preparing the Calc job".into());
    }
    if let Some(out) = output {
        out.sync_all().map_err(|e| e.to_string())?;
    }
    Ok(FileIdentity {
        device: before.dev(),
        inode: before.ino(),
        size: count,
        mtime_ns: before.mtime() as i128 * 1_000_000_000 + before.mtime_nsec() as i128,
        sha256: format!("{:x}", hash.finalize()),
    })
}
pub fn identify(path: &Path) -> Result<FileIdentity> {
    transfer(path, None)
}
pub fn copy_no_clobber(source: &Path, destination: &Path) -> Result<FileIdentity> {
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(destination)
        .map_err(|e| e.to_string())?;
    let created = out.metadata().map_err(|e| e.to_string())?;
    let result = transfer(source, Some(&mut out));
    if result.is_err()
        && fs::symlink_metadata(destination)
            .is_ok_and(|m| m.dev() == created.dev() && m.ino() == created.ino())
    {
        let _ = fs::remove_file(destination);
    }
    result
}
pub fn atomic_bytes(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or("destination needs a parent directory")?;
    let tmp = parent.join(format!(".omasheets-{}", random_id()?));
    let result = (|| {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        out.write_all(bytes)
            .and_then(|_| out.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        sync_dir(parent)
    })();
    let _ = fs::remove_file(tmp);
    result
}
pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().ok_or("state file needs a parent")?;
    private_dir(parent)?;
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    atomic_bytes(path, &bytes, 0o600)
}
pub fn read_json(path: &Path, limit: u64) -> Result<Value> {
    let (f, m) = regular(path)?;
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 || m.len() > limit {
        return Err("state file must be private, owned and bounded".into());
    }
    let mut bytes = Vec::new();
    f.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("state file exceeds its size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
pub struct Lock(File);
impl Lock {
    pub fn acquire(path: &Path) -> Result<Self> {
        private_dir(path.parent().ok_or("lock needs a parent")?)?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .map_err(|e| e.to_string())?;
        let m = f.metadata().map_err(|e| e.to_string())?;
        if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err("lock file is not private and owned".into());
        }
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self(f))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
#[derive(Clone)]
pub struct AppPaths {
    pub state: PathBuf,
    pub cache: PathBuf,
    pub runtime: PathBuf,
}
impl AppPaths {
    pub fn discover() -> Result<Self> {
        let home = std::env::var_os("HOME").ok_or("HOME is unavailable")?;
        let home = PathBuf::from(home);
        let p = Self {
            state: PathBuf::from(
                std::env::var_os("XDG_STATE_HOME")
                    .unwrap_or_else(|| home.join(".local/state").into_os_string()),
            )
            .join("omasheets"),
            cache: PathBuf::from(
                std::env::var_os("XDG_CACHE_HOME")
                    .unwrap_or_else(|| home.join(".cache").into_os_string()),
            )
            .join("omasheets"),
            runtime: PathBuf::from(
                std::env::var_os("XDG_RUNTIME_DIR")
                    .unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::geteuid() }).into()),
            )
            .join("omasheets"),
        };
        p.ensure()?;
        Ok(p)
    }
    pub fn ensure(&self) -> Result<()> {
        for p in [&self.state, &self.cache, &self.runtime] {
            private_dir(p)?;
        }
        Ok(())
    }
}
