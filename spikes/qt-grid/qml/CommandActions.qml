pragma ComponentBehavior: Bound
import QtQuick

Item {
    id: commands
    required property var gridModel
    required property var grid
    required property var tools
    required property var files
    property bool shortcutsEnabled: true
    property bool gridHasFocus: false
    signal reviewRequested()
    signal helpRequested()
    signal updatesRequested()
    signal closeRequested()

    readonly property bool available: files.available && !gridModel.busy
    readonly property bool sheetAvailable: available && gridModel.documentMode && !gridModel.homeMode
    readonly property bool selectionAvailable: sheetAvailable && !grid.hasDraft

    // The menu and direct shortcuts share the same action and availability check.
    // IDs describe the menu tree; only these local callbacks can execute commands.
    function command(id, text, shortcut, enabled, run, detail, gridOnly) {
        return {id:id, parent:id.slice(0,id.lastIndexOf(".")), text:text,
            shortcut:shortcut, enabled:enabled, run:run, detail:detail || "", gridOnly:!!gridOnly};
    }
    function group(id, text) {
        return {id:id, parent:id.indexOf(".")<0 ? "" : id.slice(0,id.lastIndexOf(".")),
            text:text, shortcut:"", enabled:true, detail:"", group:true};
    }
    function trigger(id) {
        const entry=entries.find(item => item.id===id);
        if (!entry || !entry.enabled || entry.group || !available) return false;
        entry.run();
        return true;
    }

    readonly property var entries: [
        group("file", "Workbook"),
        group("edit", "Edit"),
        group("format", "Format"),
        group("data", "Data"),
        group("sheet", "Sheet"),
        group("view", "View"),
        group("agent", "Agent"),
        group("help", "Help"),

        command("file.new", "New workbook…", "Ctrl+N", available, () => files.newAction.trigger()),
        command("file.open", "Open workbook…", "Ctrl+O", available, () => files.openAction.trigger()),
        command("file.import", "Import Excel workbook…", "", available, () => files.importAction.trigger()),
        command("file.compatibility", "Open Excel or OpenDocument…", "", available, () => files.compatibilityAction.trigger(), "Compatibility window for XLS, XLSX, XLSM and ODS"),
        command("file.save", "Save cell draft", "Ctrl+S", sheetAvailable, () => grid.commitEdit(), "Edits are saved on this computer"),
        group("file.export", "Export a copy"),
        command("file.export.xlsx", "Excel workbook…", "", sheetAvailable, () => files.xlsxAction.trigger(), "XLSX"),
        command("file.export.csv", "Current sheet as CSV…", "", sheetAvailable, () => files.csvAction.trigger()),
        command("file.export.parquet", "Current sheet as Parquet…", "", sheetAvailable, () => files.parquetAction.trigger()),
        command("file.close", "Close window", "Ctrl+W", available, () => commands.closeRequested()),

        command("edit.undo", "Undo", "Ctrl+Z", selectionAvailable, () => grid.undoSelection(false), "", true),
        command("edit.redo", "Redo", "Ctrl+Shift+Z", selectionAvailable, () => grid.undoSelection(true), "Ctrl+Y also works in the grid", true),
        command("edit.copy", "Copy selection", "Ctrl+C", selectionAvailable, () => grid.copySelection(), "", true),
        command("edit.paste", "Paste", "Ctrl+V", selectionAvailable, () => grid.pasteSelection(), "", true),
        command("edit.clear", "Clear selected cells", "", selectionAvailable, () => grid.clearCell(), "Delete or Backspace in the grid"),
        command("edit.fill-down", "Fill down", "Ctrl+D", sheetAvailable, () => tools.fill(false)),
        command("edit.fill-right", "Fill right", "Ctrl+R", sheetAvailable, () => tools.fill(true)),
        command("edit.find", "Find and replace…", "Ctrl+F", sheetAvailable, () => tools.showFind()),
        command("edit.goto", "Go to cell or range…", "Ctrl+G", sheetAvailable, () => tools.enter("goto","Go to cell or range",""), "Select an address such as A1 or A1:D20"),
        command("edit.cell", "Edit selected cell", "", sheetAvailable, () => grid.beginEdit(), "Enter or F2 in the grid"),

        command("format.cells", "Format cells…", "Ctrl+1", sheetAvailable, () => tools.showFormat(), "Font size, colours, alignment, borders and number formats"),
        command("format.bold", "Bold", "Ctrl+B", sheetAvailable, () => tools.toggle("bold")),
        command("format.italic", "Italic", "Ctrl+I", sheetAvailable, () => tools.toggle("italic")),
        command("format.underline", "Underline", "Ctrl+U", sheetAvailable, () => tools.toggle("underline")),
        command("format.wrap", "Wrap text", "", sheetAvailable, () => tools.toggle("wrap")),
        group("format.number", "Number format"),
        command("format.number.general", "General", "", sheetAvailable, () => tools.format({number_format:"General"})),
        command("format.number.decimal", "Number with two decimals", "", sheetAvailable, () => tools.format({number_format:"#,##0.00"})),
        command("format.number.percent", "Percent", "", sheetAvailable, () => tools.format({number_format:"0.0%"})),
        command("format.number.currency", "Currency (pounds)", "", sheetAvailable, () => tools.format({number_format:"£#,##0.00"})),
        command("format.number.date", "Date (YYYY-MM-DD)", "", sheetAvailable, () => tools.format({number_format:"yyyy-mm-dd"})),
        group("format.align", "Alignment"),
        command("format.align.left", "Align left", "", sheetAvailable, () => tools.format({alignment:"left"})),
        command("format.align.center", "Align centre", "", sheetAvailable, () => tools.format({alignment:"center"})),
        command("format.align.right", "Align right", "", sheetAvailable, () => tools.format({alignment:"right"})),
        command("format.clear", "Clear formatting", "", sheetAvailable, () => tools.run({action:"clear_format",range:tools.selection})),
        command("format.note", "Edit note…", "", sheetAvailable, () => tools.showNote()),
        command("format.dimensions", "Row height and column width…", "", sheetAvailable, () => tools.showDimensions(), "Resize the selection"),
        command("format.autofit", "Autofit selected columns", "", sheetAvailable, () => tools.run({action:"dimensions",range:tools.selection,autofit:true,width:null,height:null})),
        command("format.merge", "Merge selection", "", sheetAvailable, () => tools.run({action:"merge",range:tools.selection})),
        command("format.unmerge", "Unmerge selection", "", sheetAvailable, () => tools.run({action:"merge",range:tools.selection,unmerge:true})),

        command("data.sort", "Sort selected rows…", "", sheetAvailable, () => tools.showSort()),
        command("data.filter", "Filter by current cell", "", sheetAvailable, () => tools.filterCurrent(), "Filter the selection using the current value"),
        command("data.clear-filter", "Clear filter", "", sheetAvailable && !!tools.sheetView.filter_active, () => tools.run({action:"clear_filter"})),
        command("data.deduplicate", "Remove duplicate rows…", "", sheetAvailable, () => tools.confirm({action:"deduplicate",range:tools.selection,header:true},"Remove duplicate rows?","Keeps the first selected row as a header and removes later duplicate rows, including their cells outside the selection.")),
        command("data.highlight", "Highlight values…", "", sheetAvailable, () => tools.showConditional(), "Conditional formatting"),
        command("data.clear-conditional", "Clear conditional formatting", "", sheetAvailable, () => tools.run({action:"clear_conditional"})),
        command("data.charts", "Charts…", "", sheetAvailable, () => tools.showCharts(), "Create or remove bar, line and pie charts"),

        command("sheet.add", "Add sheet…", "", sheetAvailable, () => tools.enter("add_sheet","New sheet","")),
        command("sheet.rename", "Rename sheet…", "", sheetAvailable, () => tools.enter("rename_sheet","Rename sheet",gridModel.sheetName)),
        command("sheet.duplicate", "Duplicate sheet…", "", sheetAvailable, () => tools.enter("duplicate_sheet","Duplicate sheet",gridModel.sheetName+" copy")),
        command("sheet.delete", "Delete sheet…", "", sheetAvailable, () => tools.confirm({action:"delete_sheet"},"Delete sheet?","Delete “"+gridModel.sheetName+"” and all of its cells?")),
        command("sheet.insert-rows", "Insert rows above", "", sheetAvailable, () => tools.run({action:"insert_rows",at:grid.selectionRow,count:grid.selectionRows}), "Insert the selected number of rows"),
        command("sheet.insert-columns", "Insert columns before", "", sheetAvailable, () => tools.run({action:"insert_columns",at:grid.selectionColumn,count:grid.selectionColumns}), "Insert the selected number of columns"),
        command("sheet.delete-rows", "Delete selected rows…", "", sheetAvailable, () => tools.confirm({action:"delete_rows",at:grid.selectionRow,count:grid.selectionRows},"Delete rows?","Delete "+grid.selectionRows+" entire rows starting at row "+(grid.selectionRow+1)+"?")),
        command("sheet.delete-columns", "Delete selected columns…", "", sheetAvailable, () => tools.confirm({action:"delete_columns",at:grid.selectionColumn,count:grid.selectionColumns},"Delete columns?","Delete "+grid.selectionColumns+" entire columns starting at "+gridModel.columnLabel(grid.selectionColumn)+"?")),
        command("sheet.previous", "Previous sheet", "Ctrl+PgUp", sheetAvailable && gridModel.currentSheet>0, () => grid.switchSheet(gridModel.currentSheet-1)),
        command("sheet.next", "Next sheet", "Ctrl+PgDown", sheetAvailable && gridModel.currentSheet+1<gridModel.sheetCount, () => grid.switchSheet(gridModel.currentSheet+1)),

        group("view.freeze", "Freeze panes"),
        command("view.freeze.row", "Freeze top row", "", sheetAvailable, () => tools.run({action:"freeze",rows:1,columns:0})),
        command("view.freeze.column", "Freeze first column", "", sheetAvailable, () => tools.run({action:"freeze",rows:0,columns:1})),
        command("view.freeze.selection", "Freeze at current cell", "", sheetAvailable, () => tools.run({action:"freeze",rows:grid.currentRow,columns:grid.currentColumn}), "Freeze rows above and columns before the cell"),
        command("view.freeze.clear", "Unfreeze panes", "", sheetAvailable, () => tools.run({action:"freeze",rows:0,columns:0})),
        command("view.gridlines", tools.sheetView.show_grid_lines===false ? "Show gridlines" : "Hide gridlines", "", sheetAvailable, () => tools.run({action:"grid_lines",visible:tools.sheetView.show_grid_lines===false})),

        command("agent.ask", "Ask Agent", "Ctrl+Shift+A", sheetAvailable, () => {if(grid.commitEdit())gridModel.askAgent(grid.selectionRow,grid.selectionColumn,grid.selectionRows,grid.selectionColumns);}),
        command("agent.review", "Review proposals…", "Ctrl+Shift+R", sheetAvailable, () => {if(grid.commitEdit())commands.reviewRequested();}, "Inspect changes, approve or reject"),
        command("help.example", "Try an example…", "", available, () => files.exampleAction.trigger()),
        command("help.keyboard", "Keyboard help", "F1", available, () => commands.helpRequested()),
        command("help.updates", "Updates…", "", available, () => {if(gridModel.homeMode || grid.commitEdit())commands.updatesRequested();})
    ]

    Repeater {
        model: commands.entries.filter(entry => entry.shortcut.length>0)
        delegate: Item {
            id: binding
            required property var modelData
            Shortcut {
                sequence: binding.modelData.shortcut
                enabled: commands.shortcutsEnabled && binding.modelData.enabled
                    && (!binding.modelData.gridOnly || commands.gridHasFocus)
                onActivated: commands.trigger(binding.modelData.id)
            }
        }
    }
}
