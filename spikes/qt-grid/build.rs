use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("io.omasheets.grid").qml_file("qml/Main.qml")
            .qml_file("qml/WorkbookActions.qml")
            .qml_file("qml/FirstSteps.qml"),
    )
    .qt_module("Network")
    .files(["src/grid_model.rs"])
    .build();
}
