//! Command-line primitives over a single `.omasheets` store: `replay`,
//! `check`, `branch`, `diff` and `merge`. Every mutation is attributed to a
//! human actor named on the command line; agents drive the library API on
//! their own branches and never reach `merge`.

use omasheets_core::{Actor, ActorKind, Severity};
use omasheets_store::{Store, StoreError};
use std::env;
use std::process::ExitCode;

const USAGE: &str = "usage:\n  omasheets-store replay FILE [--branch NAME]\n  omasheets-store check FILE [--branch NAME]\n  omasheets-store branch FILE NAME --from BRANCH --actor NAME\n  omasheets-store diff FILE SOURCE [--target NAME]\n  omasheets-store merge FILE SOURCE [--target NAME] --approved-by NAME";

fn option(arguments: &[String], flag: &str) -> Result<Option<String>, String> {
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == flag {
            return arguments
                .get(index + 1)
                .cloned()
                .map(Some)
                .ok_or_else(|| format!("{flag} needs a value"));
        }
        index += 1;
    }
    Ok(None)
}

fn check_flags(arguments: &[String], allowed: &[&str]) -> Result<(), String> {
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument.starts_with("--") {
            if !allowed.contains(&argument.as_str()) {
                return Err(USAGE.into());
            }
            index += 2;
        } else {
            return Err(USAGE.into());
        }
    }
    Ok(())
}

fn human(name: Option<String>) -> Result<Actor, String> {
    let name = name.ok_or_else(|| USAGE.to_string())?;
    if name.trim().is_empty() || name.chars().count() > 128 {
        return Err("actor names must be 1-128 characters".into());
    }
    Ok(Actor::new(ActorKind::Human, name))
}

fn timestamp() -> i64 {
    // The only clock read in the store surface: the wall time recorded on a
    // human's own command, never consulted by replay.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn run(arguments: Vec<String>) -> Result<bool, String> {
    let (command, file, rest) = match arguments.as_slice() {
        [command, file, rest @ ..] => (command.as_str(), file, rest),
        _ => return Err(USAGE.into()),
    };
    let mut store = Store::open(file).map_err(|error| error.to_string())?;
    let resolve = |store: &Store, name: Option<String>| -> Result<_, String> {
        store
            .branch_id(name.as_deref().unwrap_or("main"))
            .map_err(|error| error.to_string())
    };
    match command {
        "replay" => {
            check_flags(rest, &["--branch"])?;
            let branch = resolve(&store, option(rest, "--branch")?)?;
            store.evict();
            store.document(branch).map_err(|error| error.to_string())?;
            let report = store.load_report(branch).expect("loaded");
            println!(
                "{}",
                serde_json::to_string_pretty(report).map_err(|error| error.to_string())?
            );
            Ok(true)
        }
        "check" => {
            check_flags(rest, &["--branch"])?;
            let branch = resolve(&store, option(rest, "--branch")?)?;
            let results = store.check(branch).map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&results).map_err(|error| error.to_string())?
            );
            Ok(!results
                .iter()
                .any(|result| result.severity == Severity::Error && !result.passed))
        }
        "branch" => {
            let (name, flags) = match rest {
                [name, flags @ ..] if !name.starts_with("--") => (name, flags),
                _ => return Err(USAGE.into()),
            };
            check_flags(flags, &["--from", "--actor"])?;
            let from = resolve(&store, option(flags, "--from")?)?;
            let actor = human(option(flags, "--actor")?)?;
            let branch = store
                .create_branch(from, name, actor, timestamp())
                .map_err(|error| error.to_string())?;
            println!("{{\"branch\": \"{name}\", \"id\": \"{branch}\"}}");
            Ok(true)
        }
        "diff" => {
            let (source, flags) = match rest {
                [source, flags @ ..] if !source.starts_with("--") => (source, flags),
                _ => return Err(USAGE.into()),
            };
            check_flags(flags, &["--target"])?;
            let source = resolve(&store, Some(source.clone()))?;
            let target = resolve(&store, option(flags, "--target")?)?;
            let diff = store
                .diff(source, target)
                .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&diff).map_err(|error| error.to_string())?
            );
            Ok(true)
        }
        "merge" => {
            let (source, flags) = match rest {
                [source, flags @ ..] if !source.starts_with("--") => (source, flags),
                _ => return Err(USAGE.into()),
            };
            check_flags(flags, &["--target", "--approved-by"])?;
            let source = resolve(&store, Some(source.clone()))?;
            let target = resolve(&store, option(flags, "--target")?)?;
            let approver = human(option(flags, "--approved-by")?)?;
            match store.merge(source, target, approver, timestamp()) {
                Ok(report) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
                    );
                    store.close().map_err(|error| error.to_string())?;
                    Ok(true)
                }
                Err(StoreError::ChecksFailed(results)) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&results).map_err(|error| error.to_string())?
                    );
                    Err("merge refused: error-severity checks failed on the source branch".into())
                }
                Err(StoreError::Conflicts(touches)) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&touches).map_err(|error| error.to_string())?
                    );
                    Err("merge refused: conflicting objects need explicit resolution".into())
                }
                Err(error) => Err(error.to_string()),
            }
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            eprintln!("omasheets-store: {error}");
            ExitCode::FAILURE
        }
    }
}
