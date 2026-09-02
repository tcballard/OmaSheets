import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// This plugin is deliberately a thin local-status surface. Omarchy plugins
// run unsandboxed in the desktop shell, so no agent-supplied string is ever
// interpolated into a command. Every process below has a fixed argv vector.
Panel {
  id: root
  moduleName: "io.github.tcballard.omasheets"
  ipcTarget: "io.github.tcballard.omasheets"

  property bool statusRunning: false
  property bool installed: false
  property string statusError: ""
  property bool selected: false
  property string workbookName: "No workbook selected"
  property string workbookFormat: ""
  property bool reviewPending: false
  property string reviewState: ""
  property int operationCount: 0
  property int destructiveCount: 0
  property int warningCount: 0
  property int formulaErrorCount: 0

  readonly property string pluginRoot: String(root.manifest.__sourceDir || "")
  readonly property string pluginLauncher: pluginRoot + "/bin/omasheets-plugin"

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  function refresh() {
    if (statusProcess.running) return
    statusError = ""
    statusRunning = true
    statusDeadline.restart()
    statusProcess.running = true
  }

  function applyStatus(payload) {
    var current = payload.current || {}
    var review = payload.review || {}
    installed = payload.installed !== false
    selected = current.selected === true
    workbookName = selected ? String(current.display_name || "Selected workbook") : "No workbook selected"
    workbookFormat = selected ? String(current.format || "").toUpperCase() : ""
    reviewPending = review.pending === true
    reviewState = reviewPending ? String(review.status || "") : ""
    operationCount = reviewPending ? Number(review.operation_count || 0) : 0
    destructiveCount = reviewPending ? Number(review.destructive_count || 0) : 0
    warningCount = reviewPending ? Number(review.warning_count || 0) : 0
    formulaErrorCount = reviewPending ? Number(review.formula_error_count || 0) : 0
  }

  function open() {
    refresh()
    controller.show()
  }

  function openWorkbook() {
    Quickshell.execDetached([root.pluginLauncher, "run", "window-current"])
    close()
  }

  function openWorkbookInCalc() {
    Quickshell.execDetached([root.pluginLauncher, "run", "open-current"])
    close()
  }

  function reviewChanges() {
    // The CLI resolves the current sealed plan itself and requires the user to
    // type the exact approval token in a terminal. The panel cannot commit.
    Quickshell.execDetached([
      "omarchy-launch-tui",
      "--app-id=org.omarchy.omasheets-review",
      root.pluginLauncher,
      "run",
      "review-current"
    ])
    close()
  }

  function askAgent() {
    // The installed CLI owns the fixed, path-free session prompt. Omarchy
    // resolves and launches the user's configured default agent.
    Quickshell.execDetached([root.pluginLauncher, "run", "agent-session"])
    close()
  }

  function installProduct() {
    Quickshell.execDetached([
      "omarchy-launch-tui",
      "--app-id=org.omarchy.omasheets-setup",
      root.pluginLauncher,
      "install"
    ])
    close()
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onOpenedChanged: if (opened) refresh()

  // The status helper enforces its own 5 s deadline and 16 KiB output limit
  // and terminates the launcher's process group when either trips. This timer
  // is the widget-side backstop if the helper itself never returns.
  Timer {
    id: statusDeadline
    interval: 8000
    repeat: false
    onTriggered: {
      if (statusProcess.running) {
        statusProcess.running = false
        root.statusRunning = false
        root.statusError = "OmaSheets status unavailable"
      }
    }
  }

  Process {
    id: statusProcess
    command: [root.pluginLauncher, "status"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        try {
          var textValue = String(text || "")
          if (textValue.length > 16384) throw new Error("status response exceeded limit")
          root.applyStatus(JSON.parse(textValue))
        } catch (error) {
          root.statusError = "OmaSheets status unavailable"
        }
      }
    }
    onExited: function(exitCode) {
      statusDeadline.stop()
      root.statusRunning = false
      if (exitCode !== 0) root.statusError = "OmaSheets status unavailable"
    }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: "󰈛"
    active: root.reviewPending
    tooltipText: root.reviewPending ? "OmaSheets changes await review" : "OmaSheets"
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.MiddleButton) root.refresh()
      else if (buttonCode === Qt.RightButton && root.selected) root.openWorkbook()
      else root.toggle()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(360))
    contentHeight: panel.fittedContentHeight(content.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onActivateRequested: root.reviewPending ? root.reviewChanges() : root.refresh()
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(textValue) {
        if (textValue === "r" || textValue === "R") root.refresh()
        else if ((textValue === "o" || textValue === "O") && root.selected) root.openWorkbook()
        else if ((textValue === "a" || textValue === "A") && root.selected) root.askAgent()
      }

      Column {
        id: content
        width: parent.width
        spacing: Style.space(12)

        PanelHero {
          width: parent.width
          title: root.installed ? root.workbookName : "Finish OmaSheets setup"
          meta: root.installed
            ? (root.workbookFormat === "" ? "Native Linux spreadsheets" : root.workbookFormat + " · selected locally")
            : "Dependencies are explicit; installation stays user-local"
          foreground: root.foreground
          fontFamily: root.fontFamily
        }

        BorderSurface {
          width: parent.width
          implicitHeight: reviewColumn.implicitHeight + Style.space(24)
          color: root.reviewPending ? Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.08) : "transparent"
          radius: Style.cornerRadius

          Column {
            id: reviewColumn
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.margins: Style.space(12)
            spacing: Style.space(6)

            Text {
              width: parent.width
              textFormat: Text.PlainText
              text: root.reviewPending ? "Changes awaiting local review" : "No staged changes"
              color: root.foreground
              font.family: root.fontFamily
              font.pixelSize: Style.font.body
              font.bold: true
            }

            Text {
              visible: root.reviewPending
              width: parent.width
              textFormat: Text.PlainText
              text: root.operationCount + " operations · " + root.destructiveCount + " destructive · "
                + root.warningCount + " warnings · " + root.formulaErrorCount + " formula errors"
              color: (root.destructiveCount + root.formulaErrorCount) > 0 ? root.urgent : root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }

            Text {
              visible: root.statusError !== ""
              width: parent.width
              textFormat: Text.PlainText
              text: root.statusError
              color: root.urgent
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
          }
        }

        Flow {
          width: parent.width
          spacing: Style.space(8)

          Button {
            visible: !root.installed
            text: "Install OmaSheets"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.installProduct()
          }

          Button {
            visible: root.selected
            text: "Open in OmaSheets"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.openWorkbook()
          }

          Button {
            visible: root.selected
            text: "Ask Agent"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.askAgent()
          }

          Button {
            visible: root.selected
            text: "Calc fallback"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.openWorkbookInCalc()
          }

          Button {
            visible: root.reviewPending
            text: "Review in terminal"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.reviewChanges()
          }

          Button {
            text: root.statusRunning ? "Refreshing…" : "Refresh"
            enabled: !root.statusRunning
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.refresh()
          }
        }
      }
    }
  }
}
