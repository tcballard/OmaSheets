use omasheets_app::{Result, VERSION, agent, launcher, mcp};
use serde_json::json;
use std::path::Path;
fn run() -> Result<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version"] => println!("omasheets {VERSION}"),
        ["--provenance"] => println!(
            "{}",
            json!({"source_commit":option_env!("OMASHEETS_SOURCE_COMMIT").unwrap_or("development"),"source_sha256":option_env!("OMASHEETS_SOURCE_SHA256").unwrap_or("development")})
        ),
        [] | ["launch"] => println!(
            "{}",
            json!({"pid":launcher::launch(None)?,"window":"omasheets"})
        ),
        ["launch", path] => println!(
            "{}",
            json!({"pid":launcher::launch(Some(Path::new(path)))?,"window":"native-grid"})
        ),
        ["--native-host"] => return launcher::host(None),
        ["--native-host", path] => return launcher::host(Some(Path::new(path))),
        ["agent-session", "resource"] => println!("{}", agent::resource()?),
        ["agent-session", "tools"] => println!("{}", json!({"tools":agent::tools()})),
        ["agent-session", "call", tool, "--arguments", args] => println!(
            "{}",
            agent::call(tool, serde_json::from_str(args).map_err(|e| e.to_string())?)?
        ),
        ["mcp", "serve"] => {
            mcp::serve(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())?
        }
        ["--help"] | ["-h"] => println!(
            "OmaSheets Rust native runtime (migration in progress)\n  launch [DOCUMENT.omasheets]\n  agent-session resource|tools|call TOOL --arguments JSON\n  mcp serve\n  --version\n  --provenance\nCompatibility and installation commands are not migrated yet."
        ),
        _ => return Err(
            "Command is not yet available in the Rust migration; no Python fallback is executed"
                .into(),
        ),
    }
    Ok(0)
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => std::process::ExitCode::from(code.clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
