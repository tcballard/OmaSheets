import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Frame {
    id: guide
    property int step: 0
    signal finished()
    signal selectCell(int row, int column)
    readonly property var pages: [
        { title: "1. Change a number", body: "Select B2, type 3, then press Enter. The train-ticket total recalculates. Each finished cell edit saves to your practice file.", row: 1, column: 1 },
        { title: "2. See the formula", body: "Select D2 and press F2. Its formula is =B2*C2. Press Escape to leave it unchanged. Formula results update when their inputs change.", row: 1, column: 3 },
        { title: "3. Undo and try again", body: "Outside the cell editor, Ctrl+Z undoes your last edit. Ctrl+Shift+Z restores it. Use arrow keys to move and Shift+arrows to select a range.", row: 1, column: 1 },
        { title: "4. Share your work", body: "Press Ctrl+Space and search for Excel to export a separate copy. Read the export report for any unsupported features. F1 brings back the full keyboard guide anytime.", row: 4, column: 3 }
    ]
    width: Math.min(420, parent.width - 32)
    background: Rectangle { color: window.panelColor; border.color: window.accentColor; radius: 8 }
    contentItem: ColumnLayout {
        spacing: 10
        Label { text: "FIRST STEPS · " + (guide.step + 1) + " / 4"; color: window.accentColor; font.bold: true }
        Label { text: guide.pages[guide.step].title; font.pixelSize: 18; Layout.fillWidth: true; wrapMode: Text.WordWrap }
        Label { text: guide.pages[guide.step].body; Layout.fillWidth: true; wrapMode: Text.WordWrap }
        RowLayout {
            Button { text: "Close tour"; onClicked: guide.finished() }
            Item { Layout.fillWidth: true }
            Button { text: "Back"; enabled: guide.step > 0; onClicked: { guide.step--; guide.selectCell(guide.pages[guide.step].row, guide.pages[guide.step].column); } }
            Button {
                text: guide.step === 3 ? "Done" : "Next"
                highlighted: true
                onClicked: {
                    if (guide.step === 3) guide.finished();
                    else { guide.step++; guide.selectCell(guide.pages[guide.step].row, guide.pages[guide.step].column); }
                }
            }
        }
    }
}
