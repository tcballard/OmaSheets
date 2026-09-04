import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window
import io.omasheets.grid 1.0

ApplicationWindow {
    id: window

    width: 1240
    height: 760
    minimumWidth: 760
    minimumHeight: 480
    visible: true
    title: backend.documentName + " — OmaSheets"
    color: palette.window

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

    GridModel {
        id: backend
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
                    text: backend.documentName
                    color: window.textColor
                    font.pixelSize: 14
                    font.weight: Font.DemiBold
                }

                Label {
                    text: backend.rowCount.toLocaleString(Qt.locale("en_US"), "f", 0)
                        + " rows  ·  " + backend.columnCount + " columns  ·  " + backend.sheetName
                    color: window.mutedColor
                    font.family: "monospace"
                    font.pixelSize: 11
                }

                Item { Layout.fillWidth: true }

                Label {
                    text: (backend.documentMode ? "LOCAL SERVICE" : "GRID SPIKE")
                        + "  ·  " + backend.themeName.toUpperCase()
                    color: window.mutedColor
                    font.family: "monospace"
                    font.pixelSize: 10
                    font.letterSpacing: 1.2
                }
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

                Label {
                    Layout.preferredWidth: 78
                    Layout.fillHeight: true
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    text: backend.columnLabel(grid.currentColumn) + (grid.currentRow + 1)
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

                Label {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    leftPadding: 4
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                    text: {
                        backend.revision;
                        return backend.cellInput(grid.currentRow, grid.currentColumn);
                    }
                    color: window.textColor
                    font.family: "monospace"
                    font.pixelSize: 12
                }

                Label {
                    Layout.preferredWidth: 320
                    Layout.fillHeight: true
                    rightPadding: 12
                    horizontalAlignment: Text.AlignRight
                    verticalAlignment: Text.AlignVCenter
                    text: backend.sourceStatus
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
            readonly property int rowCapacity: Math.min(backend.rowCount,
                Math.ceil(Math.max(1, body.height) / window.rowHeight) + 2)
            readonly property int columnCapacity: Math.min(backend.columnCount,
                Math.ceil(Math.max(1, body.width) / window.cellWidth) + 2)
            readonly property int firstVisibleRow: Math.min(backend.rowCount - rowCapacity,
                Math.max(0, Math.floor(body.contentY / window.rowHeight)))
            readonly property int firstVisibleColumn: Math.min(backend.columnCount - columnCapacity,
                Math.max(0, Math.floor(body.contentX / window.cellWidth)))
            readonly property int visibleRowCount: rowCapacity
            readonly property int visibleColumnCount: columnCapacity
            readonly property int visibleDelegates: Math.max(0, visibleRowCount * visibleColumnCount)

            function selectCell(row, column) {
                currentRow = Math.max(0, Math.min(backend.rowCount - 1, row));
                currentColumn = Math.max(0, Math.min(backend.columnCount - 1, column));
                ensureVisible();
                body.forceActiveFocus();
            }

            function ensureVisible() {
                const left = currentColumn * window.cellWidth;
                const right = left + window.cellWidth;
                const top = currentRow * window.rowHeight;
                const bottom = top + window.rowHeight;
                if (left < body.contentX)
                    body.contentX = left;
                else if (right > body.contentX + body.width)
                    body.contentX = right - body.width;
                if (top < body.contentY)
                    body.contentY = top;
                else if (bottom > body.contentY + body.height)
                    body.contentY = bottom - body.height;
            }

            function beginEdit() {
                editor.text = backend.cellInput(currentRow, currentColumn);
                editor.visible = true;
                editor.forceActiveFocus();
                editor.selectAll();
            }

            function commitEdit() {
                if (!editor.visible)
                    return;
                backend.setCellText(currentRow, currentColumn, editor.text);
                editor.visible = false;
                body.forceActiveFocus();
            }

            function switchSheet(index) {
                if (index < 0 || index >= backend.sheetCount || index === backend.currentSheet)
                    return;
                editor.visible = false;
                backend.selectSheet(index);
                currentRow = 0;
                currentColumn = 0;
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
                        readonly property int logicalColumn: grid.firstVisibleColumn + index

                        x: logicalColumn * window.cellWidth - body.contentX
                        width: window.cellWidth
                        height: window.columnHeaderHeight
                        color: logicalColumn === grid.currentColumn
                            ? window.selectedHeaderColor : window.headerColor
                        border.color: window.gridLineColor

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
                        readonly property int logicalRow: grid.firstVisibleRow + index

                        y: logicalRow * window.rowHeight - body.contentY
                        width: window.rowHeaderWidth
                        height: window.rowHeight
                        color: logicalRow === grid.currentRow
                            ? window.selectedHeaderColor : window.headerColor
                        border.color: window.gridLineColor

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
                contentWidth: backend.columnCount * window.cellWidth
                contentHeight: backend.rowCount * window.rowHeight
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
                            readonly property int rowOffset: Math.floor(index / grid.visibleColumnCount)
                            readonly property int columnOffset: index % grid.visibleColumnCount
                            readonly property int logicalRow: grid.firstVisibleRow + rowOffset
                            readonly property int logicalColumn: grid.firstVisibleColumn + columnOffset
                            readonly property bool selectedCell: logicalRow === grid.currentRow
                                && logicalColumn === grid.currentColumn

                            x: logicalColumn * window.cellWidth
                            y: logicalRow * window.rowHeight
                            width: window.cellWidth
                            height: window.rowHeight
                            color: selectedCell ? window.selectedCellColor
                                : (logicalRow % 2 === 0 ? window.canvasColor : window.alternateRowColor)
                            border.width: 1
                            border.color: selectedCell ? window.accentColor : window.gridLineColor

                            Accessible.role: Accessible.StaticText
                            Accessible.name: backend.columnLabel(logicalColumn) + (logicalRow + 1)
                            Accessible.description: "Spreadsheet cell, " + backend.cellKind(logicalRow, logicalColumn)
                                + ", value " + valueLabel.text
                            Accessible.focusable: true
                            Accessible.focused: selectedCell && body.activeFocus
                            Accessible.selected: selectedCell

                            Label {
                                id: valueLabel
                                anchors.fill: parent
                                leftPadding: 7
                                rightPadding: 7
                                verticalAlignment: Text.AlignVCenter
                                horizontalAlignment: backend.cellKind(parent.logicalRow, parent.logicalColumn) === "number"
                                    ? Text.AlignRight : Text.AlignLeft
                                elide: Text.ElideRight
                                text: {
                                    backend.revision;
                                    return backend.cellText(parent.logicalRow, parent.logicalColumn);
                                }
                                color: backend.cellKind(parent.logicalRow, parent.logicalColumn) === "formula"
                                    ? window.formulaColor
                                    : (parent.logicalColumn % 6 === 5 && text === "Reviewed"
                                        ? window.successColor : window.textColor)
                                font.family: "monospace"
                                font.pixelSize: 11
                            }

                            TapHandler {
                                acceptedButtons: Qt.LeftButton
                                onTapped: grid.selectCell(cell.logicalRow, cell.logicalColumn)
                                onDoubleTapped: {
                                    grid.selectCell(cell.logicalRow, cell.logicalColumn);
                                    grid.beginEdit();
                                }
                            }
                        }
                    }

                    TextField {
                        id: editor

                        x: grid.currentColumn * window.cellWidth
                        y: grid.currentRow * window.rowHeight
                        width: window.cellWidth
                        height: window.rowHeight
                        visible: false
                        z: 10
                        leftPadding: 6
                        selectByMouse: true
                        font.family: "monospace"
                        font.pixelSize: 11

                        Accessible.role: Accessible.EditableText
                        Accessible.name: "Edit " + backend.columnLabel(grid.currentColumn) + (grid.currentRow + 1)
                        Accessible.description: "Type a new cell value. Enter commits and Escape cancels."

                        onAccepted: grid.commitEdit()
                        Keys.onEscapePressed: {
                            visible = false;
                            body.forceActiveFocus();
                        }
                    }
                }

                Keys.onPressed: event => {
                    const control = (event.modifiers & Qt.ControlModifier) !== 0;
                    if (event.key === Qt.Key_PageUp && control)
                        grid.switchSheet(backend.currentSheet - 1);
                    else if (event.key === Qt.Key_PageDown && control)
                        grid.switchSheet(backend.currentSheet + 1);
                    else if (event.key === Qt.Key_Left)
                        grid.selectCell(grid.currentRow, grid.currentColumn - 1);
                    else if (event.key === Qt.Key_Right)
                        grid.selectCell(grid.currentRow, grid.currentColumn + 1);
                    else if (event.key === Qt.Key_Up)
                        grid.selectCell(grid.currentRow - 1, grid.currentColumn);
                    else if (event.key === Qt.Key_Down)
                        grid.selectCell(grid.currentRow + 1, grid.currentColumn);
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
                        backend.setCellText(0, 0, "7");
                        backend.setCellText(0, 1, "=A1*3");
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
                anchors.rightMargin: 8
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
        }
    }

    Component.onCompleted: body.forceActiveFocus()
}
