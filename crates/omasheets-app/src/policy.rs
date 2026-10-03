//! Format and human publication authority shared by every transport.
use crate::Result;
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Xls,
    Xlsx,
    Xlsm,
    Ods,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Agent,
    Local,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishMode {
    Copy,
    Replace,
}
pub fn format(path: &Path) -> Result<Format> {
    match path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "xls" => Ok(Format::Xls),
        "xlsx" => Ok(Format::Xlsx),
        "xlsm" => Ok(Format::Xlsm),
        "ods" => Ok(Format::Ods),
        _ => Err("unsupported workbook format".into()),
    }
}
pub fn stageable(path: &Path) -> Result<Format> {
    let f = format(path)?;
    if matches!(f, Format::Xls | Format::Xlsm) {
        Err("this workbook format is read-only; .xls requires local conversion".into())
    } else {
        Ok(f)
    }
}
pub fn publish_authority(actor: Actor, _mode: PublishMode) -> Result<()> {
    if actor == Actor::Agent {
        Err("agents cannot publish workbook bytes".into())
    } else {
        Ok(())
    }
}
pub fn conversion_destination(source: &Path) -> Result<PathBuf> {
    if format(source)? != Format::Xls {
        Err("only .xls inputs use the legacy conversion flow".into())
    } else {
        Ok(source.with_extension("xlsx"))
    }
}
