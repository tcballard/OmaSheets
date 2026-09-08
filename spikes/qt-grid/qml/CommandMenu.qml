pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Popup {
    id: menu
    objectName: "commandMenu"
    required property var entries
    required property color backgroundColor
    required property color textColor
    required property color accentColor
    required property color mutedColor
    property string route: ""
    property string query: ""
    property int selectedIndex: -1
    property var returnFocusItem: null
    property string pendingCommand: ""
    signal commandChosen(string commandId)
    anchors.centerIn: parent
    width: Math.min(480, parent.width-40)
    height: Math.min(500, parent.height-40)
    padding: 16
    modal: true
    focus: true
    closePolicy: Popup.CloseOnPressOutside
    // Fixed geometry keeps the search field still while results narrow.
    enter: Transition {}
    exit: Transition {}

    function entry(id) { return entries.find(item => item.id===id); }
    function selectable(item) {
        if (!item || !item.enabled) return false;
        if (!item.group) return true;
        return entries.some(child => !child.group && child.enabled && child.id.indexOf(item.id+".")===0);
    }
    function path(id) {
        const parts=id.split(".");
        let names=[];
        for(let i=1;i<=parts.length;i++) {
            const item=entry(parts.slice(0,i).join("."));
            if(item)names.push(item.text);
        }
        return names.join(" › ");
    }
    function score(item, text) {
        const label=item.text.toLowerCase();
        const haystack=(path(item.id)+" "+item.detail).toLowerCase();
        if(!text.split(/\s+/).every(word => haystack.indexOf(word)>=0))return -1;
        return label===text ? 0 : label.indexOf(text)===0 ? 1 : label.indexOf(text)>=0 ? 2 : 3;
    }
    readonly property var rows: {
        const text=query.trim().toLowerCase();
        if(!text)return entries.filter(item => item.parent===route);
        return entries.filter(item => !item.group && selectable(item)
                && (!route || item.id.indexOf(route+".")===0) && score(item,text)>=0)
            .map((item,index) => ({item:item,index:index,score:score(item,text)}))
            .sort((a,b) => a.score-b.score || a.index-b.index).map(result => result.item);
    }
    onRowsChanged: resetSelection()

    function resetSelection() {
        selectedIndex=rows.findIndex(item => selectable(item));
        if(selectedIndex>=0)results.positionViewAtIndex(selectedIndex,ListView.Contain);
    }
    function show(previousFocus) {
        returnFocusItem=previousFocus;
        pendingCommand="";
        route="";
        query="";
        open();
    }
    function move(delta) {
        if(!rows.length)return;
        const direction=delta<0 ? -1 : 1;
        let index=selectedIndex;
        for(let step=0;step<Math.abs(delta);step++) {
            for(let tried=0;tried<rows.length;tried++) {
                index=(index+direction+rows.length)%rows.length;
                if(selectable(rows[index]))break;
            }
        }
        if(selectable(rows[index]))selectedIndex=index;
        results.positionViewAtIndex(selectedIndex,ListView.Contain);
    }
    function back() {
        if(query.length) {query="";return;}
        if(!route)return;
        const previous=route;
        const item=entry(route);
        route=item ? item.parent : "";
        selectedIndex=rows.findIndex(row => row.id===previous);
        search.forceActiveFocus();
    }
    function activate(index) {
        const item=rows[index];
        if(!selectable(item))return;
        if(item.group) {
            route=item.id;
            query="";
            resetSelection();
            search.forceActiveFocus();
        } else {
            pendingCommand=item.id;
            close();
        }
    }
    onOpened: {resetSelection();search.forceActiveFocus();}
    onClosed: {
        const command=pendingCommand;
        pendingCommand="";
        if(returnFocusItem && returnFocusItem.visible && returnFocusItem.enabled)
            returnFocusItem.forceActiveFocus(Qt.ShortcutFocusReason);
        returnFocusItem=null;
        if(command.length)commandChosen(command);
    }

    background: Rectangle {
        color: menu.backgroundColor
        border.color: menu.accentColor
        border.width: 2
        radius: 4
    }
    contentItem: ColumnLayout {
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            ToolButton {
                visible: menu.route.length>0
                text: "‹"
                focusPolicy: Qt.NoFocus
                Accessible.name: "Back to parent menu"
                onClicked: menu.back()
            }
            Label {
                Layout.fillWidth: true
                text: menu.route ? menu.path(menu.route) : "OmaSheets"
                textFormat: Text.PlainText
                elide: Text.ElideLeft
                font.bold: true
                color: menu.textColor
            }
            Label {text:"Ctrl+Space";font.pixelSize:11;color:menu.mutedColor}
        }
        TextField {
            id: search
            objectName: "commandSearch"
            Layout.fillWidth: true
            placeholderText: menu.route ? "Search here…" : "Search all commands…"
            placeholderTextColor: menu.mutedColor
            text: menu.query
            onTextEdited: menu.query=text
            selectByMouse: true
            Accessible.name: "Search spreadsheet commands"
            Keys.priority: Keys.BeforeItem
            Keys.onPressed: event => {
                const control=(event.modifiers & Qt.ControlModifier)!==0;
                const shift=(event.modifiers & Qt.ShiftModifier)!==0;
                if(control && (event.key===Qt.Key_Space || shift && event.key===Qt.Key_P))menu.close();
                else if(event.key===Qt.Key_Escape) {
                    if(menu.query.length)menu.query="";
                    else menu.close();
                } else if(event.key===Qt.Key_Backspace && !menu.query.length
                          || event.key===Qt.Key_Left && !menu.query.length) menu.back();
                else if(event.key===Qt.Key_Up || event.key===Qt.Key_Backtab)menu.move(-1);
                else if(event.key===Qt.Key_Down || event.key===Qt.Key_Tab)menu.move(1);
                else if(event.key===Qt.Key_PageUp)menu.move(-6);
                else if(event.key===Qt.Key_PageDown)menu.move(6);
                else if(event.key===Qt.Key_Return || event.key===Qt.Key_Enter)menu.activate(menu.selectedIndex);
                else if(event.key===Qt.Key_Right && !menu.query.length)menu.activate(menu.selectedIndex);
                else return;
                event.accepted=true;
            }
        }
        ListView {
            id: results
            objectName: "commandResults"
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: menu.rows
            spacing: 3
            ScrollBar.vertical: ScrollBar {}
            delegate: ItemDelegate {
                id: row
                required property var modelData
                required property int index
                width: results.width
                height: menu.query.trim().length ? 57 : 40
                focusPolicy: Qt.NoFocus
                enabled: menu.selectable(modelData)
                highlighted: index===menu.selectedIndex
                Accessible.role: Accessible.MenuItem
                Accessible.name: modelData.text+(modelData.shortcut ? ", "+modelData.shortcut : "")
                Accessible.description: menu.path(modelData.parent)
                Accessible.selected: highlighted
                onClicked: menu.activate(index)
                // Pointer hover never overrides a keyboard-selected row.
                background: Rectangle {
                    color: row.highlighted && row.enabled ? menu.accentColor : "transparent"
                    radius: 2
                }
                contentItem: RowLayout {
                    spacing: 8
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Label {
                            Layout.fillWidth: true
                            text: row.modelData.text
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            color: row.highlighted && row.enabled ? menu.backgroundColor : menu.textColor
                            opacity: row.enabled ? 1 : 0.4
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: menu.query.trim().length>0
                            text: menu.path(row.modelData.parent)
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            font.pixelSize: 11
                            color: row.highlighted ? menu.backgroundColor : menu.mutedColor
                        }
                    }
                    Label {
                        text: row.modelData.group ? "›" : row.modelData.shortcut
                        font.pixelSize: row.modelData.group ? 18 : 11
                        color: row.highlighted && row.enabled ? menu.backgroundColor : menu.mutedColor
                    }
                }
            }
            Label {
                anchors.centerIn: parent
                visible: menu.rows.length===0
                width: parent.width-24
                text: "No matching commands\nTry another word, or press Escape to clear."
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                color: menu.mutedColor
            }
        }
        Label {
            Layout.fillWidth: true
            text: "↑↓ Navigate   Enter Choose   ← Back   Esc Clear / close"
            font.pixelSize: 11
            wrapMode: Text.WordWrap
            color: menu.mutedColor
        }
    }
}
