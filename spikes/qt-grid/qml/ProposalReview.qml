pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Dialog {
    id: dialog
    required property var gridModel
    property var branches: JSON.parse(gridModel.proposalsJson || "[]")
    property var review: gridModel.reviewJson.length ? JSON.parse(gridModel.reviewJson) : null
    title: "Review agent proposal"
    modal: true
    focus: true
    standardButtons: Dialog.Close
    closePolicy: gridModel.busy ? Popup.NoAutoClose : Popup.CloseOnEscape
    onOpened: {gridModel.listProposals();proposals.forceActiveFocus();}

    function valueText(cell) {
        if (!cell || !cell.value || cell.value.type === "blank") return "(blank)";
        return String(cell.value.value === undefined ? cell.value.type : cell.value.value);
    }
    function formulaText(cell) {
        return cell && cell.state && cell.state.input.formula ? cell.state.input.formula.source : "";
    }

    Connections {
        target: dialog.gridModel
        function onProposalsJsonChanged() {
            if (!dialog.visible || dialog.gridModel.captureReview.length === 0) return;
            const index = dialog.branches.indexOf(dialog.gridModel.captureReview);
            if (index >= 0) { proposals.currentIndex = index; dialog.gridModel.reviewProposal(dialog.branches[index]); }
        }
    }

    contentItem: ColumnLayout {
        spacing: 12
        RowLayout {
            Layout.fillWidth: true
            ComboBox {
                id: proposals
                Layout.fillWidth: true
                model: dialog.branches
                displayText: currentIndex < 0 ? "No proposals yet" : "Proposal " + (currentIndex + 1) + " · " + currentText.slice(-8)
                enabled: !dialog.gridModel.busy
            }
            Button {
                text: "Load review"
                enabled: !dialog.gridModel.busy && proposals.currentIndex >= 0
                onClicked: dialog.gridModel.reviewProposal(proposals.currentText)
            }
            Button {
                text: "Refresh list"
                enabled: !dialog.gridModel.busy
                onClicked: dialog.gridModel.listProposals()
            }
        }
        Label {
            Layout.fillWidth: true
            visible: !dialog.review
            text: dialog.gridModel.busy ? "Preparing review…" : "Select a proposal to see its edits, calculated results and checks."
            wrapMode: Text.WordWrap
        }
        Label {
            Layout.fillWidth: true
            visible: dialog.gridModel.operationMessage.length > 0
            text: dialog.gridModel.operationMessage
            textFormat: Text.PlainText
            wrapMode: Text.WordWrap
        }
        ScrollView {
            id: scroll
            objectName: "proposalDetails"
            focusPolicy: Qt.StrongFocus
            padding: 4
            background: Rectangle {
                color: "transparent"
                border.width: scroll.activeFocus ? 1 : 0
                border.color: dialog.palette.highlight
            }
            Keys.onPressed: event => {
                const viewport=scroll.contentItem;
                const maximum=Math.max(0,viewport.contentHeight-viewport.height);
                const page=Math.max(1,viewport.height-24);
                let next=viewport.contentY;
                if(event.key===Qt.Key_Down)next+=40;
                else if(event.key===Qt.Key_Up)next-=40;
                else if(event.key===Qt.Key_PageDown)next+=page;
                else if(event.key===Qt.Key_PageUp)next-=page;
                else if(event.key===Qt.Key_Home)next=0;
                else if(event.key===Qt.Key_End)next=maximum;
                else return;
                viewport.contentY=Math.max(0,Math.min(maximum,next));
                event.accepted=true;
            }
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            visible: dialog.review !== null
            ColumnLayout {
                width: scroll.availableWidth
                spacing: 12
                Label {
                    Layout.fillWidth: true
                    text: dialog.review ? dialog.review.goal : ""
                    font.bold: true
                    font.pixelSize: 19
                    textFormat: Text.PlainText
                    wrapMode: Text.WordWrap
                }
                Label {
                    Layout.fillWidth: true
                    text: dialog.review ? dialog.review.explanation : ""
                    textFormat: Text.PlainText
                    wrapMode: Text.WordWrap
                }
                Label {
                    Layout.fillWidth: true
                    text: dialog.review ? "Assumptions\n" + (dialog.review.assumptions.join("\n") || "None stated")
                        + "\n\nEvidence\n" + dialog.review.evidence.join("\n") : ""
                    textFormat: Text.PlainText
                    wrapMode: Text.WordWrap
                }
                Label {
                    Layout.fillWidth: true
                    font.bold: true
                    text: !dialog.review ? "" : dialog.review.status !== "pending" ? "Proposal " + dialog.review.status
                        : dialog.review.can_approve ? "Ready for your decision"
                        : "Approval blocked. Check conflicts, checks and review limits below."
                    wrapMode: Text.WordWrap
                }
                Label {
                    Layout.fillWidth: true
                    visible: dialog.review && (dialog.review.truncated || dialog.review.unsupported_operations.length > 0 || dialog.review.diff.conflicts.length > 0)
                    text: !dialog.review ? "" : (dialog.review.truncated ? "The complete change exceeds review limits. Ask for a smaller proposal.\n" : "")
                        + (dialog.review.unsupported_operations.length ? "Unsupported changes: " + dialog.review.unsupported_operations.join(", ") + "\n" : "")
                        + (dialog.review.diff.conflicts.length ? "Conflicting edits: " + JSON.stringify(dialog.review.diff.conflicts) : "")
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                }
                Repeater {
                    model: dialog.review ? dialog.review.checks : []
                    delegate: Label {
                        required property var modelData
                        Layout.fillWidth: true
                        text: (modelData.passed ? "PASS · " : "FAIL · ") + modelData.name + " · " + modelData.severity + " · " + modelData.message
                        textFormat: Text.PlainText
                        wrapMode: Text.WordWrap
                    }
                }
                Label { text: "Cell edits and calculated changes"; font.bold: true }
                Repeater {
                    model: dialog.review ? dialog.review.cells : []
                    delegate: Frame {
                        id: change
                        required property var modelData
                        Layout.fillWidth: true
                        RowLayout {
                            width: parent.width
                            Label {
                                Layout.preferredWidth: 150
                                text: change.modelData.sheet + "!" + (change.modelData.after.a1 || change.modelData.before.a1)
                                textFormat: Text.PlainText
                                wrapMode: Text.WrapAnywhere
                                font.bold: true
                            }
                            Label {
                                Layout.fillWidth: true
                                Layout.preferredWidth: 1
                                text: "Before\n" + dialog.valueText(change.modelData.before) + "\n" + dialog.formulaText(change.modelData.before)
                                textFormat: Text.PlainText
                                wrapMode: Text.WrapAnywhere
                            }
                            Label {
                                Layout.fillWidth: true
                                Layout.preferredWidth: 1
                                text: "After\n" + dialog.valueText(change.modelData.after) + "\n" + dialog.formulaText(change.modelData.after)
                                textFormat: Text.PlainText
                                wrapMode: Text.WrapAnywhere
                            }
                        }
                    }
                }
            }
        }
        RowLayout {
            Layout.alignment: Qt.AlignRight
            Button {
                text: "Reject proposal"
                enabled: !dialog.gridModel.busy && dialog.review && dialog.review.status === "pending"
                onClicked: dialog.gridModel.resolveProposal(false)
            }
            Button {
                text: "Approve and save"
                enabled: !dialog.gridModel.busy && dialog.review && dialog.review.can_approve
                onClicked: dialog.gridModel.resolveProposal(true)
            }
        }
    }
}
