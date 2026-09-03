mod grid_model;

use cxx_qt::casting::Upcast;
use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QQmlEngine, QUrl};
use std::pin::Pin;

fn main() {
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
