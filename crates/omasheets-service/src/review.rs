//! Bounded agent proposals and revision-bound, local human review.
use crate::{CellReport, ServiceError, revision};
use omasheets_core::{
    Actor, ActorKind, CellRef, CheckResult, Command, Document, ObjectId, ProposalId,
    ProposalStatus, Severity,
};
use omasheets_store::{BranchDiff, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MAX_REVIEW_CELLS: usize = 1_000;
const MAX_SCAN: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub goal: String,
    pub explanation: String,
    pub assumptions: Vec<String>,
    pub evidence: Vec<String>,
    pub commands: Vec<Command>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Metadata {
    native_branch: String,
    goal: String,
    explanation: String,
    assumptions: Vec<String>,
    evidence: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellChange {
    pub sheet: String,
    pub before: CellReport,
    pub after: CellReport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub branch: String,
    pub status: String,
    pub source_revision: String,
    pub target_revision: String,
    pub goal: String,
    pub explanation: String,
    pub assumptions: Vec<String>,
    pub evidence: Vec<String>,
    pub diff: BranchDiff,
    pub checks: Vec<CheckResult>,
    pub cells: Vec<CellChange>,
    pub truncated: bool,
    pub unsupported_operations: Vec<String>,
    pub can_approve: bool,
}

fn invalid(message: &str) -> ServiceError {
    ServiceError::new("invalid_proposal", message)
}

pub(crate) fn human() -> Actor {
    Actor::new(ActorKind::Human, "omasheets-review")
}

pub fn propose(
    store: &mut Store,
    expected: &str,
    now: i64,
    proposal: Proposal,
) -> Result<String, ServiceError> {
    let encoded = serde_json::to_vec(&proposal).map_err(|_| invalid("Invalid proposal JSON."))?;
    if proposal.commands.is_empty() || proposal.commands.len() > 500 || encoded.len() > 1_048_576 {
        return Err(invalid("A proposal needs 1–500 commands, within 1 MiB."));
    }
    if proposal.goal.trim().is_empty()
        || proposal.goal.len() > 1_024
        || proposal.explanation.trim().is_empty()
        || proposal.explanation.len() > 8_192
        || proposal.assumptions.len() > 16
        || proposal.evidence.is_empty()
        || proposal.evidence.len() > 32
        || proposal
            .assumptions
            .iter()
            .chain(&proposal.evidence)
            .any(|s| s.trim().is_empty() || s.len() > 2_048)
    {
        return Err(invalid(
            "Provide a bounded goal, explanation, assumptions and nonempty evidence.",
        ));
    }
    if proposal.commands.iter().any(|command| {
        !matches!(
            command,
            Command::SetValue { .. }
                | Command::SetFormula { .. }
                | Command::ClearCell { .. }
                | Command::AddCheck { .. }
                | Command::WatchOutput { .. }
        )
    }) {
        return Err(invalid(
            "Native proposals support cell edits, checks and watched outputs only.",
        ));
    }
    let main = store.branch_id("main")?;
    if revision(store.document(main)?) != expected {
        return Err(ServiceError::new(
            "document_changed",
            "The workbook changed; inspect its current revision before proposing.",
        ));
    }
    let branch = format!(
        "proposal-{}",
        ObjectId::from_seed(&format!(
            "{expected}:{now}:{}",
            String::from_utf8_lossy(&encoded)
        ))
    );
    let metadata = Metadata {
        native_branch: branch.clone(),
        goal: proposal.goal,
        explanation: proposal.explanation,
        assumptions: proposal.assumptions,
        evidence: proposal.evidence,
    };
    let description =
        serde_json::to_string(&metadata).map_err(|_| invalid("Invalid proposal metadata."))?;
    if description.len() > 30_000 {
        return Err(invalid("Proposal metadata exceeds 30,000 bytes."));
    }
    let mut commands = vec![Command::Propose { description }];
    commands.extend(proposal.commands);
    store.create_branch_with_commands(
        main,
        &branch,
        Actor::new(ActorKind::Agent, "omasheets-agent"),
        now,
        commands,
    )?;
    Ok(branch)
}

fn metadata(
    document: &Document,
    branch: &str,
) -> Result<(ProposalId, ProposalStatus, Metadata), ServiceError> {
    document
        .proposals()
        .iter()
        .find_map(|(id, record)| {
            let metadata: Metadata = serde_json::from_str(&record.description).ok()?;
            (metadata.native_branch == branch).then_some((*id, record.status, metadata))
        })
        .ok_or_else(|| invalid("This branch does not contain a native agent proposal."))
}

fn report(document: &Document, cell: CellRef) -> CellReport {
    CellReport {
        cell,
        a1: document.project_a1(cell),
        value: document.value(cell),
        state: document.cell(cell).cloned(),
    }
}

fn passed(checks: &[CheckResult]) -> bool {
    checks
        .iter()
        .all(|check| check.severity != Severity::Error || check.passed)
}

pub fn inspect(store: &mut Store, branch: &str) -> Result<Review, ServiceError> {
    let source = store.branch_id(branch)?;
    let target = store.branch_id("main")?;
    let source_document = store.document(source)?;
    let (_, status, metadata) = metadata(source_document, branch)?;
    let source_revision = revision(source_document);
    let diff = store.diff(source, target)?;
    let candidate = store.preview_merge(source, target)?;
    let checks = candidate.check_results();
    let target_document = store.document(target)?;
    let target_revision = revision(target_document);
    let mut identities = BTreeSet::new();
    let mut truncated = false;
    for document in [target_document, &candidate] {
        for sheet in document.sheets() {
            for cell in document.cells_in_view(*sheet) {
                identities.insert(cell);
                if identities.len() > MAX_SCAN {
                    truncated = true;
                    break;
                }
            }
            if truncated {
                break;
            }
        }
        if truncated {
            break;
        }
    }
    let mut cells = Vec::new();
    let mut review_bytes = 0;
    for cell in identities {
        let before = report(target_document, cell);
        let after = report(&candidate, cell);
        if before.value != after.value
            || before.state.as_ref().map(|s| &s.input) != after.state.as_ref().map(|s| &s.input)
        {
            if cells.len() == MAX_REVIEW_CELLS {
                truncated = true;
                break;
            }
            let change = CellChange {
                sheet: candidate
                    .sheet_name(cell.sheet)
                    .unwrap_or("Deleted sheet")
                    .into(),
                before,
                after,
            };
            review_bytes += serde_json::to_vec(&change)
                .map_err(|_| invalid("Invalid cell projection."))?
                .len();
            if review_bytes > 2 * 1024 * 1024 {
                truncated = true;
                break;
            }
            cells.push(change);
        }
    }
    let unsupported_operations = diff
        .source_operations
        .iter()
        .filter(|op| {
            !matches!(
                op.operation.as_str(),
                "propose"
                    | "set_value"
                    | "set_formula"
                    | "clear_cell"
                    | "add_check"
                    | "watch_output"
                    | "reject_proposal"
            )
        })
        .map(|op| op.operation.clone())
        .collect::<Vec<_>>();
    let status = if status == ProposalStatus::Rejected {
        "rejected"
    } else if diff.source_operations.is_empty() {
        "applied"
    } else {
        "pending"
    }
    .to_string();
    let can_approve = status == "pending"
        && !truncated
        && unsupported_operations.is_empty()
        && diff.conflicts.is_empty()
        && passed(&checks)
        && passed(&diff.source_checks);
    Ok(Review {
        branch: branch.into(),
        status,
        source_revision,
        target_revision,
        goal: metadata.goal,
        explanation: metadata.explanation,
        assumptions: metadata.assumptions,
        evidence: metadata.evidence,
        diff,
        checks,
        cells,
        truncated,
        unsupported_operations,
        can_approve,
    })
}

pub fn reject(
    store: &mut Store,
    branch: &str,
    expected: &str,
    now: i64,
    reason: String,
) -> Result<(), ServiceError> {
    if reason.trim().is_empty() || reason.len() > 8_192 {
        return Err(invalid("Give a rejection reason within 8,192 bytes."));
    }
    let current = inspect(store, branch)?;
    if current.source_revision != expected || current.status != "pending" {
        return Err(ServiceError::new(
            "stale_review",
            "The proposal changed; refresh the review.",
        ));
    }
    let source = store.branch_id(branch)?;
    let (proposal, _, _) = metadata(store.document(source)?, branch)?;
    store.append(
        source,
        human(),
        now,
        Command::RejectProposal { proposal, reason },
    )?;
    Ok(())
}
