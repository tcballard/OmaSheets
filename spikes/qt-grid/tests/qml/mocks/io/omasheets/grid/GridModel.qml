// UI boundary fixture: the production QML runs unchanged. No disk writes.
import QtQuick

QtObject {
    id: model
    objectName: "testGridModel"
    property int rowCount: 16
    property int columnCount: 8
    property int revision: 0
    property bool benchmark: false
    property bool documentMode: true
    property bool homeMode: false
    property bool busy: false
    property string sheetViewJson: "{}"
    property string reviewJson: ""
    property string proposalsJson: "[]"
    property string captureReview: ""
    property string capturePanel: ""
    property string capturePath: ""
    property bool tourSeen: false
    property bool packageManaged: false
    property string documentPath: ""
    property string operationMessage: ""
    property int documentGeneration: 0
    property string documentName: "Keyboard test"
    property string sheetName: "Forecast"
    property int sheetCount: 2
    property int currentSheet: 0
    property string sourceStatus: ""
    property string themeName: ""
    property string themeBackground: "#101216"
    property string themeForeground: "#f0f1f2"
    property string themeAccent: "#ffaa00"
    property string themeMuted: "#667788"
    property string themeRed: "#ff6677"
    property string themeGreen: "#77cc88"
    property string themeYellow: "#e8c060"
    property string themeBlue: "#70a5ff"
    property string themeMagenta: "#c58cff"
    property bool failWrites: false
    property var actions: []
    property var values: ({"2:1":"120"})
    property var style: ({})
    function sheetAction(text) {
        const action=JSON.parse(text);
        actions=actions.concat([action]);
        if(action.action==="format")style=Object.assign({},style,action.patch);
        revision++;
        return true;
    }
    function inspectRange(range) { return "{}"; }
    function findSheet(text) { return JSON.stringify({matches:[{row:2,column:1,a1:"B3",value:{type:"number",value:120}}]}); }
    function fillRange(row,column,rows,columns,right) {actions=actions.concat([{action:"fill",right:right}]);return true;}
    function cellPresentation(row,column) {return JSON.stringify({style:style});}
    function selectedCellPresentation(row,column) {return JSON.stringify({style:style,raw_display:cellText(row,column)});}
    function cellInput(row,column) {return values[row+":"+column] || "";}
    function cellText(row,column) {return cellInput(row,column);}
    function cellPreview(row,column) {return cellInput(row,column);}
    function cellKind(row,column) {return cellInput(row,column).indexOf("=")===0 ? "formula" : "text";}
    function columnLabel(column) {return String.fromCharCode(65+column);}
    function sheetLabel(index) {return index===0 ? "Forecast" : "Operations";}
    function selectSheet(index) {currentSheet=index;sheetName=sheetLabel(index);}
    function prepareCellEdit(row,column) {return true;}
    function setCellText(row,column,text) {
        if(failWrites)return false;
        const next=Object.assign({},values);next[row+":"+column]=text;values=next;
        revision++;
        actions=actions.concat([{action:"edit",text:text}]);
        return true;
    }
    function copyRange(row,column,rows,columns) {actions=actions.concat([{action:"copy"}]);return true;}
    function pasteCells(row,column,text) {actions=actions.concat([{action:"paste",text:text}]);return true;}
    function undoEdit(redo) {actions=actions.concat([{action:"undo",redo:redo}]);return true;}
    function clearCells(row,column,rows,columns) {actions=actions.concat([{action:"clear"}]);return true;}
    function refreshTheme() {}
    function reportBenchmark() {}
    function finishTour() {}
    function askAgent() {actions=actions.concat([{action:"ask"}]);}
    function listProposals() {}
    function reviewProposal(branch) {}
    function resolveProposal(approve) {}
    function createExample(url) {}
    function openDocument(url,create) {}
    function openUpdater() {return false;}
    function importDocument(source,output) {}
    function exportDocument(output,format) {}
    function openCompatibility(url) {}
    function captureWindow() {return false;}
}
