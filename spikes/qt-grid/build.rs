use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    println!("cargo:rerun-if-changed=src/native_clipboard.h");
    let builder = CxxQtBuilder::new_qml_module(
        QmlModule::new("io.omasheets.grid").qml_file("qml/Main.qml"),
    )
    .qt_module("Network")
    .files(["src/grid_model.rs"]);
    // Only add our private header directory; preserve generated build options.
    unsafe { builder.cc_builder(|cc| { cc.include("src"); }) }.build();
}
