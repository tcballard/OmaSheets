import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import QtCore

Item {
    id: controls
    property var gridModel
    property var finishEditing
    property bool blocked: false
    property alias newAction: newAction
    property alias openAction: openAction
    property alias importAction: importAction
    property alias compatibilityAction: compatibilityAction
    property alias xlsxAction: xlsxAction
    property alias csvAction: csvAction
    property alias parquetAction: parquetAction
    readonly property bool available: !blocked && !gridModel.busy && !report.visible
        && !openFile.visible && !newFile.visible && !importFile.visible
        && !importDestination.visible && !exportFile.visible && !compatibilityFile.visible
        && !importNotice.visible
    property url importSource
    property string exportFormat: "xlsx"

    function prepare() { return gridModel.homeMode || finishEditing(); }
    function chooseExport(format) {
        if (!prepare()) return;
        exportFormat = format;
        exportFile.open();
    }

    Action {
        id: newAction
        text: "New workbook…"
        shortcut: StandardKey.New
        enabled: controls.available
        onTriggered: { if (controls.prepare()) newFile.open(); }
    }
    Action {
        id: openAction
        text: "Open workbook…"
        shortcut: StandardKey.Open
        enabled: controls.available
        onTriggered: { if (controls.prepare()) openFile.open(); }
    }
    Action {
        id: importAction
        text: "Import Excel workbook…"
        enabled: controls.available
        onTriggered: { if (controls.prepare()) importNotice.open(); }
    }
    Action {
        id: compatibilityAction
        text: "Open Excel or OpenDocument…"
        enabled: controls.available
        onTriggered: { if (controls.prepare()) compatibilityFile.open(); }
    }
    Action {
        id: xlsxAction
        text: "Export workbook as Excel…"
        enabled: controls.available && gridModel.documentMode && !gridModel.homeMode
        onTriggered: controls.chooseExport("xlsx")
    }
    Action {
        id: csvAction
        text: "Export current sheet as CSV…"
        enabled: xlsxAction.enabled
        onTriggered: controls.chooseExport("csv")
    }
    Action {
        id: parquetAction
        text: "Export current sheet as Parquet…"
        enabled: xlsxAction.enabled
        onTriggered: controls.chooseExport("parquet")
    }

    FileDialog {
        id: newFile
        title: "Create workbook — choose a new filename"
        fileMode: FileDialog.SaveFile
        defaultSuffix: "omasheets"
        nameFilters: ["OmaSheets workbooks (*.omasheets)"]
        currentFolder: StandardPaths.writableLocation(StandardPaths.DocumentsLocation)
        onAccepted: gridModel.openDocument(selectedFile, true)
    }
    FileDialog {
        id: openFile
        title: "Open workbook"
        nameFilters: ["OmaSheets workbooks (*.omasheets)"]
        currentFolder: StandardPaths.writableLocation(StandardPaths.DocumentsLocation)
        onAccepted: gridModel.openDocument(selectedFile, false)
    }
    FileDialog {
        id: compatibilityFile
        title: "Open in the compatibility window"
        nameFilters: ["Excel and OpenDocument (*.xlsx *.xls *.xlsm *.ods)"]
        currentFolder: StandardPaths.writableLocation(StandardPaths.DocumentsLocation)
        onAccepted: gridModel.openCompatibility(selectedFile)
    }
    Dialog {
        id: importNotice
        anchors.centerIn: parent
        title: "Import a native copy"
        modal: true
        width: Math.min(540, controls.width - 32)
        height: Math.min(300, controls.height - 32)
        standardButtons: Dialog.Ok | Dialog.Cancel
        contentItem: Label {
            text: "The original Excel file stays unchanged. OmaSheets imports supported cells and formulas into a new .omasheets workbook. Formatting, charts, macros and some formulas may not carry over. A report shows what was preserved or omitted.\n\nChoose a source file, then a new destination."
            wrapMode: Text.WordWrap
        }
        onAccepted: importFile.open()
    }
    FileDialog {
        id: importFile
        title: "Choose Excel workbook to import"
        nameFilters: ["Excel workbooks (*.xlsx)"]
        currentFolder: StandardPaths.writableLocation(StandardPaths.DocumentsLocation)
        onAccepted: { controls.importSource = selectedFile; importDestination.open(); }
    }
    FileDialog {
        id: importDestination
        title: "Save imported workbook — choose a new filename"
        fileMode: FileDialog.SaveFile
        defaultSuffix: "omasheets"
        nameFilters: ["OmaSheets workbooks (*.omasheets)"]
        currentFolder: StandardPaths.writableLocation(StandardPaths.DocumentsLocation)
        onAccepted: gridModel.importDocument(controls.importSource, selectedFile)
    }
    FileDialog {
        id: exportFile
        title: "Export a copy — choose a new filename"
        fileMode: FileDialog.SaveFile
        defaultSuffix: controls.exportFormat
        nameFilters: [controls.exportFormat.toUpperCase() + " files (*." + controls.exportFormat + ")"]
        currentFolder: StandardPaths.writableLocation(StandardPaths.DocumentsLocation)
        onAccepted: gridModel.exportDocument(selectedFile, controls.exportFormat)
    }
    Dialog {
        id: report
        anchors.centerIn: parent
        title: "File operation"
        modal: true
        width: Math.min(600, controls.width - 32)
        height: Math.min(460, controls.height - 32)
        standardButtons: Dialog.Close
        contentItem: ScrollView {
            clip: true
            contentWidth: availableWidth
            TextArea {
                text: gridModel.operationMessage
                readOnly: true
                wrapMode: TextEdit.Wrap
                selectByMouse: true
                textFormat: TextEdit.PlainText
            }
        }
    }
    Connections {
        target: gridModel
        function onOperationMessageChanged() {
            if (gridModel.operationMessage.length > 0) report.open();
        }
    }
}
