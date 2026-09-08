import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window
import QtCore
import io.omasheets.grid 1.0

ApplicationWindow {
    id: window

    width: 1240
    height: 760
    minimumWidth: 760
    minimumHeight: 480
    visible: true
    title: backend.homeMode ? "OmaSheets" : backend.documentName + " — OmaSheets"
    color: palette.window
    property bool examplePending: false
    property bool tourVisible: false
    onClosing: close => { close.accepted = !backend.busy && grid.commitEdit(); }

    WorkbookActions {
        id: fileActions
        anchors.fill: parent
        gridModel: backend
        blocked: keyboardHelp.visible || updatePrompt.visible || proposalReview.visible || spreadsheetTools.blocked
        finishEditing: () => grid.commitEdit()
        onExampleRequested: window.examplePending = true
    }

    SpreadsheetTools {
        id: spreadsheetTools
        anchors.fill: parent
        gridModel: backend
        grid: grid
        finishEditing: () => grid.commitEdit()
    }
    GridMetrics {
        id: metrics
        rowCount: backend.rowCount
        columnCount: backend.columnCount
        defaultRowHeight: window.rowHeight
        defaultColumnWidth: window.cellWidth
        view: spreadsheetTools.sheetView
    }
    Action {
        id: undoAction
        text: "Undo"
        shortcut: StandardKey.Undo
        enabled: backend.documentMode && fileActions.available && !grid.hasDraft
        onTriggered: grid.undoSelection(false)
    }
    Action {
        id: redoAction
        text: "Redo"
        shortcut: "Ctrl+Shift+Z"
        enabled: undoAction.enabled
        onTriggered: grid.undoSelection(true)
    }
    Action {
        id: findAction
        text: "Find and replace…"
        shortcut: StandardKey.Find
        enabled: backend.documentMode && fileActions.available
        onTriggered: spreadsheetTools.showFind()
    }
    Action {
        id: gotoAction
        text: "Go to cell or range…"
        shortcut: "Ctrl+G"
        enabled: findAction.enabled
        onTriggered: spreadsheetTools.enter("goto", "Go to cell or range", "")
    }

    ProposalReview {
        id: proposalReview
        anchors.centerIn: parent
        gridModel: backend
        width: Math.min(1000, window.width - 32)
        height: Math.min(700, window.height - 32)
    }

    Action {
        id: askAgentAction
        text: "Ask Agent"
        shortcut: "Ctrl+Shift+A"
        enabled: backend.documentMode && !backend.homeMode && !backend.busy && fileActions.available
        onTriggered: { if (grid.commitEdit()) backend.askAgent(grid.selectionRow, grid.selectionColumn, grid.selectionRows, grid.selectionColumns); }
    }
    Action {
        id: reviewAction
        text: "Review proposals…"
        shortcut: "Ctrl+Shift+R"
        enabled: askAgentAction.enabled
        onTriggered: { if (grid.commitEdit()) proposalReview.open(); }
    }

    menuBar: MenuBar {
        Menu {
            title: "File"
            MenuItem { action: fileActions.newAction }
            MenuItem { action: fileActions.openAction }
            MenuItem { action: fileActions.importAction }
            MenuItem { action: fileActions.compatibilityAction }
            MenuSeparator {}
            MenuItem {
                text: "Save cell draft"
                enabled: !backend.homeMode && !backend.busy && fileActions.available
                onTriggered: grid.commitEdit()
            }
            MenuItem { action: fileActions.xlsxAction }
            MenuItem { action: fileActions.csvAction }
            MenuItem { action: fileActions.parquetAction }
            MenuSeparator {}
            MenuItem { text: "Close window"; onTriggered: window.close() }
        }
        Menu {
            title: "Edit"
            MenuItem {action:undoAction}
            MenuItem {action:redoAction}
            MenuSeparator {}
            MenuItem {text:"Copy selection";enabled:undoAction.enabled;onTriggered:grid.copySelection()}
            MenuItem {text:"Paste";enabled:undoAction.enabled;onTriggered:grid.pasteSelection()}
            MenuItem {text:"Fill down";enabled:findAction.enabled;onTriggered:spreadsheetTools.fill(false)}
            MenuItem {text:"Fill right";enabled:findAction.enabled;onTriggered:spreadsheetTools.fill(true)}
            MenuSeparator {}
            MenuItem {action:findAction}
            MenuItem {action:gotoAction}
        }
        Menu {
            title: "Format"
            enabled:findAction.enabled
            MenuItem {text:"Format cells…";onTriggered:spreadsheetTools.showFormat()}
            MenuItem {text:"Clear formatting";onTriggered:spreadsheetTools.run({action:"clear_format",range:spreadsheetTools.selection})}
            MenuItem {text:"Edit note…";onTriggered:spreadsheetTools.enter("note","Cell note",spreadsheetTools.cell.note || "")}
            MenuSeparator {}
            MenuItem {text:"Dimensions…";onTriggered:spreadsheetTools.showDimensions()}
            MenuItem {text:"Autofit selected columns";onTriggered:spreadsheetTools.run({action:"dimensions",range:spreadsheetTools.selection,autofit:true,width:null,height:null})}
            MenuSeparator {}
            MenuItem {text:"Merge selection";onTriggered:spreadsheetTools.run({action:"merge",range:spreadsheetTools.selection})}
            MenuItem {text:"Unmerge selection";onTriggered:spreadsheetTools.run({action:"merge",range:spreadsheetTools.selection,unmerge:true})}
        }
        Menu {
            title: "Data"
            enabled:findAction.enabled
            MenuItem {text:"Sort selected rows…";onTriggered:spreadsheetTools.showSort()}
            MenuItem {text:"Filter selection by current cell";onTriggered:spreadsheetTools.run({action:"filter",range:spreadsheetTools.selection,column:grid.currentColumn,text:spreadsheetTools.cell.raw_display || "",header:false})}
            MenuItem {text:"Clear filter";enabled:!!spreadsheetTools.sheetView.filter_active;onTriggered:spreadsheetTools.run({action:"clear_filter"})}
            MenuItem {text:"Remove duplicate rows…";onTriggered:spreadsheetTools.confirm({action:"deduplicate",range:spreadsheetTools.selection,header:true},"Remove duplicate rows?","Keeps the first selected row as a header and removes later duplicate rows, including their cells outside the selection.")}
            MenuSeparator {}
            MenuItem {text:"Highlight values…";onTriggered:spreadsheetTools.showConditional()}
            MenuItem {text:"Clear conditional formatting";onTriggered:spreadsheetTools.run({action:"clear_conditional"})}
            MenuItem {text:"Charts…";onTriggered:spreadsheetTools.showCharts()}
        }
        Menu {
            title: "Sheet"
            enabled:findAction.enabled
            MenuItem {text:"Add sheet…";onTriggered:spreadsheetTools.enter("add_sheet","New sheet","")}
            MenuItem {text:"Rename sheet…";onTriggered:spreadsheetTools.enter("rename_sheet","Rename sheet",backend.sheetName)}
            MenuItem {text:"Duplicate sheet…";onTriggered:spreadsheetTools.enter("duplicate_sheet","Duplicate sheet",backend.sheetName+" copy")}
            MenuItem {text:"Delete sheet…";onTriggered:spreadsheetTools.confirm({action:"delete_sheet"},"Delete sheet?","Delete “"+backend.sheetName+"” and all of its cells?")}
            MenuSeparator {}
            MenuItem {text:"Insert selected number of rows above";onTriggered:spreadsheetTools.run({action:"insert_rows",at:grid.selectionRow,count:grid.selectionRows})}
            MenuItem {text:"Insert selected number of columns before";onTriggered:spreadsheetTools.run({action:"insert_columns",at:grid.selectionColumn,count:grid.selectionColumns})}
            MenuItem {text:"Delete selected rows…";onTriggered:spreadsheetTools.confirm({action:"delete_rows",at:grid.selectionRow,count:grid.selectionRows},"Delete rows?","Delete "+grid.selectionRows+" entire rows starting at row "+(grid.selectionRow+1)+"?")}
            MenuItem {text:"Delete selected columns…";onTriggered:spreadsheetTools.confirm({action:"delete_columns",at:grid.selectionColumn,count:grid.selectionColumns},"Delete columns?","Delete "+grid.selectionColumns+" entire columns starting at "+backend.columnLabel(grid.selectionColumn)+"?")}
        }
        Menu {
            title: "View"
            enabled:findAction.enabled
            MenuItem {text:"Freeze top row";onTriggered:spreadsheetTools.run({action:"freeze",rows:1,columns:0})}
            MenuItem {text:"Freeze first column";onTriggered:spreadsheetTools.run({action:"freeze",rows:0,columns:1})}
            MenuItem {text:"Freeze above and before current cell";onTriggered:spreadsheetTools.run({action:"freeze",rows:grid.currentRow,columns:grid.currentColumn})}
            MenuItem {text:"Unfreeze panes";onTriggered:spreadsheetTools.run({action:"freeze",rows:0,columns:0})}
            MenuItem {text:spreadsheetTools.sheetView.show_grid_lines===false ? "Show gridlines" : "Hide gridlines";onTriggered:spreadsheetTools.run({action:"grid_lines",visible:spreadsheetTools.sheetView.show_grid_lines===false})}
        }
        Menu {
            title: "Agent"
            MenuItem { action: askAgentAction }
            MenuItem { action: reviewAction }
        }
        Menu {
            title: "Help"
            MenuItem { action: fileActions.exampleAction }
            MenuItem {
                text: "Keyboard help (F1)"
                enabled: !backend.busy
                onTriggered: keyboardHelp.open()
            }
            MenuSeparator {}
            MenuItem {
                text: "Updates…"
                enabled: fileActions.available
                onTriggered: { if (backend.homeMode || grid.commitEdit()) updatePrompt.open(); }
            }
        }
    }

    Dialog {
        id: updatePrompt
        anchors.centerIn: parent
        title: "Update OmaSheets"
        modal: true
        width: Math.min(480, window.width - 32)
        standardButtons: backend.packageManaged ? Dialog.Close : Dialog.Ok | Dialog.Cancel
        contentItem: Label {
            text: backend.packageManaged
                ? "OmaSheets is managed by your system package manager.\n\nFor an AUR installation, use Omarchy’s Update menu. If you downloaded the package from GitHub, install a newer package from the releases page.\n\nClose OmaSheets before updating. Your workbooks stay on your computer."
                : "OmaSheets Setup will download and verify the latest development build. This workbook window will close so the app can be updated safely. Your committed edits are saved.\n\nClose any other OmaSheets windows before installing, then reopen the app from Setup."
            wrapMode: Text.WordWrap
        }
        onAccepted: { if (backend.openUpdater()) window.close(); }
    }

    FirstSteps {
        id: firstSteps
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: 48
        z: 20
        visible: window.tourVisible && !backend.homeMode && !backend.busy && !keyboardHelp.visible
        onFinished: { window.tourVisible = false; backend.finishTour(); body.forceActiveFocus(); }
        onSelectCell: (row, column) => grid.selectCell(row, column)
    }

    Shortcut {
        sequence: StandardKey.Save
        enabled: fileActions.available && !backend.homeMode
        onActivated: grid.commitEdit()
    }

    Shortcut {
        sequence: "F1"
        enabled: !keyboardHelp.visible && !backend.busy
        onActivated: keyboardHelp.open()
    }

    Dialog {
        id: keyboardHelp
        anchors.centerIn: parent
        width: Math.min(window.width - 32, 620)
        height: Math.min(window.height - 32, 640)
        title: "Keyboard help"
        modal: true
        focus: true
        closePolicy: Popup.CloseOnEscape
        standardButtons: Dialog.Close
        Shortcut {
            sequence: "F1"
            enabled: keyboardHelp.visible
            onActivated: keyboardHelp.close()
        }
        onOpened: helpScroll.forceActiveFocus()
        onClosed: {
            if (backend.homeMode)
                newWorkbookButton.forceActiveFocus();
            else if (editor.visible)
                editor.forceActiveFocus();
            else
                body.forceActiveFocus();
        }

        contentItem: ScrollView {
            id: helpScroll
            clip: true
            contentWidth: availableWidth
            ScrollBar.horizontal.policy: ScrollBar.AlwaysOff

            ColumnLayout {
                width: helpScroll.availableWidth
                spacing: 14

                Label {
                    Layout.fillWidth: true
                    text: "Select a cell, type a value or formula, then press Enter. "
                        + "Press F1 or Escape to close this guide and pick up where you left off."
                    wrapMode: Text.WordWrap
                    color: window.textColor
                }

                Repeater {
                    model: [
                        { heading: "Workbook", shortcuts: [
                            ["Ctrl+N", "Create a native workbook"],
                            ["Ctrl+O", "Open a native workbook"],
                            ["File menu", "Import, open Excel / ODS, or export a copy"]
                        ] },
                        { heading: "Move around", shortcuts: [
                            ["Arrow keys", "Move one cell"],
                            ["Tab / Shift+Tab", "Move right / left"],
                            ["Page Up / Page Down", "Move up / down a screen"],
                            ["Home / End", "First / last column in the row"],
                            ["Ctrl+Home / Ctrl+End", "First / last cell in the grid"],
                            ["Ctrl+Page Up / Page Down", "Previous / next sheet"]
                        ] },
                        { heading: "Edit a cell", shortcuts: [
                            ["Enter / F2", "Edit the selected cell"],
                            ["Start typing", "Replace the selected cell"],
                            ["Enter / Shift+Enter", "Save draft and move down / up"],
                            ["Tab / Shift+Tab", "Save draft and move right / left"],
                            ["Ctrl+S", "Save draft without moving"],
                            ["Escape", "Cancel the cell draft"],
                            ["Delete / Backspace", "Clear selected cells (outside the editor)"]
                        ] },
                        { heading: "Select and reuse", shortcuts: [
                            ["Shift+Arrow keys", "Extend a rectangular selection"],
                            ["Ctrl+C / Ctrl+V", "Copy selection / paste at its top-left cell"],
                            ["Ctrl+Z", "Undo an edit, clear or paste"],
                            ["Ctrl+Shift+Z / Ctrl+Y", "Redo"]
                        ] }
                    ]

                    delegate: ColumnLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        spacing: 6

                        Label {
                            text: modelData.heading
                            font.bold: true
                            color: window.accentColor
                        }

                        Repeater {
                            model: modelData.shortcuts
                            delegate: RowLayout {
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: 12
                                Label {
                                    Layout.preferredWidth: 220
                                    text: modelData[0]
                                    color: window.textColor
                                    font.family: "monospace"
                                    wrapMode: Text.WordWrap
                                }
                                Label {
                                    Layout.fillWidth: true
                                    text: modelData[1]
                                    color: window.textColor
                                    wrapMode: Text.WordWrap
                                }
                            }
                        }
                    }
                }

                Label {
                    Layout.fillWidth: true
                    text: "Start formulas with =. Prefix literal text with an apostrophe: '00123 or '=1+1."
                    wrapMode: Text.WordWrap
                    color: window.mutedColor
                }
                Label {
                    Layout.fillWidth: true
                    text: backend.homeMode
                        ? "Create or open a workbook from the File menu to get started."
                        : backend.documentMode
                        ? "Native document edits save when committed. Selection clipboard and undo shortcuts apply outside the cell editor."
                        : "Demo grid: edits are not saved. Selection clipboard and undo require a native document."
                    wrapMode: Text.WordWrap
                    color: window.mutedColor
                }
            }
        }
    }

    TextEdit {
        id: clipboardBuffer
        visible: false
        textFormat: TextEdit.PlainText
    }

    function blend(from, to, amount) {
        return Qt.rgba(from.r + (to.r - from.r) * amount,
            from.g + (to.g - from.g) * amount,
            from.b + (to.b - from.b) * amount,
            from.a + (to.a - from.a) * amount);
    }

    readonly property color canvasColor: backend.themeBackground
    readonly property color textColor: backend.themeForeground
    readonly property color accentColor: backend.themeAccent
    readonly property color mutedColor: backend.themeMuted
    readonly property color formulaColor: backend.themeMagenta
    readonly property color successColor: backend.themeGreen
    readonly property color warningColor: backend.themeYellow
    readonly property color errorColor: backend.themeRed
    readonly property color panelColor: blend(canvasColor, textColor, 0.055)
    readonly property color gridLineColor: blend(canvasColor, textColor, 0.14)
    readonly property color headerColor: blend(canvasColor, textColor, 0.025)
    readonly property color selectedHeaderColor: blend(canvasColor, accentColor, 0.20)
    readonly property color selectedCellColor: blend(canvasColor, accentColor, 0.12)
    readonly property color alternateRowColor: blend(canvasColor, textColor, 0.018)
    readonly property int rowHeight: 27
    readonly property int cellWidth: 132
    readonly property int rowHeaderWidth: 62
    readonly property int columnHeaderHeight: 29

    palette.window: canvasColor
    palette.windowText: textColor
    palette.base: panelColor
    palette.text: textColor
    palette.highlight: accentColor
    palette.button: panelColor
    palette.buttonText: textColor
    palette.highlightedText: canvasColor

    GridModel {
        id: backend
    }

    Connections {
        target: backend
        function onDocumentGenerationChanged() {
            grid.currentRow = 0;
            grid.currentColumn = 0;
            grid.anchorRow = 0;
            grid.anchorColumn = 0;
            body.contentX = 0;
            body.contentY = 0;
            body.forceActiveFocus();
            window.tourVisible = window.examplePending;
            if (window.examplePending) {
                firstSteps.step = 0;
                grid.selectCell(1, 1);
                window.examplePending = false;
            }
        }
        function onOperationMessageChanged() {
            if (!backend.busy && backend.operationMessage.length > 0) window.examplePending = false;
        }
    }

    ColumnLayout {
        id: welcomePane
        anchors.centerIn: parent
        width: Math.min(520, parent.width - 48)
        spacing: 14
        visible: backend.homeMode
        enabled: !backend.busy
        Label {
            text: "OMA / SHEETS"
            color: window.accentColor
            font.family: "monospace"
            font.bold: true
            font.pixelSize: 18
        }
        Label {
            text: backend.tourSeen ? "Your next workbook starts here." : "Welcome to OmaSheets."
            color: window.textColor
            font.pixelSize: 25
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
        }
        Label {
            text: "Create a workbook or pick up an existing one. Your files and edits stay on this computer."
            color: window.mutedColor
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
        }
        RowLayout {
            Button { id: newWorkbookButton; action: fileActions.newAction; highlighted: true; focus: backend.homeMode }
            Button { action: fileActions.openAction }
        }
        Button { action: fileActions.exampleAction }
        Label {
            text: "New here? Try a small budget, change a number, and watch the formulas update. A four-step guide shows you around. You choose where to save your practice workbook."
            color: window.mutedColor
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
        }
        RowLayout {
            Button { action: fileActions.importAction }
            Button { action: fileActions.compatibilityAction }
        }
        Label {
            text: "Native workbooks save as you finish each cell edit. Press F1 for the keyboard guide."
            color: window.mutedColor
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
        }
    }

    Popup {
        anchors.centerIn: parent
        visible: backend.busy
        modal: true
        focus: true
        closePolicy: Popup.NoAutoClose
        contentItem: RowLayout {
            BusyIndicator { running: backend.busy }
            Label { text: "Working…" }
        }
    }

    Timer {
        interval: 1500
        repeat: true
        running: !backend.benchmark
        onTriggered: backend.refreshTheme()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0
        visible: !backend.homeMode
        enabled: !backend.busy

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 46
            color: window.headerColor

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 15
                anchors.rightMargin: 15
                spacing: 12

                Label {
                    text: "OMA / SHEETS"
                    color: window.accentColor
                    font.family: "monospace"
                    font.bold: true
                    font.pixelSize: 13
                }

                Rectangle {
                    Layout.preferredWidth: 1
                    Layout.preferredHeight: 18
                    color: window.gridLineColor
                }

                Label {
                    Layout.maximumWidth: 240
                    elide: Text.ElideRight
                    text: backend.documentName
                    textFormat: Text.PlainText
                    color: window.textColor
                    font.pixelSize: 14
                    font.weight: Font.DemiBold
                }

                Label {
                    visible: window.width > 1000
                    text: backend.rowCount.toLocaleString(Qt.locale("en_US"), "f", 0)
                        + " rows  ·  " + backend.columnCount + " columns  ·  " + backend.sheetName
                    color: window.mutedColor
                    font.family: "monospace"
                    font.pixelSize: 11
                }

                Item { Layout.fillWidth: true }

                Label {
                    text: (backend.documentMode ? "SAVED ON THIS COMPUTER" : "PRACTICE GRID")
                    textFormat: Text.PlainText
                    color: window.mutedColor
                    font.family: "monospace"
                    font.pixelSize: 10
                    font.letterSpacing: 1.2
                }
            }
        }

        ToolBar {
            Layout.fillWidth: true
            Layout.preferredHeight: 36
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 8
                anchors.rightMargin: 8
                spacing: 4
                ToolButton {text:"B";font.bold:true;checkable:true;checked:!!spreadsheetTools.style.bold;enabled:findAction.enabled;onClicked:spreadsheetTools.toggle("bold");ToolTip.text:"Bold";ToolTip.visible:hovered}
                ToolButton {text:"I";font.italic:true;checkable:true;checked:!!spreadsheetTools.style.italic;enabled:findAction.enabled;onClicked:spreadsheetTools.toggle("italic");ToolTip.text:"Italic";ToolTip.visible:hovered}
                ToolButton {text:"%";enabled:findAction.enabled;onClicked:spreadsheetTools.format({number_format:"0.0%"});ToolTip.text:"Percent format";ToolTip.visible:hovered}
                ToolButton {text:"Format…";enabled:findAction.enabled;onClicked:spreadsheetTools.showFormat()}
                ToolSeparator {}
                ToolButton {text:"Find";action:findAction}
                ToolButton {text:"Charts";enabled:findAction.enabled;onClicked:spreadsheetTools.showCharts()}
                Item {Layout.fillWidth:true}
                ToolButton {action:askAgentAction}
                ToolButton {text:"Review";action:reviewAction}
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 38
            color: window.panelColor
            border.color: window.gridLineColor

            RowLayout {
                anchors.fill: parent
                spacing: 0

                TextField {
                    id: addressBox
                    Layout.preferredWidth: 96
                    Layout.fillHeight: true
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    selectByMouse: true
                    text: backend.columnLabel(grid.currentColumn) + (grid.currentRow + 1)
                    onAccepted: {if(!spreadsheetTools.goTo(text))spreadsheetTools.enter("goto","Go to cell or range",text);}
                    color: window.accentColor
                    font.family: "monospace"
                    font.bold: true
                }

                Rectangle {
                    Layout.preferredWidth: 1
                    Layout.fillHeight: true
                    color: window.gridLineColor
                }

                Label {
                    Layout.preferredWidth: 34
                    Layout.fillHeight: true
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    text: "fx"
                    color: window.formulaColor
                    font.family: "serif"
                    font.italic: true
                }

                TextField {
                    id: formulaBar
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    leftPadding: 6
                    selectByMouse: true
                    property bool editing: false
                    property string original: ""
                    property string draft: ""
                    text: {backend.revision;return editing ? draft : backend.cellPreview(grid.currentRow,grid.currentColumn);}
                    onActiveFocusChanged: {
                        if(activeFocus && !editing){
                            if(!grid.commitEdit() || !backend.prepareCellEdit(grid.currentRow,grid.currentColumn))return;
                            original=backend.cellInput(grid.currentRow,grid.currentColumn);draft=original;editing=true;forceActiveFocus();
                        }
                    }
                    onTextEdited:draft=text
                    onAccepted:grid.commitEdit()
                    Keys.onEscapePressed:{editing=false;body.forceActiveFocus();}
                    color: window.textColor
                    font.family: "monospace"
                    font.pixelSize: 12
                    Accessible.name: "Formula bar"
                }

                Label {
                    Layout.preferredWidth: window.width > 1000 ? 280 : 180
                    Layout.fillHeight: true
                    rightPadding: 12
                    horizontalAlignment: Text.AlignRight
                    verticalAlignment: Text.AlignVCenter
                    text: backend.sourceStatus
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    HoverHandler { id: statusHover }
                    ToolTip {
                        visible: statusHover.hovered
                        width: 360
                        contentItem: Text {
                            text: backend.sourceStatus
                            textFormat: Text.PlainText
                            wrapMode: Text.WordWrap
                            color: window.textColor
                        }
                        background: Rectangle {
                            color: window.panelColor
                            border.color: window.gridLineColor
                        }
                    }
                    color: window.mutedColor
                    font.pixelSize: 10
                }
            }
        }

        Item {
            id: grid

            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true

            property int currentRow: 0
            property int currentColumn: 0
            property int anchorRow: 0
            property int anchorColumn: 0
            readonly property int selectionRow: Math.min(currentRow, anchorRow)
            readonly property int selectionColumn: Math.min(currentColumn, anchorColumn)
            readonly property int selectionRows: Math.abs(currentRow - anchorRow) + 1
            readonly property int selectionColumns: Math.abs(currentColumn - anchorColumn) + 1
            readonly property bool hasDraft: editor.visible || formulaBar.editing
            readonly property var visibleRows: metrics.visibleRows(body.contentY,body.height)
            readonly property var visibleColumns: metrics.visibleColumns(body.contentX,body.width)
            readonly property int visibleRowCount: visibleRows.length
            readonly property int visibleColumnCount: visibleColumns.length
            readonly property var visibleCells: metrics.cells(visibleRows,visibleColumns,body.contentX,body.contentY,body.width,body.height)
            readonly property int visibleDelegates: visibleCells.length
            onCurrentRowChanged: selectionStats.restart()
            onCurrentColumnChanged: selectionStats.restart()
            onAnchorRowChanged: selectionStats.restart()
            onAnchorColumnChanged: selectionStats.restart()

            function selectCell(row, column, extend) {
                if (!commitEdit())
                    return false;
                row=Math.max(0,Math.min(backend.rowCount-1,row));
                column=Math.max(0,Math.min(backend.columnCount-1,column));
                const direction=row<currentRow ? -1 : 1;
                while(row>=0 && row<backend.rowCount && metrics.rowHeight(row)===0)row+=direction;
                if(row<0 || row>=backend.rowCount)return false;
                const merge=metrics.mergeAt(row,column);
                if(merge && !extend){row=merge.row;column=merge.column;}
                currentRow=row;
                currentColumn=column;
                if (!extend) {
                    anchorRow = currentRow;
                    anchorColumn = currentColumn;
                }
                ensureVisible();
                body.forceActiveFocus();
                return true;
            }

            function ensureVisible() {
                const left=metrics.columnPosition(currentColumn),right=left+metrics.columnWidth(currentColumn);
                const top=metrics.rowPosition(currentRow),bottom=top+metrics.rowHeight(currentRow);
                if(currentColumn>=metrics.frozenColumns){
                    if(left<body.contentX+metrics.frozenWidth)body.contentX=Math.max(0,left-metrics.frozenWidth);
                    else if(right>body.contentX+body.width)body.contentX=right-body.width;
                }
                if(currentRow>=metrics.frozenRows){
                    if(top<body.contentY+metrics.frozenHeight)body.contentY=Math.max(0,top-metrics.frozenHeight);
                    else if(bottom>body.contentY+body.height)body.contentY=bottom-body.height;
                }
            }

            function moveCell(rowDelta,columnDelta,extend){
                const merge=metrics.mergeAt(currentRow,currentColumn);
                return selectCell(currentRow+(merge && rowDelta>0 ? merge.rows : rowDelta),
                    currentColumn+(merge && columnDelta>0 ? merge.columns : columnDelta),extend);
            }

            function beginEdit(replacement) {
                if(formulaBar.editing && !commitEdit())return;
                if(metrics.rowHeight(currentRow)===0)return;
                if (editor.visible) {
                    editor.forceActiveFocus();
                    return;
                }
                if (!backend.prepareCellEdit(currentRow, currentColumn))
                    return;
                editor.originalText = backend.cellInput(currentRow, currentColumn);
                editor.text = replacement === undefined ? editor.originalText : replacement;
                editor.visible = true;
                editor.forceActiveFocus();
                if (replacement === undefined)
                    editor.selectAll();
                else
                    editor.cursorPosition = editor.text.length;
            }

            function commitEdit() {
                if(formulaBar.editing){
                    if(formulaBar.draft!==formulaBar.original && !backend.setCellText(currentRow,currentColumn,formulaBar.draft)){
                        formulaBar.forceActiveFocus();return false;
                    }
                    formulaBar.editing=false;body.forceActiveFocus();
                }
                if (!editor.visible)
                    return true;
                if (editor.text !== editor.originalText
                        && !backend.setCellText(currentRow, currentColumn, editor.text)) {
                    editor.forceActiveFocus();
                    return false;
                }
                editor.visible = false;
                body.forceActiveFocus();
                return true;
            }

            function finishEdit(rowDelta, columnDelta) {
                if (commitEdit())
                    moveCell(rowDelta,columnDelta);
            }

            function clearCell() {
                if (editor.visible)
                    return;
                if (selectionRows > 1 || selectionColumns > 1) {
                    backend.clearCells(selectionRow, selectionColumn, selectionRows, selectionColumns);
                    return;
                }
                beginEdit("");
                commitEdit();
            }

            function copySelection() {
                if (editor.visible)
                    return;
                if (!backend.copyRange(selectionRow, selectionColumn,
                        selectionRows, selectionColumns))
                    return;
                body.forceActiveFocus();
            }

            function pasteSelection() {
                if (editor.visible)
                    return;
                clipboardBuffer.text = "";
                clipboardBuffer.paste();
                const text = clipboardBuffer.text;
                clipboardBuffer.text = "";
                if (text.length > 0 && backend.pasteCells(selectionRow, selectionColumn, text))
                    selectCell(selectionRow, selectionColumn);
                body.forceActiveFocus();
            }

            function undoSelection(redo) {
                if (!hasDraft)
                    backend.undoEdit(redo);
            }

            function switchSheet(index) {
                if (index < 0 || index >= backend.sheetCount || index === backend.currentSheet)
                    return;
                if (!commitEdit())
                    return;
                backend.selectSheet(index);
                currentRow = 0;
                currentColumn = 0;
                anchorRow = 0;
                anchorColumn = 0;
                body.contentX = 0;
                body.contentY = 0;
                body.forceActiveFocus();
            }

            Rectangle {
                width: window.rowHeaderWidth
                height: window.columnHeaderHeight
                color: window.headerColor
                border.color: window.gridLineColor

                Label {
                    anchors.centerIn: parent
                    text: "#"
                    color: window.mutedColor
                    font.family: "monospace"
                    font.pixelSize: 10
                }
            }

            Item {
                id: columnHeaders
                x: window.rowHeaderWidth
                width: parent.width - x
                height: window.columnHeaderHeight
                clip: true

                Repeater {
                    model: grid.visibleColumnCount

                    delegate: Rectangle {
                        required property int index
                        readonly property int logicalColumn: grid.visibleColumns[index]

                        z: logicalColumn<metrics.frozenColumns ? 2 : 0
                        x: metrics.screenColumn(logicalColumn,body.contentX)
                        width: metrics.columnWidth(logicalColumn)
                        height: window.columnHeaderHeight
                        color: logicalColumn === grid.currentColumn
                            ? window.selectedHeaderColor : window.headerColor
                        border.color: window.gridLineColor

                        TapHandler {onDoubleTapped:spreadsheetTools.run({action:"dimensions",range:{row:0,column:parent.logicalColumn,rows:1,columns:1},width:null,height:null,autofit:true})}
                        Rectangle {
                            id:columnResize
                            anchors.right:parent.right
                            width:5
                            height:parent.height
                            color:"transparent"
                            property real startSize:0
                            property real proposed:0
                            HoverHandler {cursorShape:Qt.SizeHorCursor}
                            DragHandler {
                                target:null
                                onActiveChanged:{if(active){columnResize.startSize=columnResize.parent.width;columnResize.proposed=columnResize.startSize;}else spreadsheetTools.run({action:"dimensions",range:{row:0,column:columnResize.parent.logicalColumn,rows:1,columns:1},width:Math.max(24,Math.min(1200,columnResize.proposed)),height:null});}
                                onTranslationChanged:{if(active)columnResize.proposed=columnResize.startSize+translation.x;}
                            }
                        }
                        Label {
                            anchors.centerIn: parent
                            text: backend.columnLabel(parent.logicalColumn)
                            color: parent.logicalColumn === grid.currentColumn ? window.accentColor : window.mutedColor
                            font.family: "monospace"
                            font.pixelSize: 10
                            font.bold: parent.logicalColumn === grid.currentColumn
                        }
                    }
                }
            }

            Item {
                id: rowHeaders
                y: window.columnHeaderHeight
                width: window.rowHeaderWidth
                height: parent.height - y
                clip: true

                Repeater {
                    model: grid.visibleRowCount

                    delegate: Rectangle {
                        required property int index
                        readonly property int logicalRow: grid.visibleRows[index]

                        z: logicalRow<metrics.frozenRows ? 2 : 0
                        y: metrics.screenRow(logicalRow,body.contentY)
                        width: window.rowHeaderWidth
                        height: metrics.rowHeight(logicalRow)
                        color: logicalRow === grid.currentRow
                            ? window.selectedHeaderColor : window.headerColor
                        border.color: window.gridLineColor

                        Rectangle {
                            id:rowResize
                            anchors.bottom:parent.bottom
                            width:parent.width
                            height:5
                            color:"transparent"
                            property real startSize:0
                            property real proposed:0
                            HoverHandler {cursorShape:Qt.SizeVerCursor}
                            DragHandler {
                                target:null
                                onActiveChanged:{if(active){rowResize.startSize=rowResize.parent.height;rowResize.proposed=rowResize.startSize;}else spreadsheetTools.run({action:"dimensions",range:{row:rowResize.parent.logicalRow,column:0,rows:1,columns:1},height:Math.max(16,Math.min(600,rowResize.proposed)),width:null});}
                                onTranslationChanged:{if(active)rowResize.proposed=rowResize.startSize+translation.y;}
                            }
                        }
                        Label {
                            anchors.centerIn: parent
                            text: (parent.logicalRow + 1).toLocaleString(Qt.locale("en_US"), "f", 0)
                            color: parent.logicalRow === grid.currentRow ? window.accentColor : window.mutedColor
                            font.family: "monospace"
                            font.pixelSize: 10
                        }
                    }
                }
            }

            Flickable {
                id: body

                x: window.rowHeaderWidth
                y: window.columnHeaderHeight
                width: parent.width - x
                height: parent.height - y
                contentWidth: metrics.contentWidth
                contentHeight: metrics.contentHeight
                boundsBehavior: Flickable.StopAtBounds
                flickDeceleration: 5500
                maximumFlickVelocity: 9000
                clip: true
                focus: true
                activeFocusOnTab: true

                Accessible.role: Accessible.Table
                Accessible.name: "OmaSheets data grid"
                Accessible.description: "One million row virtual spreadsheet. Arrow keys move the selected cell and Enter edits it."
                Accessible.focusable: true

                ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
                ScrollBar.horizontal: ScrollBar { policy: ScrollBar.AsNeeded }

                Item {
                    id: contentLayer
                    width: body.contentWidth
                    height: body.contentHeight

                    Repeater {
                        model: grid.visibleDelegates

                        delegate: Rectangle {
                            id: cell

                            required property int index
                            readonly property int logicalRow: grid.visibleCells[index].row
                            readonly property int logicalColumn: grid.visibleCells[index].column
                            readonly property var merge: metrics.mergeAt(logicalRow,logicalColumn)
                            readonly property bool covered: merge!==null && (merge.row!==logicalRow || merge.column!==logicalColumn)
                            readonly property var presentation: {backend.revision;return JSON.parse(backend.cellPresentation(logicalRow,logicalColumn) || "{}");}
                            readonly property var style: presentation.style || ({})
                            readonly property string valueKind: {
                                backend.revision;
                                return backend.cellKind(logicalRow, logicalColumn);
                            }
                            readonly property bool selectedCell: logicalRow >= grid.selectionRow
                                && logicalRow < grid.selectionRow + grid.selectionRows
                                && logicalColumn >= grid.selectionColumn
                                && logicalColumn < grid.selectionColumn + grid.selectionColumns

                            visible: !covered
                            clip: true
                            z: (logicalRow<metrics.frozenRows ? 2 : 0)+(logicalColumn<metrics.frozenColumns ? 1 : 0)
                            x: metrics.screenColumn(logicalColumn,body.contentX)+body.contentX
                            y: metrics.screenRow(logicalRow,body.contentY)+body.contentY
                            width: merge ? metrics.columnPosition(merge.column+merge.columns)-metrics.columnPosition(merge.column) : metrics.columnWidth(logicalColumn)
                            height: merge ? metrics.rowPosition(merge.row+merge.rows)-metrics.rowPosition(merge.row) : metrics.rowHeight(logicalRow)
                            color: selectedCell ? window.selectedCellColor
                                : (style.background || (logicalRow % 2 === 0 ? window.canvasColor : window.alternateRowColor))
                            border.width: selectedCell || style.border==="all" || spreadsheetTools.sheetView.show_grid_lines!==false ? 1 : 0
                            border.color: selectedCell ? window.accentColor : style.border==="all" ? window.textColor : window.gridLineColor

                            Accessible.role: Accessible.StaticText
                            Accessible.name: backend.columnLabel(logicalColumn) + (logicalRow + 1)
                            Accessible.description: "Spreadsheet cell, " + valueKind
                                + ", value " + valueLabel.text
                            Accessible.focusable: true
                            Accessible.focused: selectedCell && body.activeFocus
                            Accessible.selected: selectedCell

                            Label {
                                id: valueLabel
                                textFormat: Text.PlainText
                                anchors.fill: parent
                                leftPadding: 7
                                rightPadding: 7
                                verticalAlignment: Text.AlignVCenter
                                horizontalAlignment: cell.style.alignment==="center" ? Text.AlignHCenter : cell.style.alignment==="left" ? Text.AlignLeft
                                    : cell.style.alignment==="right" || cell.presentation.value_type==="number" || cell.valueKind==="number" ? Text.AlignRight : Text.AlignLeft
                                wrapMode: cell.style.wrap ? Text.WordWrap : Text.NoWrap
                                elide: cell.style.wrap ? Text.ElideNone : Text.ElideRight
                                text: {
                                    backend.revision;
                                    return backend.cellText(parent.logicalRow, parent.logicalColumn);
                                }
                                color: cell.style.foreground || (parent.valueKind === "formula"
                                    ? window.formulaColor
                                    : (parent.logicalColumn % 6 === 5 && text === "Reviewed"
                                        ? window.successColor : window.textColor))
                                font.family: "monospace"
                                font.pointSize: cell.style.font_size || 8.25
                                font.bold: !!cell.style.bold
                                font.italic: !!cell.style.italic
                                font.underline: !!cell.style.underline
                            }

                            Rectangle {anchors.left:parent.left;anchors.right:parent.right;anchors.bottom:parent.bottom;height:1;color:window.textColor;visible:cell.style.border==="bottom"}
                            Rectangle {anchors.top:parent.top;anchors.right:parent.right;width:5;height:5;color:window.accentColor;visible:!!cell.presentation.note}
                            HoverHandler {id:noteHover}
                            ToolTip {
                                visible:noteHover.hovered && !!cell.presentation.note
                                width:Math.min(380,window.width-32)
                                contentItem:Text {text:cell.presentation.note || "";textFormat:Text.PlainText;wrapMode:Text.WordWrap;color:window.textColor}
                            }
                            TapHandler {
                                acceptedButtons: Qt.LeftButton
                                onTapped: grid.selectCell(cell.logicalRow, cell.logicalColumn)
                                onDoubleTapped: {
                                    if (grid.selectCell(cell.logicalRow, cell.logicalColumn))
                                        grid.beginEdit();
                                }
                            }
                        }
                    }

                    TextField {
                        id: editor

                        property var merge: metrics.mergeAt(grid.currentRow,grid.currentColumn)
                        x: metrics.screenColumn(grid.currentColumn,body.contentX)+body.contentX
                        y: metrics.screenRow(grid.currentRow,body.contentY)+body.contentY
                        width: merge ? metrics.columnPosition(merge.column+merge.columns)-metrics.columnPosition(merge.column) : metrics.columnWidth(grid.currentColumn)
                        height: merge ? metrics.rowPosition(merge.row+merge.rows)-metrics.rowPosition(merge.row) : metrics.rowHeight(grid.currentRow)
                        visible: false
                        z: 10
                        leftPadding: 6
                        selectByMouse: true
                        font.family: "monospace"
                        font.pixelSize: 11

                        Accessible.role: Accessible.EditableText
                        Accessible.name: "Edit " + backend.columnLabel(grid.currentColumn) + (grid.currentRow + 1)
                        Accessible.description: "Type a new cell value. Enter commits and Escape cancels."

                        property string originalText: ""
                        onAccepted: grid.finishEdit(1, 0)
                        Keys.onReturnPressed: event => {
                            grid.finishEdit((event.modifiers & Qt.ShiftModifier) ? -1 : 1, 0);
                            event.accepted = true;
                        }
                        Keys.onEnterPressed: event => {
                            grid.finishEdit((event.modifiers & Qt.ShiftModifier) ? -1 : 1, 0);
                            event.accepted = true;
                        }
                        Keys.onTabPressed: event => {
                            grid.finishEdit(0, 1);
                            event.accepted = true;
                        }
                        Keys.onBacktabPressed: event => {
                            grid.finishEdit(0, -1);
                            event.accepted = true;
                        }
                        Keys.onEscapePressed: {
                            visible = false;
                            body.forceActiveFocus();
                        }
                    }
                }

                Keys.onPressed: event => {
                    const control = (event.modifiers & Qt.ControlModifier) !== 0;
                    const shift = (event.modifiers & Qt.ShiftModifier) !== 0;
                    if (control && event.key === Qt.Key_C)
                        grid.copySelection();
                    else if (control && event.key === Qt.Key_V)
                        grid.pasteSelection();
                    else if (control && event.key === Qt.Key_Z)
                        grid.undoSelection(shift);
                    else if (control && event.key === Qt.Key_Y)
                        grid.undoSelection(true);
                    else if (event.key === Qt.Key_PageUp && control)
                        grid.switchSheet(backend.currentSheet - 1);
                    else if (event.key === Qt.Key_PageDown && control)
                        grid.switchSheet(backend.currentSheet + 1);
                    else if (event.key === Qt.Key_Left)
                        grid.moveCell(0,-1,shift);
                    else if (event.key === Qt.Key_Right)
                        grid.moveCell(0,1,shift);
                    else if (event.key === Qt.Key_Up)
                        grid.moveCell(-1,0,shift);
                    else if (event.key === Qt.Key_Down)
                        grid.moveCell(1,0,shift);
                    else if (event.key === Qt.Key_PageUp)
                        grid.selectCell(grid.currentRow - Math.max(1, grid.visibleRowCount - 2), grid.currentColumn);
                    else if (event.key === Qt.Key_PageDown)
                        grid.selectCell(grid.currentRow + Math.max(1, grid.visibleRowCount - 2), grid.currentColumn);
                    else if (event.key === Qt.Key_Home && control)
                        grid.selectCell(0, 0);
                    else if (event.key === Qt.Key_End && control)
                        grid.selectCell(backend.rowCount - 1, backend.columnCount - 1);
                    else if (event.key === Qt.Key_Home)
                        grid.selectCell(grid.currentRow, 0);
                    else if (event.key === Qt.Key_End)
                        grid.selectCell(grid.currentRow, backend.columnCount - 1);
                    else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_F2)
                        grid.beginEdit();
                    else if (event.key === Qt.Key_Tab)
                        grid.moveCell(0,1);
                    else if (event.key === Qt.Key_Backtab)
                        grid.moveCell(0,-1);
                    else if (event.key === Qt.Key_Delete || event.key === Qt.Key_Backspace)
                        grid.clearCell();
                    else if (event.text.length > 0 && !control
                            && (event.modifiers & (Qt.AltModifier | Qt.MetaModifier)) === 0
                            && event.text.charCodeAt(0) >= 32)
                        grid.beginEdit(event.text);
                    else
                        return;
                    event.accepted = true;
                }
            }

            FrameAnimation {
                id: benchmarkAnimation

                running: backend.benchmark
                property int warmupFrames: 30
                property int measuredFrames: 180
                property int frameNumber: 0
                property real measuredStart: 0
                property var samples: []

                onTriggered: {
                    frameNumber += 1;
                    if (frameNumber === 1 && backend.documentMode) {
                        if (backend.sheetCount > 1)
                            grid.switchSheet(1);
                        if (!backend.pasteCells(0, 0, "7\t=A1*3")
                                || !backend.undoEdit(false) || !backend.undoEdit(true)) {
                            Qt.exit(1);
                            return;
                        }
                        grid.selectCell(0, 0);
                        grid.selectCell(0, 1, true);
                        grid.copySelection();
                        clipboardBuffer.text = "";
                        clipboardBuffer.paste();
                        if (clipboardBuffer.text !== "7\t=A1*3") {
                            Qt.exit(1);
                            return;
                        }
                        // Real MIME clipboard paste must translate B1's A1 to A2.
                        grid.selectCell(1, 0);
                        grid.pasteSelection();
                        if (backend.cellInput(1, 1) !== "=A2*3"
                                || backend.cellText(1, 1) !== "21"
                                || !backend.undoEdit(false) || !backend.undoEdit(true)
                                || backend.cellInput(1, 1) !== "=A2*3") {
                            Qt.exit(1);
                            return;
                        }
                        // Another application's identical plain text has no origin.
                        clipboardBuffer.text = "7\t=A1*3";
                        clipboardBuffer.selectAll();
                        clipboardBuffer.copy();
                        grid.selectCell(1, 0);
                        grid.pasteSelection();
                        if (backend.cellInput(1, 1) !== "=A1*3") {
                            Qt.exit(1);
                            return;
                        }
                        clipboardBuffer.text = "";
                        grid.selectCell(0, 0);
                    }
                    const progress = Math.min(1, frameNumber / (warmupFrames + measuredFrames));
                    body.contentY = progress * Math.max(0, body.contentHeight - body.height);
                    body.contentX = (0.5 - 0.5 * Math.cos(progress * Math.PI * 4))
                        * Math.max(0, body.contentWidth - body.width);

                    if (frameNumber === warmupFrames)
                        measuredStart = elapsedTime;
                    else if (frameNumber > warmupFrames)
                        samples.push(frameTime * 1000);

                    if (frameNumber >= warmupFrames + measuredFrames) {
                        running = false;
                        samples.sort((a, b) => a - b);
                        const p95Index = Math.min(samples.length - 1, Math.ceil(samples.length * 0.95) - 1);
                        backend.reportBenchmark(samples.length, elapsedTime - measuredStart,
                            samples[p95Index], samples[samples.length - 1], grid.visibleDelegates);
                        Qt.quit();
                    }
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 34
            color: window.headerColor
            border.color: window.gridLineColor

            Flickable {
                anchors.fill: parent
                anchors.leftMargin: window.rowHeaderWidth
                anchors.rightMargin: helpButton.width + statsLabel.width + 28
                contentWidth: sheetTabs.width
                contentHeight: height
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                Row {
                    id: sheetTabs
                    height: parent.height
                    spacing: 2

                    Repeater {
                        model: backend.sheetCount

                        delegate: Rectangle {
                            id: sheetTab
                            required property int index
                            readonly property bool selectedTab: index === backend.currentSheet

                            width: Math.max(96, sheetLabel.implicitWidth + 30)
                            height: sheetTabs.height
                            color: selectedTab ? window.selectedHeaderColor : "transparent"

                            Accessible.role: Accessible.Button
                            Accessible.name: "Sheet " + sheetLabel.text
                            Accessible.description: selectedTab ? "Current sheet" : "Switch to sheet"
                            Accessible.selected: selectedTab

                            Label {
                                id: sheetLabel
                                anchors.centerIn: parent
                                text: backend.sheetLabel(sheetTab.index)
                                textFormat: Text.PlainText
                                color: sheetTab.selectedTab ? window.accentColor : window.mutedColor
                                font.family: "monospace"
                                font.pixelSize: 11
                                font.bold: sheetTab.selectedTab
                            }

                            Rectangle {
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.bottom: parent.bottom
                                height: 2
                                color: sheetTab.selectedTab ? window.accentColor : "transparent"
                            }

                            TapHandler {
                                acceptedButtons: Qt.LeftButton
                                onTapped: grid.switchSheet(sheetTab.index)
                            }
                        }
                    }
                }
            }

            Label {
                id:statsLabel
                anchors.right:helpButton.left
                anchors.rightMargin:12
                anchors.verticalCenter:parent.verticalCenter
                width:Math.min(260,window.width/3)
                horizontalAlignment:Text.AlignRight
                elide:Text.ElideRight
                color:window.mutedColor
                font.pixelSize:11
                text:window.stats.numeric_count>0 ? "Sum "+(window.stats.sum===null ? "out of range" : Number(window.stats.sum).toLocaleString(Qt.locale(),"g",8))+" · Avg "+(window.stats.average===null ? "out of range" : Number(window.stats.average).toLocaleString(Qt.locale(),"g",6))+" · Count "+window.stats.count
                    : window.stats.count ? "Count "+window.stats.count : ""
            }
            Button {
                id: helpButton
                anchors.right: parent.right
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                height: parent.height - 4
                text: "F1 Help"
                focusPolicy: Qt.NoFocus
                Accessible.name: "Keyboard help (F1)"
                onClicked: keyboardHelp.open()
            }
        }
    }

    property var stats: ({})
    Timer {
        id: selectionStats
        interval: 250
        onTriggered: {
            if(backend.documentMode && !backend.busy && !backend.benchmark && grid.selectionRows*grid.selectionColumns<=10000)
                window.stats=JSON.parse(backend.inspectRange(JSON.stringify(spreadsheetTools.selection)));
            else window.stats={};
        }
    }
    Connections {
        target:backend
        function onRevisionChanged(){selectionStats.restart();}
    }

    Component.onCompleted: {
        if (backend.captureReview.length > 0) proposalReview.open();
        else if (backend.capturePath.length > 0 && backend.documentMode) window.tourVisible = true;
        if (backend.homeMode) newWorkbookButton.forceActiveFocus();
        else body.forceActiveFocus();
    }
    Timer {
        interval: 1200
        running: backend.capturePath.length > 0 && !backend.busy
            && (backend.captureReview.length === 0 || backend.reviewJson.length > 0)
        onTriggered: {
            const target = backend.captureReview.length > 0 ? proposalReview.contentItem
                : backend.homeMode ? welcomePane : firstSteps;
            if (!target.grabToImage(result => {
                if (!result.saveToFile(backend.capturePath)) Qt.exit(1);
                else Qt.quit();
            })) Qt.exit(1);
        }
    }
}
