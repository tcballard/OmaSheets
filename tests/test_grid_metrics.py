"""Exercise the viewport geometry used by QML over sparse and hidden axes."""
import shutil
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("node"), "JavaScript runtime unavailable")
class GridMetricsTests(unittest.TestCase):
    def test_sparse_dimensions_hidden_rows_freeze_and_merge_anchors(self):
        qml = (ROOT / "spikes/qt-grid/qml/GridMetrics.qml").read_text()
        functions = []
        offset = 0
        while (start := qml.find("function ", offset)) >= 0:
            opening = qml.index("{", start)
            depth, end = 1, opening + 1
            while depth:
                depth += (qml[end] == "{") - (qml[end] == "}")
                end += 1
            functions.append(qml[start:end])
            offset = end
        script = """
const assert=require('node:assert/strict');
let view={};const rowCount=1000000,columnCount=64,defaultRowHeight=27,defaultColumnWidth=132;
let rows,columns,frozenRows,frozenColumns;
function refresh() {
 rows=axis(rowCount,defaultRowHeight,view.row_heights||[],view.hidden_rows||[]);
 columns=axis(columnCount,defaultColumnWidth,view.column_widths||[],[]);
 frozenRows=view.frozen_rows||0;frozenColumns=view.frozen_columns||0;
}
""" + "\n".join(functions) + """
refresh();assert.equal(rowPosition(1000000),27000000);
assert.equal(indexAt(rows,27),1);assert.equal(indexAt(rows,26.9),0);
view={row_heights:[[2,54]],column_widths:[[0,200]],hidden_rows:[1,3],frozen_rows:3,frozen_columns:1};
refresh();assert.equal(rowPosition(2),27);assert.equal(rowPosition(4),81);
assert.equal(indexAt(rows,27),2);assert.equal(indexAt(rows,81),4);
assert.equal(columnPosition(2),332);
assert.deepEqual(visibleRows(270,200).slice(0,2),[0,2]);
assert.equal(screenRow(2,270),27);assert.equal(screenColumn(0,132),0);
view={hidden_rows:Array.from({length:50000},(_,i)=>i+1)};refresh();
const shownRows=visibleRows(0,270);
assert.equal(shownRows[0],0);assert.equal(shownRows[1],50001);assert.ok(shownRows.length<20);
view={merges:[{row:0,column:0,rows:2,columns:3}]};refresh();
const rendered=cells(visibleRows(27,100),visibleColumns(132,200),132,27,200,100);
assert.ok(rendered.some(cell=>cell.row===0 && cell.column===0));
assert.equal(new Set(rendered.map(cell=>cell.row+':'+cell.column)).size,rendered.length);
assert.ok(visibleRows(100000,1000000).length<=128);
assert.ok(visibleColumns(0,1000000).length<=80);
"""
        subprocess.run([shutil.which("node"), "-e", script], check=True)


if __name__ == "__main__":
    unittest.main()
