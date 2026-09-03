//! The local service over a Unix socket, and the CLI client that speaks to
//! it.
//!
//! `serve` listens on `<runtime>/omasheets/native.sock`, where `<runtime>`
//! is `$XDG_RUNTIME_DIR` unless `--runtime-dir` says otherwise, and writes a
//! fresh random session token to `<runtime>/omasheets/native.token` with
//! mode 0600. Every connection must send the token as its first line or it
//! is closed. Each following line is one JSON [`Request`]; each answer is
//! one JSON line, `{"ok":true,"response":…}` or `{"ok":false,"error":…}`.
//! The socket lives in a directory only the user can enter, so nothing on
//! the network can reach it at all.
//!
//! `call` reads the token, sends one request (JSON on the command line or
//! on stdin) and prints the answer; its exit status is non-zero for a
//! refusal.

use omasheets_service::{Request, Service};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

const USAGE: &str = "usage:\n  omasheets-service serve [--runtime-dir DIR] [--once]\n  omasheets-service call [--runtime-dir DIR] [REQUEST_JSON]";
/// Largest request line accepted, so a client cannot exhaust memory.
const MAX_LINE_BYTES: u64 = 4 * 1024 * 1024;
const TOKEN_BYTES: usize = 32;

#[derive(Serialize, Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<omasheets_service::Response>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<omasheets_service::ServiceError>,
}

fn runtime_dir(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    let base = match explicit {
        Some(directory) => directory,
        None => env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or("XDG_RUNTIME_DIR is not set; pass --runtime-dir")?,
    };
    Ok(base.join("omasheets"))
}

fn socket_path(directory: &Path) -> PathBuf {
    directory.join("native.sock")
}

fn token_path(directory: &Path) -> PathBuf {
    directory.join("native.token")
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| format!("cannot read /dev/urandom: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn prepare_directory(directory: &Path) -> Result<(), String> {
    if !directory.is_dir() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    }
    let metadata = fs::metadata(directory).map_err(|error| error.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "{} must not be readable by other users",
            directory.display()
        ));
    }
    Ok(())
}

fn write_token(path: &Path, token: &str) -> Result<(), String> {
    let _ = fs::remove_file(path);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.write_all(token.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| error.to_string())
}

fn read_line_bounded(reader: &mut BufReader<UnixStream>) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    let read = reader.by_ref().take(MAX_LINE_BYTES).read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') {
        return Err(std::io::Error::other("request line exceeds the size limit"));
    }
    Ok(Some(line.trim_end().to_string()))
}

fn serve_connection(
    stream: UnixStream,
    token: &str,
    service: &Mutex<Service>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    match read_line_bounded(&mut reader)? {
        Some(presented) if constant_time_equal(presented.as_bytes(), token.as_bytes()) => {}
        _ => {
            writer.write_all(b"{\"ok\":false,\"error\":{\"code\":\"unauthorized\",\"message\":\"session token missing or wrong\"}}\n")?;
            return Ok(());
        }
    }
    while let Some(line) = read_line_bounded(&mut reader)? {
        if line.is_empty() {
            continue;
        }
        let envelope = match serde_json::from_str::<Request>(&line) {
            Ok(request) => match service.lock().expect("service lock").handle(request) {
                Ok(response) => Envelope {
                    ok: true,
                    response: Some(response),
                    error: None,
                },
                Err(error) => Envelope {
                    ok: false,
                    response: None,
                    error: Some(error),
                },
            },
            Err(error) => Envelope {
                ok: false,
                response: None,
                error: Some(omasheets_service::ServiceError {
                    code: "invalid_request".into(),
                    message: error.to_string(),
                    details: None,
                }),
            },
        };
        serde_json::to_writer(&mut writer, &envelope)?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn serve(directory: PathBuf, once: bool) -> Result<(), String> {
    prepare_directory(&directory)?;
    let token = random_token()?;
    write_token(&token_path(&directory), &token)?;
    let socket = socket_path(&directory);
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .map_err(|error| format!("cannot listen on {}: {error}", socket.display()))?;
    let service = Arc::new(Mutex::new(Service::default()));
    let token = Arc::new(token);
    eprintln!("omasheets-service: listening on {}", socket.display());
    for connection in listener.incoming() {
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("omasheets-service: accept failed: {error}");
                continue;
            }
        };
        let service = Arc::clone(&service);
        let token = Arc::clone(&token);
        if once {
            let result = serve_connection(stream, &token, &service);
            if let Err(error) = result {
                eprintln!("omasheets-service: connection failed: {error}");
            }
            break;
        }
        std::thread::spawn(move || {
            if let Err(error) = serve_connection(stream, &token, &service) {
                eprintln!("omasheets-service: connection failed: {error}");
            }
        });
    }
    let mut service = service.lock().expect("service lock");
    service.close_all().map_err(|error| error.to_string())?;
    let _ = fs::remove_file(&socket);
    let _ = fs::remove_file(token_path(&directory));
    Ok(())
}

fn call(directory: PathBuf, request: Option<String>) -> Result<bool, String> {
    let request = match request {
        Some(text) => text,
        None => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| error.to_string())?;
            text
        }
    };
    let request: Request = serde_json::from_str(request.trim())
        .map_err(|error| format!("request is not valid JSON for the service: {error}"))?;
    let token = fs::read_to_string(token_path(&directory))
        .map_err(|error| format!("no session token; is the service running? ({error})"))?;
    let stream = UnixStream::connect(socket_path(&directory))
        .map_err(|error| format!("cannot reach the service: {error}"))?;
    let mut writer = stream.try_clone().map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);
    writeln!(writer, "{}", token.trim()).map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut writer, &request).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    let mut answer = String::new();
    reader
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    let envelope: Envelope = serde_json::from_str(answer.trim()).map_err(|error| {
        format!("service answered with something that is not an envelope: {error}")
    })?;
    println!("{}", answer.trim());
    Ok(envelope.ok)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let (command, rest) = match arguments.split_first() {
        Some((command, rest)) => (command.as_str(), rest),
        None => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let mut directory = None;
    let mut once = false;
    let mut positional = Vec::new();
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--runtime-dir" => {
                directory = rest.get(index + 1).map(PathBuf::from);
                if directory.is_none() {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
                index += 2;
            }
            "--once" => {
                once = true;
                index += 1;
            }
            other if other.starts_with("--") => {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            other => {
                positional.push(other.to_string());
                index += 1;
            }
        }
    }
    let result = match (command, positional.as_slice()) {
        ("serve", []) => {
            runtime_dir(directory).and_then(|directory| serve(directory, once).map(|()| true))
        }
        ("call", []) => runtime_dir(directory).and_then(|directory| call(directory, None)),
        ("call", [request]) => {
            runtime_dir(directory).and_then(|directory| call(directory, Some(request.clone())))
        }
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            eprintln!("omasheets-service: {error}");
            ExitCode::FAILURE
        }
    }
}
