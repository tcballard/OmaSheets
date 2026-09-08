pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Dialogs

Item {
    id: tools
    required property var gridModel
    required property var grid
    property var finishEditing: () => true
    readonly property bool blocked: formatDialog.visible || entryDialog.visible || findDialog.visible
        || sizeDialog.visible || sortDialog.visible || conditionalDialog.visible || chartDialog.visible || confirmDialog.visible
    readonly property var selection: ({row:grid.selectionRow,column:grid.selectionColumn,rows:grid.selectionRows,columns:grid.selectionColumns})
    readonly property var cell: { gridModel.revision; return JSON.parse(gridModel.cellPresentation(grid.currentRow,grid.currentColumn) || "{}"); }
    readonly property var style: cell.style || ({})
    readonly property var sheetView: JSON.parse(gridModel.sheetViewJson || "{}")
    property string entryKind: ""
    property var pending: ({})
    property var found: ({matches:[]})
    property var findSelection: ({})
    property string foreground: ""
    property string background: ""
    property bool pickingBackground: false

    function run(action) {
        if (!gridModel.documentMode || gridModel.busy || !finishEditing()) return false;
        return gridModel.sheetAction(JSON.stringify(action));
    }
    function format(patch) { return run({action:"format",range:selection,patch:patch}); }
    function toggle(name) { const patch={}; patch[name]=!style[name]; format(patch); }
    function fill(right) { if (finishEditing()) gridModel.fillRange(selection.row,selection.column,selection.rows,selection.columns,right); }
    function enter(kind,title,value) {
        if (!finishEditing()) return;
        entryKind=kind; entryDialog.title=title; entryText.text=value || ""; entryError.text=""; entryDialog.open();
    }
    function goTo(text) {
        const match=/^\$?([A-Za-z]+)\$?([1-9][0-9]*)(?::\$?([A-Za-z]+)\$?([1-9][0-9]*))?$/.exec(text.trim());
        if (!match) return false;
        function column(value) { let index=0; for (const character of value.toUpperCase()) index=index*26+character.charCodeAt(0)-64; return index-1; }
        const row=Number(match[2])-1, col=column(match[1]);
        const lastRow=match[4] ? Number(match[4])-1 : row, lastCol=match[3] ? column(match[3]) : col;
        if (Math.max(row,lastRow)>=gridModel.rowCount || Math.max(col,lastCol)>=gridModel.columnCount) return false;
        const hidden=sheetView.hidden_rows || [];
        if (hidden.indexOf(row)>=0 || hidden.indexOf(lastRow)>=0) {
            gridModel.operationMessage="This address is hidden by the filter. Clear the filter to go there.";
            return false;
        }
        if (!grid.selectCell(row,col)) return false;
        if (lastRow!==row || lastCol!==col) grid.selectCell(lastRow,lastCol,true);
        return true;
    }
    function showFormat() {
        if (!finishEditing()) return;
        bold.checked=!!style.bold; italic.checked=!!style.italic; underline.checked=!!style.underline; wrap.checked=!!style.wrap;
        fontSize.text=style.font_size ? String(style.font_size) : "";
        foreground=style.foreground || ""; background=style.background || "";
        alignment.currentIndex=Math.max(0,["general","left","center","right"].indexOf(style.alignment || "general"));
        border.currentIndex=Math.max(0,["none","all","bottom"].indexOf(style.border || "none"));
        numberFormat.editText=style.number_format || "General"; formatDialog.open();
    }
    function showFind() { if (finishEditing()) {findSelection=selection; found={matches:[]}; findDialog.open();} }
    function showDimensions() { if (finishEditing()) {columnWidth.text=""; rowHeight.text=""; sizeDialog.open();} }
    function showSort() { if (finishEditing()) {sortColumn.value=grid.currentColumn+1; sortDialog.open();} }
    function showConditional() { if (finishEditing()) conditionalDialog.open(); }
    function showCharts() { if (finishEditing()) chartDialog.open(); }
    function confirm(action,title,description) {
        if (!finishEditing()) return;
        pending=action; confirmDialog.title=title; confirmText.text=description+"\n\nStructural changes start a new undo history."; confirmDialog.open();
    }
    function reveal(result) {
        const hidden=sheetView.hidden_rows || [];
        if (hidden.indexOf(result.row)>=0 && !run({action:"clear_filter"})) return;
        findDialog.close(); grid.selectCell(result.row,result.column);
    }

    Dialog {
        id: entryDialog
        anchors.centerIn: parent
        width: Math.min(500,tools.width-32)
        modal: true
        standardButtons: Dialog.NoButton
        onOpened: {entryText.forceActiveFocus(); entryText.selectAll();}
        contentItem: ColumnLayout {
            TextField {id:entryText; Layout.fillWidth:true; placeholderText:tools.entryKind==="goto" ? "A1 or A1:D20" : ""; onAccepted:entryApply.clicked()}
            Label {id:entryError; Layout.fillWidth:true; wrapMode:Text.WordWrap; visible:text.length>0}
            RowLayout {
                Layout.alignment: Qt.AlignRight
                Button {text:"Cancel"; onClicked:entryDialog.close()}
                Button {
                    id:entryApply
                    text:tools.entryKind==="goto" ? "Go" : "Save"
                    onClicked: {
                        let ok=false;
                        if (tools.entryKind==="goto") ok=tools.goTo(entryText.text);
                        else if (tools.entryKind==="note") ok=tools.run({action:"note",row:tools.grid.currentRow,column:tools.grid.currentColumn,text:entryText.text});
                        else ok=tools.run({action:tools.entryKind,name:entryText.text});
                        if (ok) entryDialog.close(); else entryError.text="Check the value and try again. " + tools.gridModel.operationMessage;
                    }
                }
            }
        }
    }

    Dialog {
        id: formatDialog
        anchors.centerIn:parent
        width:Math.min(550,tools.width-32)
        title:"Format selection"
        modal:true
        standardButtons:Dialog.Ok|Dialog.Cancel
        contentItem:ColumnLayout {
            RowLayout {CheckBox {id:bold;text:"Bold"} CheckBox {id:italic;text:"Italic"} CheckBox {id:underline;text:"Underline"}}
            GridLayout {
                columns:2
                Layout.fillWidth:true
                Label {text:"Font size (points)"} TextField {id:fontSize;Layout.fillWidth:true;placeholderText:"Default";validator:DoubleValidator {bottom:6;top:72}}
                Label {text:"Alignment"} ComboBox {id:alignment;Layout.fillWidth:true;model:["General","Left","Center","Right"]}
                Label {text:"Borders"} ComboBox {id:border;Layout.fillWidth:true;model:["None","All","Bottom"]}
                Label {text:"Number format"}
                ComboBox {id:numberFormat;Layout.fillWidth:true;editable:true;model:["General","0","0.0","0.00","#,##0","#,##0.00","0%","0.0%","£#,##0.00","$#,##0.00","€#,##0.00","yyyy-mm-dd","dd/mm/yyyy","mm/dd/yyyy"]}
                Label {text:"Text colour"}
                RowLayout {
                    Button {text:tools.foreground || "Default";onClicked:{tools.pickingBackground=false;colour.selectedColor=tools.foreground || tools.gridModel.themeForeground;colour.open();}}
                    Button {text:"Reset";onClicked:tools.foreground=""}
                }
                Label {text:"Cell colour"}
                RowLayout {
                    Button {text:tools.background || "Default";onClicked:{tools.pickingBackground=true;colour.selectedColor=tools.background || tools.gridModel.themeBackground;colour.open();}}
                    Button {text:"Reset";onClicked:tools.background=""}
                }
            }
            CheckBox {id:wrap;text:"Wrap text"}
        }
        onAccepted: tools.format({bold:bold.checked,italic:italic.checked,underline:underline.checked,wrap:wrap.checked,
            font_size:fontSize.text.length ? Number(fontSize.text) : null,foreground:tools.foreground || null,background:tools.background || null,
            alignment:["general","left","center","right"][alignment.currentIndex],border:["none","all","bottom"][border.currentIndex],number_format:numberFormat.editText})
    }
    ColorDialog {
        id:colour
        title:tools.pickingBackground ? "Cell colour" : "Text colour"
        onAccepted:{if(tools.pickingBackground)tools.background=selectedColor.toString();else tools.foreground=selectedColor.toString();}
    }

    Dialog {
        id:findDialog
        anchors.centerIn:parent
        title:"Find and replace"
        width:Math.min(650,tools.width-32)
        height:Math.min(540,tools.height-32)
        modal:true
        standardButtons:Dialog.Close
        onOpened:findText.forceActiveFocus()
        contentItem:ColumnLayout {
            RowLayout {
                TextField {id:findText;Layout.fillWidth:true;placeholderText:"Find values or formulas in this sheet";onAccepted:findButton.clicked()}
                Button {id:findButton;text:"Find";enabled:findText.text.length>0;onClicked:tools.found=JSON.parse(tools.gridModel.findSheet(findText.text))}
            }
            RowLayout {
                TextField {id:replaceText;Layout.fillWidth:true;placeholderText:"Replace text cells in the selection"}
                Button {text:"Replace in selection";enabled:findText.text.length>0;onClicked:tools.run({action:"replace",range:tools.findSelection,find:findText.text,replacement:replaceText.text,case_sensitive:matchCase.checked,whole_cell:wholeCell.checked})}
            }
            RowLayout {CheckBox {id:matchCase;text:"Match case for replacement"} CheckBox {id:wholeCell;text:"Whole cell"}}
            Label {Layout.fillWidth:true;wrapMode:Text.WordWrap;text:"Replacement changes text cells only; formulas keep their meaning."}
            Label {visible:!!tools.found.truncated;text:"Showing a bounded search. Refine the text for more specific results.";wrapMode:Text.WordWrap;Layout.fillWidth:true}
            ListView {
                Layout.fillWidth:true
                Layout.fillHeight:true
                clip:true
                model:tools.found.matches || []
                delegate:ItemDelegate {
                    required property var modelData
                    width:ListView.view.width
                    text:modelData.a1+" · "+String(modelData.value.value === undefined ? "" : modelData.value.value)
                        +((tools.sheetView.hidden_rows || []).indexOf(modelData.row)>=0 ? " · Reveal and clear filter" : "")
                    onClicked:tools.reveal(modelData)
                }
            }
        }
    }

    Dialog {
        id:sizeDialog
        anchors.centerIn:parent
        title:"Selection dimensions"
        width:Math.min(430,tools.width-32)
        modal:true
        standardButtons:Dialog.Ok|Dialog.Cancel
        contentItem:GridLayout {
            columns:2
            Label {text:"Column width"} TextField {id:columnWidth;placeholderText:"Keep current width";validator:DoubleValidator {bottom:24;top:1200}}
            Label {text:"Row height"} TextField {id:rowHeight;placeholderText:"Keep current height";validator:DoubleValidator {bottom:16;top:600}}
        }
        onAccepted:tools.run({action:"dimensions",range:tools.selection,width:columnWidth.text.length ? Number(columnWidth.text) : null,height:rowHeight.text.length ? Number(rowHeight.text) : null})
    }

    Dialog {
        id:sortDialog
        anchors.centerIn:parent
        width:Math.min(480,tools.width-32)
        title:"Sort selected rows"
        modal:true
        standardButtons:Dialog.Ok|Dialog.Cancel
        contentItem:ColumnLayout {
            Label {Layout.fillWidth:true;text:"Moves entire rows, including cells outside the selected columns. Formulas and formatting follow each row.";wrapMode:Text.WordWrap}
            RowLayout {Label {text:"Column"} SpinBox {id:sortColumn;from:tools.selection.column+1;to:tools.selection.column+tools.selection.columns;editable:true} Label {text:tools.gridModel.columnLabel(sortColumn.value-1)}}
            CheckBox {id:sortHeader;text:"Selection includes a header row";checked:true}
            CheckBox {id:descending;text:"Descending"}
        }
        onAccepted:tools.run({action:"sort",range:tools.selection,column:sortColumn.value-1,header:sortHeader.checked,descending:descending.checked})
    }

    Dialog {
        id:conditionalDialog
        anchors.centerIn:parent
        width:Math.min(450,tools.width-32)
        title:"Highlight values"
        modal:true
        standardButtons:Dialog.Ok|Dialog.Cancel
        contentItem:ColumnLayout {
            ComboBox {id:comparison;model:["Greater than","Less than","Equal to"]}
            TextField {id:threshold;Layout.fillWidth:true;text:"0";validator:DoubleValidator {}}
            Label {text:"Matching numeric cells use the theme’s highlight colour.";Layout.fillWidth:true;wrapMode:Text.WordWrap}
        }
        onAccepted:tools.run({action:"conditional",range:tools.selection,comparison:["greater","less","equal"][comparison.currentIndex],value:Number(threshold.text),patch:{background:tools.gridModel.themeYellow,foreground:tools.gridModel.themeBackground}})
    }

    Dialog {
        id:chartDialog
        anchors.centerIn:parent
        width:Math.min(920,tools.width-32)
        height:Math.min(660,tools.height-32)
        title:"Charts"
        modal:true
        standardButtons:Dialog.Close
        contentItem:ColumnLayout {
            RowLayout {
                TextField {id:chartTitle;Layout.fillWidth:true;placeholderText:"Chart title"}
                ComboBox {id:chartKind;model:["Bar","Line","Pie"]}
                Button {text:"Create from selection";enabled:chartTitle.text.trim().length>0;onClicked:tools.run({action:"chart",range:tools.selection,title:chartTitle.text,kind:["bar","line","pie"][chartKind.currentIndex]})}
            }
            Label {Layout.fillWidth:true;wrapMode:Text.WordWrap;text:"Use a header row, categories in the first column and numeric series in the following columns (up to 1,000 cells)."}
            RowLayout {
                ComboBox {id:charts;Layout.fillWidth:true;model:tools.sheetView.charts || [];textRole:"title"}
                Button {text:"Remove chart";enabled:charts.currentIndex>=0;onClicked:tools.run({action:"remove_chart",id:tools.sheetView.charts[charts.currentIndex].id})}
            }
            ChartView {
                Layout.fillWidth:true
                Layout.fillHeight:true
                chart:charts.currentIndex>=0 ? tools.sheetView.charts[charts.currentIndex] : null
                textColor:tools.gridModel.themeForeground
                backgroundColor:tools.gridModel.themeBackground
                colors:[tools.gridModel.themeBlue,tools.gridModel.themeGreen,tools.gridModel.themeMagenta,tools.gridModel.themeYellow,tools.gridModel.themeRed]
            }
        }
    }

    Dialog {
        id:confirmDialog
        anchors.centerIn:parent
        width:Math.min(470,tools.width-32)
        modal:true
        standardButtons:Dialog.Ok|Dialog.Cancel
        contentItem:Label {id:confirmText;wrapMode:Text.WordWrap;textFormat:Text.PlainText}
        onAccepted:tools.run(tools.pending)
    }
}
