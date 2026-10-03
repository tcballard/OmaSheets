//! One bounded Calc job per networkless Bubblewrap sandbox.
use crate::{
    Result,
    files::{self, AppPaths},
    launcher,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
pub struct Engine {
    pub paths: AppPaths,
}
struct Job(PathBuf);
impl Drop for Job {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Engine {
    pub fn execute(
        &self,
        action: &str,
        source: &Path,
        arguments: Value,
        artifacts: &[(&str, &Path)],
    ) -> Result<Value> {
        let root = self.paths.cache.join("jobs");
        files::private_dir(&root)?;
        let job = Job(root.join(format!("calc-{}", files::random_id()?)));
        fs::create_dir(&job.0).map_err(|e| e.to_string())?;
        fs::set_permissions(&job.0, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        for name in ["input", "out", "home", "runtime", "profile"] {
            files::private_dir(&job.0.join(name))?;
        }
        let ext = source
            .extension()
            .and_then(|s| s.to_str())
            .ok_or("workbook needs extension")?
            .to_ascii_lowercase();
        let input = format!("input/workbook.{ext}");
        files::copy_no_clobber(source, &job.0.join(&input))?;
        let app = launcher::executable("OMASHEETS_APP", "omasheets")?;
        let bridge = launcher::executable("OMASHEETS_UNO_BRIDGE", "omasheets-uno-bridge")?;
        if !Path::new("/usr/bin/bwrap").is_file() {
            return Err("Bubblewrap is required for Calc jobs".into());
        }
        if !Path::new("/usr/bin/soffice").is_file() {
            return Err("LibreOffice Calc is unavailable".into());
        }
        files::write_json(
            &job.0.join("request.json"),
            &json!({"action":action,"source":input,"arguments":arguments}),
        )?;
        let mut command = Command::new("/usr/bin/bwrap");
        command.args([
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--dir",
            "/run",
        ]);
        for p in ["/bin", "/sbin", "/lib", "/lib64"] {
            let p = Path::new(p);
            if p.is_symlink() {
                command
                    .arg("--symlink")
                    .arg(fs::read_link(p).map_err(|e| e.to_string())?)
                    .arg(p);
            } else if p.exists() {
                command.arg("--ro-bind").arg(p).arg(p);
            }
        }
        for p in [
            "/etc/fonts",
            "/etc/passwd",
            "/etc/group",
            "/etc/nsswitch.conf",
            "/etc/host.conf",
            "/etc/hosts",
            "/etc/localtime",
            "/etc/machine-id",
            "/var/cache/fontconfig",
        ] {
            if Path::new(p).exists() {
                command.args(["--ro-bind", p, p]);
            }
        }
        command
            .arg("--bind")
            .arg(&job.0)
            .arg("/job")
            .arg("--ro-bind")
            .arg(app)
            .arg("/omasheets-worker")
            .arg("--ro-bind")
            .arg(bridge)
            .arg("/omasheets-uno-bridge")
            .args([
                "--chdir",
                "/job",
                "--setenv",
                "HOME",
                "/job/home",
                "--setenv",
                "XDG_RUNTIME_DIR",
                "/job/runtime",
                "--setenv",
                "PATH",
                "/usr/bin",
                "--setenv",
                "LANG",
                "C.UTF-8",
                "--setenv",
                "LC_ALL",
                "C.UTF-8",
                "--setenv",
                "SAL_USE_VCLPLUGIN",
                "svp",
                "--setenv",
                "OMASHEETS_UNO_BRIDGE",
                "/omasheets-uno-bridge",
                "/omasheets-worker",
                "--calc-worker",
                "/job/request.json",
                "/job/result.json",
            ]);
        command
            .current_dir(&job.0)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o077);
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for (resource, limit) in [
                    (libc::RLIMIT_CPU, 60u64),
                    (libc::RLIMIT_AS, 2 * 1024 * 1024 * 1024),
                    (libc::RLIMIT_FSIZE, 256 * 1024 * 1024),
                    (libc::RLIMIT_NOFILE, 128),
                ] {
                    let limit = libc::rlimit {
                        rlim_cur: limit,
                        rlim_max: limit,
                    };
                    if libc::setrlimit(resource, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(90);
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break s;
            }
            if Instant::now() >= deadline {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                return Err("Calc job exceeded its time limit".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // A worker crash must not leave its office/adapter children behind.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let result = files::read_json(&job.0.join("result.json"), 4 * 1024 * 1024)
            .map_err(|_| "Calc worker returned no bounded result")?;
        if !status.success() || result["ok"] != true {
            return Err(redact(
                result["error"]
                    .as_str()
                    .unwrap_or("isolated Calc job failed"),
            ));
        }
        for (name, dest) in artifacts {
            let relative = result["artifacts"][name]
                .as_str()
                .ok_or_else(|| format!("Calc worker omitted {name}"))?;
            let relative = Path::new(relative);
            if relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
                || relative.parent() != Some(Path::new("out"))
            {
                return Err("Calc worker returned an invalid artifact path".into());
            }
            let candidate = job.0.join(relative);
            if files::identify(&candidate)?.size > 256 * 1024 * 1024 {
                return Err("Calc artifact exceeded size limit".into());
            }
            files::copy_no_clobber(&candidate, dest)?;
        }
        if !result["result"].is_object() {
            return Err("Calc worker returned an invalid result".into());
        }
        Ok(result["result"].clone())
    }
}
fn redact(s: &str) -> String {
    let url = regex::Regex::new(r"[A-Za-z][A-Za-z0-9+.-]*://\S+").unwrap();
    let path = regex::Regex::new(r"/(?:[^\s:]+/)*[^\s:]*").unwrap();
    path.replace_all(&url.replace_all(s, "<url>"), "<path>")
        .chars()
        .take(512)
        .collect()
}
