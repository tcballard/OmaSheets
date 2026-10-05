//! Human-only publication, integrity-sealed receipts, crash recovery and undo.
use crate::{
    Result,
    files::{self, AppPaths, Lock},
    policy::{self, Actor, PublishMode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub fn canonical_hash(v: &Value) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(v).map_err(|e| e.to_string())?)
    ))
}
fn time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().ok_or_else(|| format!("missing {k}"))
}
fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}
fn replace(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination.parent().ok_or("target needs parent")?;
    let temp = parent.join(format!(".omasheets-{}", files::random_id()?));
    files::copy_no_clobber(source, &temp)?;
    let result = fs::rename(&temp, destination)
        .map_err(|e| e.to_string())
        .and_then(|_| files::sync_dir(parent));
    let _ = fs::remove_file(temp);
    result
}
pub struct Receipts {
    dir: PathBuf,
}
impl Receipts {
    pub fn new(paths: &AppPaths) -> Result<Self> {
        let dir = paths.state.join("receipts");
        files::private_dir(&dir)?;
        Ok(Self { dir })
    }
    pub fn path(&self, id: &str) -> Result<PathBuf> {
        let base = id.strip_prefix("undo-").unwrap_or(id);
        if !files::valid_id(base) {
            return Err("invalid receipt identifier".into());
        }
        Ok(self.dir.join(format!("{id}.json")))
    }
    pub fn get(&self, id: &str) -> Result<Value> {
        let mut v = files::read_json(&self.path(id)?, 4 * 1024 * 1024)?;
        let hash = v
            .as_object_mut()
            .ok_or("invalid receipt")?
            .remove("receipt_hash")
            .ok_or("receipt integrity check failed")?;
        if hash.as_str() != Some(&canonical_hash(&v)?) {
            return Err("receipt integrity check failed".into());
        }
        v["receipt_hash"] = hash;
        Ok(v)
    }
    pub fn record(&self, mut receipt: Value) -> Result<Value> {
        let _lock = Lock::acquire(&self.dir.join(".chain.lock"))?;
        let path = self.path(string(&receipt, "receipt_id")?)?;
        if exists(&path) {
            return self.get(string(&receipt, "receipt_id")?);
        }
        let head = self.dir.join("chain-head.json");
        let previous = if exists(&head) {
            files::read_json(&head, 8192)?
        } else {
            json!({})
        };
        receipt["previous_receipt_hash"] = previous["receipt_hash"].clone();
        receipt["recorded_at"] = json!(time());
        receipt["receipt_hash"] = json!(canonical_hash(&receipt)?);
        files::write_json(&path, &receipt)?;
        files::write_json(
            &head,
            &json!({"receipt_id":receipt["receipt_id"],"receipt_hash":receipt["receipt_hash"]}),
        )?;
        Ok(receipt)
    }
}
pub struct Publisher {
    paths: AppPaths,
    pub receipts: Receipts,
}
impl Publisher {
    pub fn new(paths: AppPaths) -> Result<Self> {
        files::private_dir(&paths.state.join("backups"))?;
        Ok(Self {
            receipts: Receipts::new(&paths)?,
            paths,
        })
    }
    pub fn publish(&self, plan: &Value, source: &Path, actor: Actor) -> Result<Value> {
        policy::publish_authority(actor, PublishMode::Copy)?;
        let id = string(plan, "receipt_id")?;
        if !files::valid_id(id) || !files::valid_id(string(plan, "plan_id")?) {
            return Err("invalid receipt identifier".into());
        }
        let _lock = Lock::acquire(
            &self
                .paths
                .state
                .join("locks")
                .join(format!("plan-{}.lock", string(plan, "plan_id")?)),
        )?;
        if exists(&self.receipts.path(id)?) {
            return self.receipts.get(id);
        }
        let staged = Path::new(string(plan, "staged_artifact")?);
        let target = Path::new(string(plan, "target_destination")?);
        let staged_hash = files::identify(staged)?.sha256;
        if staged_hash != string(plan, "staged_sha256")? {
            return Err("staged artifact changed before publication".into());
        }
        let source_hash = string(plan, "source_sha256")?;
        let mut backup = None;
        match string(plan, "target_mode")? {
            "copy" => {
                if exists(target) {
                    if files::identify(target)?.sha256 != staged_hash {
                        return Err("copy destination already contains different bytes".into());
                    }
                } else {
                    files::copy_no_clobber(staged, target)?;
                    files::sync_dir(target.parent().ok_or("target needs parent")?)?;
                }
            }
            "replace" => {
                if target != source {
                    return Err("replace target must be the selected workbook".into());
                }
                let p = PathBuf::from(string(plan, "backup_artifact")?);
                if p.parent() != Some(self.paths.state.join("backups").as_path()) {
                    return Err("backup must be in the private backup directory".into());
                }
                let _target_lock = Lock::acquire(
                    &self
                        .paths
                        .state
                        .join("locks")
                        .join(format!("publish-{}.lock", canonical_hash(&json!(source))?)),
                )?;
                let current = files::identify(source)?.sha256;
                if current == staged_hash {
                    if files::identify(&p)?.sha256 != source_hash {
                        return Err("replacement recovery is missing its verified backup".into());
                    }
                } else {
                    if current != source_hash {
                        return Err("source changed before replacement".into());
                    }
                    if exists(&p) {
                        if files::identify(&p)?.sha256 != source_hash {
                            return Err("backup verification failed".into());
                        }
                    } else {
                        files::copy_no_clobber(source, &p)?;
                    }
                    if files::identify(&p)?.sha256 != source_hash {
                        return Err("backup verification failed".into());
                    }
                    if files::identify(source)?.sha256 != source_hash {
                        return Err("source changed while acquiring publication lock".into());
                    }
                    replace(staged, source)?;
                }
                backup = Some(p);
            }
            _ => return Err("unknown publication mode".into()),
        }
        let result = files::identify(target)?.sha256;
        if result != staged_hash {
            return Err("published workbook failed hash verification".into());
        }
        let backup_hash = backup
            .as_ref()
            .map(|p| files::identify(p).map(|i| i.sha256))
            .transpose()?;
        self.receipts.record(json!({"receipt_id":id,"kind":"publish","plan_id":plan["plan_id"],"session_id":plan["session_id"],"revision":plan["revision"],"target_mode":plan["target_mode"],"target":target,"source_sha256":source_hash,"result_sha256":result,"backup":backup,"backup_sha256":backup_hash,"plan_seal":plan["seal"]}))
    }
    pub fn undo(&self, id: &str, token: &str, actor: Actor) -> Result<Value> {
        policy::publish_authority(actor, PublishMode::Replace)?;
        if !files::valid_id(id) || token != format!("UNDO {id}") {
            return Err("undo token did not match the receipt".into());
        }
        let _lock = Lock::acquire(
            &self
                .paths
                .state
                .join("locks")
                .join(format!("undo-{id}.lock")),
        )?;
        let original = self.receipts.get(id)?;
        if original["kind"] != "publish" || original["target_mode"] != "replace" {
            return Err("only replacement receipts can be undone".into());
        }
        let undo_id = format!("undo-{id}");
        if exists(&self.receipts.path(&undo_id)?) {
            return self.receipts.get(&undo_id);
        }
        let target = Path::new(string(&original, "target")?);
        let backup = Path::new(string(&original, "backup")?);
        let _target_lock = Lock::acquire(
            &self
                .paths
                .state
                .join("locks")
                .join(format!("publish-{}.lock", canonical_hash(&json!(target))?)),
        )?;
        if files::identify(target)?.sha256 != string(&original, "result_sha256")? {
            return Err("published workbook changed after the receipt; undo refused".into());
        }
        if files::identify(backup)?.sha256 != string(&original, "backup_sha256")? {
            return Err("backup changed after the receipt; undo refused".into());
        }
        replace(backup, target)?;
        let restored = files::identify(target)?.sha256;
        if restored != string(&original, "source_sha256")? {
            return Err("undo restoration failed hash verification".into());
        }
        self.receipts.record(json!({"receipt_id":undo_id,"kind":"undo","undoes_receipt_id":id,"target":target,"before_sha256":original["result_sha256"],"result_sha256":restored}))
    }
}
