use omasheets_core::{Actor, ActorKind, CellValue, Command, Literal, Severity, SheetId};
use omasheets_service::{
    Request, Response, Service,
    review::{Proposal, Review},
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    service: Service,
    path: PathBuf,
    sheet: SheetId,
}

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "omasheets-review-{}-{}-{}.omasheets",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut service = Service::new(|| 123);
        service
            .handle(Request::Create {
                path: path.clone(),
                name: "Budget".into(),
                actor: human(),
            })
            .unwrap();
        service
            .handle(Request::Append {
                path: path.clone(),
                branch: None,
                actor: human(),
                command: Command::AddSheet {
                    name: "Budget".into(),
                },
            })
            .unwrap();
        let Response::Document(summary) = service
            .handle(Request::Document {
                path: path.clone(),
                branch: None,
            })
            .unwrap()
        else {
            panic!()
        };
        let sheet = summary.sheets[0].id;
        let mut fixture = Self {
            service,
            path,
            sheet,
        };
        fixture.append(
            None,
            Command::AddRows {
                sheet,
                count: 1100,
                at: 0,
                table: None,
            },
        );
        fixture.append(
            None,
            Command::AddColumns {
                sheet,
                count: 4,
                at: 0,
            },
        );
        fixture.append(None, fixture.value("A1", 10.0));
        fixture.append(None, fixture.value("C1", 2.0));
        fixture.append(
            None,
            Command::SetFormula {
                sheet,
                a1: "B1".into(),
                source: "=A1+C1".into(),
            },
        );
        fixture.append(
            None,
            Command::SetFormula {
                sheet,
                a1: "D1".into(),
                source: "=B1<24".into(),
            },
        );
        fixture.append(
            None,
            Command::AddCheck {
                sheet,
                a1: "D1".into(),
                name: "Budget ceiling".into(),
                severity: Severity::Error,
                message: "Total must stay below 24".into(),
            },
        );
        fixture
    }
    fn value(&self, a1: &str, value: f64) -> Command {
        Command::SetValue {
            sheet: self.sheet,
            a1: a1.into(),
            value: Literal::Number(value),
        }
    }
    fn append(&mut self, branch: Option<&str>, command: Command) {
        self.service
            .handle(Request::Append {
                path: self.path.clone(),
                branch: branch.map(str::to_owned),
                actor: human(),
                command,
            })
            .unwrap();
    }
    fn summary(&mut self) -> omasheets_service::DocumentSummary {
        let Response::Document(summary) = self
            .service
            .handle(Request::Document {
                path: self.path.clone(),
                branch: None,
            })
            .unwrap()
        else {
            panic!()
        };
        summary
    }
    fn proposal(&self) -> Proposal {
        Proposal {
            goal: "Update forecast".into(),
            explanation: "Use the revised cost.".into(),
            assumptions: vec!["Quantity is unchanged".into()],
            evidence: vec!["Budget!A1 was 10".into()],
            commands: vec![self.value("A1", 15.0)],
        }
    }
    fn propose(&mut self) -> String {
        let expected_revision = self.summary().revision;
        let Response::NativeProposed { branch } = self
            .service
            .handle(Request::ProposeNative {
                path: self.path.clone(),
                expected_revision,
                proposal: self.proposal(),
            })
            .unwrap()
        else {
            panic!()
        };
        branch
    }
    fn review(&mut self, branch: &str) -> Review {
        let Response::NativeReview(review) = self
            .service
            .handle(Request::ReviewNative {
                path: self.path.clone(),
                source: branch.into(),
            })
            .unwrap()
        else {
            panic!()
        };
        review
    }
    fn approve(&mut self, review: &Review) -> Result<Response, omasheets_service::ServiceError> {
        self.service.handle(Request::ApproveNative {
            path: self.path.clone(),
            source: review.branch.clone(),
            source_revision: review.source_revision.clone(),
            target_revision: review.target_revision.clone(),
        })
    }
    fn reopen(&mut self) {
        self.service
            .handle(Request::Close {
                path: self.path.clone(),
            })
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.service.handle(Request::Close {
            path: self.path.clone(),
        });
        let _ = std::fs::remove_file(&self.path);
    }
}
fn human() -> Actor {
    Actor::new(ActorKind::Human, "test")
}

#[test]
fn review_includes_derived_changes_and_approval_survives_reopen() {
    let mut f = Fixture::new();
    let before = f.summary().digest;
    let branch = f.propose();
    assert_eq!(f.summary().digest, before);
    let review = f.review(&branch);
    assert!(review.can_approve);
    assert!(
        review
            .cells
            .iter()
            .any(|cell| cell.after.a1.as_deref() == Some("B1")
                && cell.after.value == CellValue::Number(17.0))
    );
    f.approve(&review).unwrap();
    let digest = f.summary().digest;
    f.reopen();
    assert_eq!(digest, f.summary().digest);
    assert_eq!(f.review(&branch).status, "applied");
    assert_eq!(f.approve(&review).unwrap_err().code, "stale_review");
}

#[test]
fn prospective_checks_and_revision_guards_block_unsafe_combination() {
    let mut f = Fixture::new();
    let branch = f.propose();
    let review = f.review(&branch);
    f.append(None, f.value("C1", 10.0));
    assert_eq!(f.approve(&review).unwrap_err().code, "stale_review");
    let current = f.review(&branch);
    assert!(current.diff.source_checks.iter().all(|check| check.passed));
    assert!(current.checks.iter().any(|check| !check.passed));
    assert!(!current.can_approve);
    assert_eq!(f.approve(&current).unwrap_err().code, "review_blocked");
    assert_eq!(
        f.service
            .handle(Request::Merge {
                path: f.path.clone(),
                source: branch.clone(),
                target: None,
                approver: human()
            })
            .unwrap_err()
            .code,
        "checks_failed"
    );
    f.append(None, f.value("C1", 5.0));
    let current = f.review(&branch);
    assert!(current.can_approve);
    f.approve(&current).unwrap();
}

#[test]
fn invalid_and_stale_proposals_never_leave_partial_branches() {
    let mut f = Fixture::new();
    let before = f.summary();
    let mut proposal = f.proposal();
    proposal.commands.push(Command::SetFormula {
        sheet: f.sheet,
        a1: "B2".into(),
        source: "=NO_SUCH_FUNCTION(A1)".into(),
    });
    assert!(
        f.service
            .handle(Request::ProposeNative {
                path: f.path.clone(),
                expected_revision: before.revision.clone(),
                proposal
            })
            .is_err()
    );
    assert_eq!(f.summary().branches, before.branches);
    assert_eq!(f.summary().digest, before.digest);
    assert_eq!(
        f.service
            .handle(Request::ProposeNative {
                path: f.path.clone(),
                expected_revision: "stale".into(),
                proposal: f.proposal()
            })
            .unwrap_err()
            .code,
        "document_changed"
    );
    let mut proposal = f.proposal();
    proposal.commands = vec![Command::Tick { at: 100 }];
    assert_eq!(
        f.service
            .handle(Request::ProposeNative {
                path: f.path.clone(),
                expected_revision: before.revision,
                proposal
            })
            .unwrap_err()
            .code,
        "invalid_proposal"
    );
    assert_eq!(f.summary().branches, before.branches);
}

#[test]
fn rejection_is_durable_and_never_changes_main() {
    let mut f = Fixture::new();
    let before = f.summary().digest;
    let branch = f.propose();
    let review = f.review(&branch);
    f.service
        .handle(Request::RejectNative {
            path: f.path.clone(),
            source: branch.clone(),
            source_revision: review.source_revision,
            reason: "Wrong assumption".into(),
        })
        .unwrap();
    f.reopen();
    assert_eq!(f.summary().digest, before);
    let review = f.review(&branch);
    assert_eq!(review.status, "rejected");
    assert!(!review.can_approve);
}

#[test]
fn source_edits_invalidate_an_open_review() {
    let mut f = Fixture::new();
    let branch = f.propose();
    let review = f.review(&branch);
    f.append(Some(&branch), f.value("A1", 16.0));
    assert_eq!(f.approve(&review).unwrap_err().code, "stale_review");
}

#[test]
fn large_derived_change_requires_a_smaller_proposal() {
    let mut f = Fixture::new();
    let commands = (2..=1001)
        .map(|row| Command::SetFormula {
            sheet: f.sheet,
            a1: format!("B{row}"),
            source: "=A1".into(),
        })
        .collect();
    f.service
        .handle(Request::AppendBatch {
            path: f.path.clone(),
            branch: None,
            actor: human(),
            commands,
            expected_digest: None,
            expected_revision: None,
        })
        .unwrap();
    let branch = f.propose();
    let review = f.review(&branch);
    assert!(review.truncated);
    assert!(!review.can_approve);
    assert_eq!(review.cells.len(), 1000);
}

#[test]
fn undisplayed_structural_changes_block_approval() {
    let mut f = Fixture::new();
    let branch = f.propose();
    f.append(
        Some(&branch),
        Command::RenameSheet {
            sheet: f.sheet,
            name: "Renamed".into(),
        },
    );
    let review = f.review(&branch);
    assert!(!review.can_approve);
    assert!(
        review
            .unsupported_operations
            .contains(&"rename_sheet".into())
    );
}

#[test]
fn distinct_proposals_at_the_same_main_head_have_distinct_identities() {
    let mut f = Fixture::new();
    let before = f.summary();
    let first = f.propose();
    let review = f.review(&first);
    f.service
        .handle(Request::RejectNative {
            path: f.path.clone(),
            source: first.clone(),
            source_revision: review.source_revision,
            reason: "Try a revised assumption".into(),
        })
        .unwrap();
    f.reopen();
    let mut proposal = f.proposal();
    proposal.commands = vec![f.value("A1", 16.0)];
    let Response::NativeProposed { branch: second } = f
        .service
        .handle(Request::ProposeNative {
            path: f.path.clone(),
            expected_revision: before.revision,
            proposal,
        })
        .unwrap()
    else {
        panic!()
    };
    assert_ne!(first, second);
    assert_eq!(f.summary().branches.len(), 3);
    assert_eq!(f.summary().digest, before.digest);
    assert!(f.review(&second).can_approve);
    assert_eq!(f.review(&first).status, "rejected");
}
