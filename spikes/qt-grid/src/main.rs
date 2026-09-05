mod grid_model;
mod clipboard;
mod service_client;
mod theme;

use cxx_qt::casting::Upcast;
use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QQmlEngine, QUrl};
use serde_json::json;
use std::pin::Pin;

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 && arguments[0] == std::ffi::OsStr::new("--provenance") {
        println!(
            "{}",
            json!({
                "source_commit": option_env!("OMASHEETS_SOURCE_COMMIT")
                    .unwrap_or("development"),
                "source_sha256": option_env!("OMASHEETS_SOURCE_SHA256")
                    .unwrap_or("development"),
            })
        );
        return;
    }
    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    if let Some(mut engine) = engine.as_mut() {
        engine.as_mut().load(&QUrl::from(
            "qrc:/qt/qml/io/omasheets/grid/qml/Main.qml",
        ));
        let engine: Pin<&mut QQmlEngine> = engine.upcast_pin();
        engine
            .on_quit(|_| {
                eprintln!("OmaSheets grid closed");
            })
            .release();
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
