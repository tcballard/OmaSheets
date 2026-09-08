use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("io.omasheets.grid")
            .qml_file("qml/Main.qml")
            .qml_file("qml/WorkbookActions.qml")
            .qml_file("qml/FirstSteps.qml")
            .qml_file("qml/ProposalReview.qml")
            .qml_file("qml/SpreadsheetTools.qml")
            .qml_file("qml/GridMetrics.qml")
            .qml_file("qml/ChartView.qml"),
    )
    .qt_module("Network")
    .files(["src/grid_model.rs"])
    .build();
}
