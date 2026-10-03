//! Bounded explanation metadata: every proposal operation belongs to one group.
use crate::{Result, files::valid_id};
use serde_json::{Value, json};
use std::collections::HashSet;
fn text(v: &Value, max: usize) -> Result<String> {
    let s = v.as_str().ok_or("workflow text must be a string")?;
    if s.trim().is_empty()
        || s.chars().count() > max
        || s.chars().any(|c| c < ' ' && c != '\n' && c != '\t')
    {
        return Err("workflow text is empty, too long or contains control characters".into());
    }
    Ok(s.trim().into())
}
pub fn validate(v: &Value, count: usize) -> Result<Value> {
    let m = v
        .as_object()
        .ok_or("an agent plan requires workflow context")?;
    if m.keys().any(|k| {
        !["goal", "summary", "assumptions", "evidence_ids", "groups"].contains(&k.as_str())
    }) || ["goal", "summary", "evidence_ids", "groups"]
        .iter()
        .any(|k| !m.contains_key(*k))
    {
        return Err("workflow has invalid fields".into());
    }
    let assumptions = match m.get("assumptions") {
        None => vec![],
        Some(a) => {
            let a = a.as_array().ok_or("invalid workflow assumptions")?;
            if a.len() > 20 {
                return Err("too many workflow assumptions".into());
            }
            a.iter().map(|a| text(a, 500)).collect::<Result<Vec<_>>>()?
        }
    };
    let ids = v["evidence_ids"]
        .as_array()
        .ok_or("invalid workflow evidence")?;
    if !(1..=50).contains(&ids.len()) {
        return Err("workflow must cite between 1 and 50 observations".into());
    }
    let mut seen = HashSet::new();
    for id in ids {
        let id = id.as_str().ok_or("invalid evidence identifier")?;
        if !valid_id(id) || !seen.insert(id) {
            return Err("invalid or duplicate evidence identifier".into());
        }
    }
    let groups = v["groups"].as_array().ok_or("invalid workflow groups")?;
    if groups.is_empty() || groups.len() > 20.min(count) {
        return Err("workflow must group every operation by purpose".into());
    }
    let mut covered = vec![];
    let mut output = vec![];
    for group in groups {
        let m = group.as_object().ok_or("invalid workflow group")?;
        if m.len() != 3
            || ["title", "purpose", "operation_indexes"]
                .iter()
                .any(|k| !m.contains_key(*k))
        {
            return Err("workflow group has invalid fields".into());
        }
        let indexes = group["operation_indexes"]
            .as_array()
            .ok_or("invalid operation indexes")?;
        if indexes.is_empty() {
            return Err("empty operation group".into());
        }
        for i in indexes {
            let i = i.as_u64().ok_or("invalid operation index")? as usize;
            if i >= count {
                return Err("operation index is out of bounds".into());
            }
            covered.push(i);
        }
        output.push(json!({"title":text(&group["title"],120)?,"purpose":text(&group["purpose"],500)?,"operation_indexes":indexes}));
    }
    covered.sort_unstable();
    if covered != (0..count).collect::<Vec<_>>() {
        return Err("workflow groups must cover every operation exactly once".into());
    }
    Ok(
        json!({"goal":text(&v["goal"],1000)?,"summary":text(&v["summary"],2000)?,"assumptions":assumptions,"evidence_ids":ids,"groups":output}),
    )
}
