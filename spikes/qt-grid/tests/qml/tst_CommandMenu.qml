import QtQuick
import QtTest

Item {
    width: 20
    height: 20
    Loader {id: application;source:"../../qml/Main.qml"}
    TestCase {
        name: "SpreadsheetCommands"
        when: windowShown && application.status===Loader.Ready
        property var app: application.item
        property var menu
        property var search
        property var backend
        property var body
        property var formulaBar
        property var editor

        function initTestCase() {
            menu=findChild(app,"commandMenu");
            search=findChild(app,"commandSearch");
            backend=findChild(app,"testGridModel");
            body=findChild(app,"gridBody");
            formulaBar=findChild(app,"formulaBar");
            editor=findChild(app,"cellEditor");
            verify(menu && search && backend && body && formulaBar && editor);
            app.requestActivate();
            tryCompare(app,"active",true);
        }
        function init() {
            backend.failWrites=false;
            backend.busy=false;
            backend.homeMode=false;
            backend.documentMode=true;
            menu.pendingCommand="";
            menu.close();
            for(let i=0;i<4;i++)keyClick(Qt.Key_Escape);
            body.forceActiveFocus();
            backend.style={};
            backend.actions=[];
        }
        function type(text) {for(let i=0;i<text.length;i++)keyClick(text[i]);}
        function openMenu() {
            keyClick(Qt.Key_Space,Qt.ControlModifier);
            tryCompare(menu,"visible",true);
            tryCompare(search,"activeFocus",true);
        }
        function choose(text) {
            openMenu();type(text);verify(menu.rows.length>0);keyClick(Qt.Key_Return);
            tryCompare(menu,"visible",false);
        }
        function test_search_and_direct_shortcuts_share_actions() {
            choose("bold");
            compare(backend.actions.length,1);
            compare(backend.actions[0].action,"format");
            compare(backend.actions[0].patch.bold,true);
            tryCompare(body,"activeFocus",true);
            keyClick(Qt.Key_B,Qt.ControlModifier);
            compare(backend.actions.length,2);
            compare(backend.actions[1].patch.bold,false);
            keyClick(Qt.Key_D,Qt.ControlModifier);
            compare(backend.actions[2].action,"fill");
            compare(backend.actions[2].right,false);
        }
        function test_categories_back_search_scope_and_escape() {
            openMenu();
            keyClick(Qt.Key_Down);keyClick(Qt.Key_Down);keyClick(Qt.Key_Right);
            compare(menu.route,"format");
            type("sheet");compare(menu.rows.length,0);
            keyClick(Qt.Key_Return);compare(menu.visible,true);
            keyClick(Qt.Key_Escape);compare(menu.query,"");compare(menu.route,"format");
            keyClick(Qt.Key_Backspace);compare(menu.route,"");
            compare(menu.rows[menu.selectedIndex].id,"format");
            type("freeze");compare(menu.rows.length,4);
            keyClick(Qt.Key_Down);keyClick(Qt.Key_Return);
            compare(backend.actions.length,1);
            compare(backend.actions[0].action,"freeze");
            compare(backend.actions[0].columns,1);
        }
        function test_cancel_preserves_inline_draft_and_text_shortcuts() {
            keyClick(Qt.Key_F2);tryCompare(editor,"visible",true);
            type("unsaved");
            const text=editor.text;
            openMenu();type("bold");
            // Global shortcuts are disabled while typing into the command search.
            keyClick(Qt.Key_B,Qt.ControlModifier);
            compare(backend.actions.length,0);
            keyClick(Qt.Key_Escape);compare(menu.visible,true);compare(menu.query,"");
            keyClick(Qt.Key_Escape);tryCompare(editor,"activeFocus",true);
            compare(editor.text,text);compare(editor.visible,true);
            compare(backend.actions.length,0);
        }
        function test_failed_save_restores_formula_draft_without_formatting() {
            formulaBar.forceActiveFocus();
            keyClick(Qt.Key_A,Qt.ControlModifier);type("unsaved");
            backend.failWrites=true;
            choose("bold");
            tryCompare(formulaBar,"activeFocus",true);
            compare(formulaBar.draft,"unsaved");compare(formulaBar.editing,true);
            compare(backend.actions.length,0);
            keyClick(Qt.Key_Escape);
        }
        function test_format_dialog_and_search_results_are_keyboard_accessible() {
            choose("format cells");
            const dialog=findChild(app,"formatDialog");
            tryCompare(dialog,"visible",true);
            compare(app.activeFocusItem.text,"Bold");
            keyClick(Qt.Key_Space);keyClick(Qt.Key_Tab);
            compare(app.activeFocusItem.text,"Italic");
            keyClick(Qt.Key_Escape);tryCompare(body,"activeFocus",true);
            choose("find and replace");
            const find=findChild(app,"findDialog");
            tryCompare(find,"visible",true);type("120");keyClick(Qt.Key_Return);
            // All controls, including the results list, participate in Tab traversal.
            let reached=false;
            for(let i=0;i<14;i++) {
                keyClick(Qt.Key_Tab);
                if(app.activeFocusItem && (app.activeFocusItem.objectName==="findResults"
                    || app.activeFocusItem.objectName.indexOf("findResult_")===0)) {reached=true;break;}
            }
            verify(reached,"Find results need keyboard focus");
            keyClick(Qt.Key_Down);keyClick(Qt.Key_Return);
            tryCompare(find,"visible",false);tryCompare(body,"activeFocus",true);
        }
        function test_delete_requires_confirmation_and_escape_restores_focus() {
            choose("delete selected rows");
            const dialog=findChild(app,"confirmDialog");
            tryCompare(dialog,"visible",true);compare(backend.actions.length,0);
            keyClick(Qt.Key_Escape);tryCompare(dialog,"visible",false);
            compare(backend.actions.length,0);tryCompare(body,"activeFocus",true);
        }
        function test_exact_colour_entry_and_apply_without_pointer() {
            choose("format cells");
            let reached=false;
            for(let i=0;i<18;i++) {
                if(app.activeFocusItem.objectName==="textColourEntry") {reached=true;break;}
                keyClick(Qt.Key_Tab);
            }
            verify(reached,"Text colour must be reachable by Tab");
            type("#112233");
            reached=false;
            for(let i=0;i<12;i++) {
                keyClick(Qt.Key_Tab);
                if(app.activeFocusItem.text==="OK") {reached=true;break;}
            }
            verify(reached,"Apply must be reachable by Tab");
            keyClick(Qt.Key_Space);
            compare(backend.actions.length,1);
            compare(backend.actions[0].patch.foreground,"#112233");
            tryCompare(body,"activeFocus",true);
        }
        function test_home_busy_and_modal_guards() {
            backend.homeMode=true;
            openMenu();type("bold");compare(menu.rows.length,0);
            keyClick(Qt.Key_Escape);keyClick(Qt.Key_Escape);
            backend.homeMode=false;body.forceActiveFocus();
            backend.busy=true;
            keyClick(Qt.Key_Space,Qt.ControlModifier);compare(menu.visible,false);
            backend.busy=false;
            choose("charts");
            const dialog=findChild(app,"chartDialog");tryCompare(dialog,"visible",true);
            keyClick(Qt.Key_Space,Qt.ControlModifier);compare(menu.visible,false);
            keyClick(Qt.Key_B,Qt.ControlModifier);compare(backend.actions.length,0);
            keyClick(Qt.Key_Escape);
        }
        function test_alternate_shortcut_and_page_navigation() {
            keyClick(Qt.Key_P,Qt.ControlModifier|Qt.ShiftModifier);
            tryCompare(menu,"visible",true);
            keyClick(Qt.Key_PageDown);compare(menu.rows[menu.selectedIndex].id,"agent");
            keyClick(Qt.Key_Backtab);compare(menu.rows[menu.selectedIndex].id,"view");
            keyClick(Qt.Key_Space,Qt.ControlModifier);tryCompare(menu,"visible",false);
            tryCompare(body,"activeFocus",true);
        }
    }
}
