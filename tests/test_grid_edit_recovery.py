"""Exercise the actual QML edit functions with a rejecting service boundary."""

import shutil
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("node"), "JavaScript runtime unavailable")
class GridEditRecoveryTests(unittest.TestCase):
    def test_failed_save_preserves_draft_and_selection(self):
        qml = (ROOT / "spikes/qt-grid/qml/Main.qml").read_text()
        functions = []
        for name in ("selectCell", "beginEdit", "commitEdit", "finishEdit", "clearCell", "switchSheet",
                     "copySelection", "pasteSelection", "undoSelection"):
            start = qml.index("function " + name + "(")
            opening = qml.index("{", start)
            depth = 1
            end = opening + 1
            while depth:
                depth += (qml[end] == "{") - (qml[end] == "}")
                end += 1
            functions.append(qml[start:end])
        script = """
const assert = require('node:assert/strict');
let currentRow = 2, currentColumn = 3, accepted = false, saves = 0, readable = true;
let anchorRow = 2, anchorColumn = 3;
for (const [name, getter] of Object.entries({
    selectionRow: () => Math.min(currentRow, anchorRow),
    selectionColumn: () => Math.min(currentColumn, anchorColumn),
    selectionRows: () => Math.abs(currentRow - anchorRow) + 1,
    selectionColumns: () => Math.abs(currentColumn - anchorColumn) + 1
})) Object.defineProperty(globalThis, name, {get: getter});
let systemClipboard = 'original';
const clipboardBuffer = {text: '', selectAll() {}, copy() { systemClipboard = this.text; },
    paste() { this.text = systemClipboard; }};
const editor = { visible: true, text: '=SUM(A1:A2)',
    forceActiveFocus() {}, selectAll() {} };
const body = { forceActiveFocus() {}, contentX: 100, contentY: 100 };
const backend = { rowCount: 100, columnCount: 10, sheetCount: 2,
    prepareCellEdit() { return readable; },
    currentSheet: 0, cellInput() { return 'stored'; },
    setCellText(row, column, text) {
        assert.equal(row, 2); assert.equal(column, 3);
        assert.equal(text, '=SUM(A1:A2)'); saves++; return accepted;
    }, selectSheet(index) { this.currentSheet = index; } };
function ensureVisible() {}
""" + "\n".join(functions) + """
assert.equal(commitEdit(), false);
assert.equal(editor.visible, true);
finishEdit(0, 1);
assert.equal(currentColumn, 3);
assert.equal(editor.visible, true);
assert.equal(selectCell(9, 5), false);
assert.equal(currentRow, 2); assert.equal(currentColumn, 3);
switchSheet(1);
assert.equal(backend.currentSheet, 0);
assert.equal(body.contentX, 100);
beginEdit();
assert.equal(editor.text, '=SUM(A1:A2)');
accepted = true;
assert.equal(commitEdit(), true);
assert.equal(editor.visible, false);
const saved = saves;
assert.equal(commitEdit(), true);
assert.equal(saves, saved);
assert.equal(selectCell(9, 5), true);
assert.equal(currentRow, 9);
switchSheet(1);
assert.equal(backend.currentSheet, 1);
assert.equal(currentRow, 0); assert.equal(body.contentX, 0);
// Opening and leaving an unchanged cell never appends an event.
beginEdit();
assert.equal(editor.text, 'stored');
finishEdit(0, 1);
assert.equal(saves, saved);
assert.equal(currentColumn, 1);
// Failed initial reads cannot seed an editor with an error placeholder.
readable = false;
beginEdit();
assert.equal(editor.visible, false);
readable = true;
beginEdit('x');
assert.equal(editor.text, 'x');
assert.equal(editor.originalText, 'stored');
assert.equal(editor.cursorPosition, 1);
// A second key/start action cannot replace an existing unsaved draft.
beginEdit('y');
assert.equal(editor.text, 'x');
clearCell();
assert.equal(editor.text, 'x');
editor.visible = false;
backend.setCellText = (row, column, text) => {
    assert.equal(text, ''); saves++; return false;
};
clearCell();
assert.equal(editor.visible, true);
assert.equal(editor.text, '');
assert.equal(editor.originalText, 'stored');
assert.equal(saves, saved + 1);
// Clipboard actions cannot replace a draft. Failed copies keep clipboard data.
backend.copyRange = () => 'null';
copySelection();
assert.equal(systemClipboard, 'original');
editor.visible = false;
selectCell(1, 1);
selectCell(3, 2, true);
assert.equal(selectionRows, 3); assert.equal(selectionColumns, 2);
copySelection();
assert.equal(systemClipboard, 'original');
backend.copyRange = (r,c,rows,cols) => {
    assert.deepEqual([r,c,rows,cols], [1,1,3,2]);
    return JSON.stringify('a\\tb\\nc\\td');
};
copySelection();
assert.equal(systemClipboard, 'a\\tb\\nc\\td');
backend.pasteCells = (r,c,text) => {
    assert.deepEqual([r,c,text], [1,1,systemClipboard]); return false;
};
pasteSelection();
assert.equal(currentRow, 3); assert.equal(selectionRows, 3);
backend.pasteCells = () => true;
pasteSelection();
assert.equal(currentRow, 1); assert.equal(selectionRows, 1);
let undos = [];
backend.undoEdit = redo => { undos.push(redo); return true; };
undoSelection(false); undoSelection(true);
assert.deepEqual(undos, [false,true]);
editor.visible = true;
undoSelection(false);
assert.deepEqual(undos, [false,true]);
"""
        subprocess.run([shutil.which("node"), "-e", script], check=True)


if __name__ == "__main__":
    unittest.main()
