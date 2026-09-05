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
        for name in ("selectCell", "beginEdit", "commitEdit", "switchSheet"):
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
let currentRow = 2, currentColumn = 3, accepted = false, saves = 0;
const editor = { visible: true, text: '=SUM(A1:A2)',
    forceActiveFocus() {}, selectAll() {} };
const body = { forceActiveFocus() {}, contentX: 100, contentY: 100 };
const backend = { rowCount: 100, columnCount: 10, sheetCount: 2,
    currentSheet: 0, cellInput() { return 'stored'; },
    setCellText(row, column, text) {
        assert.equal(row, 2); assert.equal(column, 3);
        assert.equal(text, '=SUM(A1:A2)'); saves++; return accepted;
    }, selectSheet(index) { this.currentSheet = index; } };
function ensureVisible() {}
""" + "\n".join(functions) + """
assert.equal(commitEdit(), false);
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
"""
        subprocess.run([shutil.which("node"), "-e", script], check=True)


if __name__ == "__main__":
    unittest.main()
