//! Bounded `.xlsx` import into the owned OmaSheets M0 calculation engine.
//!
//! Date-formatted cells are imported as the raw serial numbers the file stores,
//! matching `omasheets_calc::serial_date`; workbooks that declare the 1904 date
//! system are rejected rather than silently offset by 1462 days.

use calamine::{Cell, CellErrorType, Data, Range, Reader, Xlsx, XlsxFormulaMetadata};
use omasheets_calc::serial_date::DATE_SYSTEM;
use omasheets_calc::{CalcError, CellId, FormulaError, Value, Workbook};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, Write};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportLimits {
    pub max_sheets: usize,
    pub max_cells: usize,
    pub max_formulas: usize,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_sheets: 256,
            max_cells: 2_000_000,
            max_formulas: 1_000_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SheetInfo {
    pub index: u32,
    pub name: String,
}

/// Upper bound on distinct unsupported function names kept in a report, so a
/// hostile workbook cannot inflate the bounded output.
pub const MAX_REPORTED_FUNCTIONS: usize = 128;
const MAX_FUNCTION_NAME_CHARS: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedFormula {
    pub cell: CellId,
    /// The structured compile error, kept so reports can group by kind and by
    /// function name without re-parsing the bounded reason text.
    pub error: FormulaError,
    pub reason: String,
}

impl UnsupportedFormula {
    /// Stable label for the kind of compile failure.
    pub fn kind(&self) -> &'static str {
        formula_error_kind(&self.error)
    }
}

pub fn formula_error_kind(error: &FormulaError) -> &'static str {
    match error {
        FormulaError::Empty => "empty",
        FormulaError::UnexpectedToken(_) => "syntax",
        FormulaError::UnsupportedFunction(_) => "unsupported_function",
        FormulaError::InvalidReference(_) => "invalid_reference",
        FormulaError::UnknownSheet(_) => "unknown_sheet",
        FormulaError::ExternalReference(_) => "external_reference",
        FormulaError::UnknownName(_) => "unknown_name",
        FormulaError::UnknownTable(_)
        | FormulaError::UnknownTableColumn { .. }
        | FormulaError::InvalidStructuredReference(_) => "structured_reference",
        FormulaError::UnsupportedName(_) => "unsupported_name",
        FormulaError::RangeTooLarge => "range_too_large",
        FormulaError::Cycle(_) => "cycle",
    }
}

/// Bounded, serialisable summary of one owned-engine import; the JSON printed
/// by `omasheets-xlsx-score` and embedded per workbook by `omasheets-corpus`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScoreReport {
    pub schema: u8,
    pub engine: String,
    pub date_system: String,
    pub source_sha256: String,
    pub sheets: usize,
    pub formula_cells_observed: usize,
    pub formula_cells_loaded: usize,
    pub formula_cells_compared: usize,
    pub stored_values_matched: usize,
    pub stored_values_mismatched: usize,
    pub unsupported_formulas: usize,
    /// Distinct unsupported function names and how many formula cells named
    /// each, capped at [`MAX_REPORTED_FUNCTIONS`] entries.
    pub unsupported_functions: BTreeMap<String, usize>,
    /// Compile-failure kinds and how many formula cells hit each.
    pub unsupported_reasons: BTreeMap<String, usize>,
    /// Sheet entries without a worksheet part that the importer skipped.
    #[serde(default)]
    pub skipped_sheets: Vec<String>,
}

pub const ENGINE_NAME: &str = "omasheets-owned-m0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParitySummary {
    pub formula_cells_observed: usize,
    pub formula_cells_loaded: usize,
    pub formula_cells_compared: usize,
    pub stored_values_matched: usize,
    pub stored_values_mismatched: usize,
    pub unsupported_formulas: usize,
}

pub struct ImportedWorkbook {
    pub workbook: Workbook,
    pub sheets: Vec<SheetInfo>,
    pub source_sha256: String,
    /// Always `"1900"`: the importer refuses every other date system.
    pub date_system: &'static str,
    pub unsupported: Vec<UnsupportedFormula>,
    /// Sheets named in `xl/workbook.xml` without a worksheet part, skipped by
    /// [`import_xlsx`]'s in-memory repair; empty for well-formed packages.
    pub skipped_sheets: Vec<String>,
    stored_formula_values: Vec<(CellId, Value)>,
    formula_cells_observed: usize,
    formula_cells_loaded: usize,
}

impl ImportedWorkbook {
    /// The compared formula cells whose recalculated value differs from the
    /// stored one, with both values, for tooling that investigates
    /// mismatches; the score report itself stays aggregate.
    pub fn mismatched_cells(&self) -> impl Iterator<Item = (CellId, &Value, Value)> {
        self.stored_formula_values
            .iter()
            .map(|(cell, stored)| (*cell, stored, self.workbook.value(*cell)))
            .filter(|(_, stored, calculated)| !values_match(stored, calculated))
    }

    pub fn parity(&self) -> ParitySummary {
        let stored_values_matched = self
            .stored_formula_values
            .iter()
            .filter(|(cell, stored)| values_match(stored, &self.workbook.value(*cell)))
            .count();
        let formula_cells_compared = self.stored_formula_values.len();
        ParitySummary {
            formula_cells_observed: self.formula_cells_observed,
            formula_cells_loaded: self.formula_cells_loaded,
            formula_cells_compared,
            stored_values_matched,
            stored_values_mismatched: formula_cells_compared - stored_values_matched,
            unsupported_formulas: self.unsupported.len(),
        }
    }

    /// Distinct unsupported function names with formula-cell counts. Names are
    /// truncated and the map is capped so the report stays bounded.
    pub fn unsupported_functions(&self) -> BTreeMap<String, usize> {
        let mut functions = BTreeMap::new();
        for unsupported in &self.unsupported {
            let FormulaError::UnsupportedFunction(name) = &unsupported.error else {
                continue;
            };
            let name: String = name.chars().take(MAX_FUNCTION_NAME_CHARS).collect();
            if functions.len() >= MAX_REPORTED_FUNCTIONS && !functions.contains_key(&name) {
                continue;
            }
            *functions.entry(name).or_insert(0) += 1;
        }
        functions
    }

    /// Compile-failure kinds with formula-cell counts.
    pub fn unsupported_reasons(&self) -> BTreeMap<String, usize> {
        let mut reasons = BTreeMap::new();
        for unsupported in &self.unsupported {
            *reasons.entry(unsupported.kind().to_string()).or_insert(0) += 1;
        }
        reasons
    }

    pub fn report(&self) -> ScoreReport {
        let parity = self.parity();
        ScoreReport {
            schema: 2,
            engine: ENGINE_NAME.into(),
            date_system: self.date_system.into(),
            source_sha256: self.source_sha256.clone(),
            sheets: self.sheets.len(),
            formula_cells_observed: parity.formula_cells_observed,
            formula_cells_loaded: parity.formula_cells_loaded,
            formula_cells_compared: parity.formula_cells_compared,
            stored_values_matched: parity.stored_values_matched,
            stored_values_mismatched: parity.stored_values_mismatched,
            unsupported_formulas: parity.unsupported_formulas,
            unsupported_functions: self.unsupported_functions(),
            unsupported_reasons: self.unsupported_reasons(),
            skipped_sheets: self.skipped_sheets.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportError {
    Open(String),
    Read(String),
    TooManySheets { observed: usize, maximum: usize },
    TooManyCells { observed: usize, maximum: usize },
    TooManyFormulas { observed: usize, maximum: usize },
    UnsupportedDateSystem { observed: &'static str },
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(formatter, "could not open xlsx: {error}"),
            Self::Read(error) => write!(formatter, "could not read xlsx: {error}"),
            Self::TooManySheets { observed, maximum } => {
                write!(
                    formatter,
                    "workbook has {observed} sheets; limit is {maximum}"
                )
            }
            Self::TooManyCells { observed, maximum } => {
                write!(
                    formatter,
                    "workbook spans {observed} cells; limit is {maximum}"
                )
            }
            Self::TooManyFormulas { observed, maximum } => {
                write!(
                    formatter,
                    "workbook has {observed} formulas; limit is {maximum}"
                )
            }
            Self::UnsupportedDateSystem { observed } => {
                write!(
                    formatter,
                    "workbook uses the {observed} date system; only the {DATE_SYSTEM} date system is supported"
                )
            }
        }
    }
}

impl std::error::Error for ImportError {}

pub fn import_xlsx(path: &Path, limits: ImportLimits) -> Result<ImportedWorkbook, ImportError> {
    let source_sha256 = hash_file(path)?;
    let (mut source, skipped_sheets) = open_repaired(path)?;
    check_date_system(source.has_1904_epoch())?;
    let sheet_names = source.sheet_names();
    if sheet_names.len() > limits.max_sheets {
        return Err(ImportError::TooManySheets {
            observed: sheet_names.len(),
            maximum: limits.max_sheets,
        });
    }

    // Read the names from the package part rather than through Calamine,
    // which drops each name's `localSheetId` scope.
    let defined_names = read_defined_names(path)?;
    let mut ranges = Vec::with_capacity(sheet_names.len());
    for name in &sheet_names {
        let values = source
            .worksheet_range(name)
            .map_err(|error| ImportError::Read(error.to_string()))?;
        let formulas = read_formulas(&mut source, name)?;
        ranges.push((name.clone(), values, formulas));
    }
    let mut imported = import_ranges_with_names(ranges, defined_names, source_sha256, limits)?;
    imported.skipped_sheets = skipped_sheets;
    Ok(imported)
}

/// Reads a sheet's formulas, expanding shared formulas from their anchor
/// cell. Calamine's `worksheet_formula` shifts a derived cell from the
/// top-left of the shared `ref` range instead; Excel anchors a group at its
/// first cell, which need not be that corner (a corner cell can carry its own
/// formula), and the corpus has sheets whose derived cells came out shifted
/// by a column as a result. A derived cell whose anchor appears later in the
/// stream is resolved at the end; one whose anchor never appears is skipped.
fn read_formulas<RS: Read + Seek>(
    source: &mut Xlsx<RS>,
    name: &str,
) -> Result<Range<String>, ImportError> {
    let read_error = |error: calamine::XlsxError| ImportError::Read(error.to_string());
    // Chart and dialog sheets have no cells; Calamine's own range readers
    // return an empty range for them and so does this one.
    let mut reader = match source.worksheet_cells_reader(name) {
        Ok(reader) => reader,
        Err(calamine::XlsxError::NotAWorksheet(_)) => return Ok(Range::default()),
        Err(error) => return Err(read_error(error)),
    };
    let mut anchors: HashMap<usize, ((u32, u32), String)> = HashMap::new();
    let mut cells = Vec::new();
    let mut pending = Vec::new();
    while let Some(record) = reader
        .next_cell_with_formula_metadata()
        .map_err(read_error)?
    {
        match record.formula {
            Some(XlsxFormulaMetadata::Normal { formula }) => {
                cells.push(Cell::new(record.pos, formula));
            }
            Some(XlsxFormulaMetadata::Shared {
                shared_index,
                formula,
                ..
            }) => {
                anchors.insert(shared_index, (record.pos, formula.clone()));
                cells.push(Cell::new(record.pos, formula));
            }
            Some(XlsxFormulaMetadata::SharedDerived { shared_index }) => {
                pending.push((record.pos, shared_index));
            }
            _ => {}
        }
    }
    for (position, shared_index) in pending {
        if let Some((anchor, template)) = anchors.get(&shared_index) {
            let formula =
                calamine::expand_shared_formula(template, *anchor, position).map_err(read_error)?;
            cells.push(Cell::new(position, formula));
        }
    }
    Ok(Range::from_sparse(cells))
}

/// Opens a workbook, and when Calamine refuses it because a `<sheet>` entry
/// carries an empty or dangling relationship id, opens an in-memory copy of
/// the package whose `xl/workbook.xml` omits those entries.
///
/// Every such sheet in the frozen Enron sample is a `veryHidden` legacy macro
/// module left behind by an `.xls` conversion: it has no worksheet part, so
/// nothing is lost by skipping it, and the names of the skipped sheets are
/// reported so the omission is never silent. The source file is never
/// modified; the repair exists only in memory for this import.
/// A workbook reader over either the file itself or the in-memory repaired
/// copy, with the names of the sheet entries the repair skipped.
type OpenedWorkbook = (Xlsx<Box<dyn ReadSeek>>, Vec<String>);

fn open_repaired(path: &Path) -> Result<OpenedWorkbook, ImportError> {
    let file = File::open(path).map_err(|error| ImportError::Open(error.to_string()))?;
    match Xlsx::new(Box::new(BufReader::new(file)) as Box<dyn ReadSeek>) {
        Ok(workbook) => Ok((workbook, Vec::new())),
        Err(calamine::XlsxError::RelationshipNotFound) => {
            let (repaired, skipped) = repair_dangling_sheets(path)?;
            if skipped.is_empty() {
                return Err(ImportError::Open("Relationship not found".into()));
            }
            let workbook = Xlsx::new(Box::new(Cursor::new(repaired)) as Box<dyn ReadSeek>)
                .map_err(|error| ImportError::Open(error.to_string()))?;
            Ok((workbook, skipped))
        }
        Err(error) => Err(ImportError::Open(error.to_string())),
    }
}

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// Largest `xl/workbook.xml` the repair will rewrite; real workbook parts are
/// a few kilobytes, and the copy is held in memory.
const MAX_WORKBOOK_PART_BYTES: u64 = 4 * 1024 * 1024;

/// Rebuilds the package without the `<sheet>` entries whose relationship id
/// is empty or absent from `xl/_rels/workbook.xml.rels`, copying every other
/// part byte for byte. Returns the new package and the skipped sheet names.
fn repair_dangling_sheets(path: &Path) -> Result<(Vec<u8>, Vec<String>), ImportError> {
    let file = File::open(path).map_err(|error| ImportError::Open(error.to_string()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| ImportError::Open(error.to_string()))?;
    let relationships = read_part(&mut archive, "xl/_rels/workbook.xml.rels")?;
    let workbook = read_part(&mut archive, "xl/workbook.xml")?;
    let known_ids: std::collections::HashSet<String> =
        attribute_values(&relationships, "Id").into_iter().collect();
    let (rewritten, skipped) = drop_dangling_sheets(&workbook, &known_ids);
    if skipped.is_empty() {
        return Ok((Vec::new(), skipped));
    }

    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|error| ImportError::Open(error.to_string()))?;
        if entry.name() == "xl/workbook.xml" {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            output
                .start_file("xl/workbook.xml", options)
                .and_then(|()| output.write_all(rewritten.as_bytes()).map_err(Into::into))
                .map_err(|error| ImportError::Open(error.to_string()))?;
        } else {
            output
                .raw_copy_file(entry)
                .map_err(|error| ImportError::Open(error.to_string()))?;
        }
    }
    let cursor = output
        .finish()
        .map_err(|error| ImportError::Open(error.to_string()))?;
    Ok((cursor.into_inner(), skipped))
}

/// Values of every `name="…"` attribute in `xml`, in document order. The
/// package parts involved are machine-written, so a lexical scan is enough.
/// Reads one small XML part of the package as text, refusing parts over
/// [`MAX_WORKBOOK_PART_BYTES`].
fn read_part(archive: &mut zip::ZipArchive<File>, name: &str) -> Result<String, ImportError> {
    let mut part = archive
        .by_name(name)
        .map_err(|error| ImportError::Open(format!("{name}: {error}")))?;
    if part.size() > MAX_WORKBOOK_PART_BYTES {
        return Err(ImportError::Open(format!(
            "{name} exceeds the workbook part size limit"
        )));
    }
    let mut text = String::new();
    part.read_to_string(&mut text)
        .map_err(|error| ImportError::Open(format!("{name}: {error}")))?;
    Ok(text)
}

/// A defined name from `xl/workbook.xml`. `sheet` is the name of the scope
/// sheet for a `localSheetId` name and `None` for a workbook-level name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinedName {
    pub sheet: Option<String>,
    pub name: String,
    pub definition: String,
}

/// Reads every `<definedName>` of the workbook part with its scope. A
/// `localSheetId` is an index into the part's own `<sheet>` list, so it is
/// resolved to a sheet name here, before any repair renumbers the sheets; a
/// name whose scope index points past that list is dropped.
fn read_defined_names(path: &Path) -> Result<Vec<DefinedName>, ImportError> {
    let file = File::open(path).map_err(|error| ImportError::Open(error.to_string()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| ImportError::Open(error.to_string()))?;
    let workbook = read_part(&mut archive, "xl/workbook.xml")?;
    Ok(parse_defined_names(&workbook))
}

fn parse_defined_names(workbook_xml: &str) -> Vec<DefinedName> {
    let mut sheets = Vec::new();
    let mut rest = workbook_xml;
    while let Some(start) = rest.find("<sheet ") {
        let Some(length) = rest[start..].find('>') else {
            break;
        };
        let element = &rest[start..start + length];
        if let Some(name) = attribute(element, "name") {
            sheets.push(name);
        }
        rest = &rest[start + length + 1..];
    }
    let mut names = Vec::new();
    let mut rest = workbook_xml;
    while let Some(start) = rest.find("<definedName ") {
        let after_tag = &rest[start..];
        let Some(tag_end) = after_tag.find('>') else {
            break;
        };
        let tag = &after_tag[..tag_end];
        let self_closing = tag.ends_with('/');
        let body_start = tag_end + 1;
        let (definition, consumed) = if self_closing {
            (String::new(), body_start)
        } else {
            match after_tag[body_start..].find("</definedName>") {
                Some(end) => (
                    unescape_xml(&after_tag[body_start..body_start + end]),
                    body_start + end,
                ),
                None => break,
            }
        };
        rest = &after_tag[consumed..];
        let Some(name) = attribute(tag, "name") else {
            continue;
        };
        let sheet = match attribute(tag, "localSheetId") {
            None => None,
            Some(index) => match index.parse::<usize>().ok().and_then(|i| sheets.get(i)) {
                Some(sheet) => Some(sheet.clone()),
                None => continue,
            },
        };
        names.push(DefinedName {
            sheet,
            name,
            definition,
        });
    }
    names
}

/// The value of attribute `name` in one start tag, unescaped.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')?;
    Some(unescape_xml(&tag[start..start + end]))
}

/// Decodes the five XML entities and numeric character references; anything
/// else is kept as written.
fn unescape_xml(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find(';').filter(|end| *end <= 10) else {
            output.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix('#')
                .and_then(|number| match number.strip_prefix('x') {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => {
                output.push(character);
                rest = &after[end + 1..];
            }
            None => {
                output.push('&');
                rest = after;
            }
        }
    }
    output.push_str(rest);
    output
}

fn attribute_values(xml: &str, name: &str) -> Vec<String> {
    let needle = format!("{name}=\"");
    let mut values = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&needle) {
        let after = &rest[start + needle.len()..];
        let Some(end) = after.find('"') else { break };
        values.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    values
}

/// Removes `<sheet …/>` elements whose `r:id` is empty or unknown and returns
/// the rewritten XML with the names of the removed sheets.
fn drop_dangling_sheets(
    workbook_xml: &str,
    known_ids: &std::collections::HashSet<String>,
) -> (String, Vec<String>) {
    let mut output = String::with_capacity(workbook_xml.len());
    let mut skipped = Vec::new();
    let mut rest = workbook_xml;
    while let Some(start) = rest.find("<sheet ") {
        let Some(length) = rest[start..].find("/>") else {
            break;
        };
        let element = &rest[start..start + length + 2];
        let id = attribute_values(element, "r:id").into_iter().next();
        let dangling = match id {
            Some(id) => id.is_empty() || !known_ids.contains(&id),
            None => true,
        };
        output.push_str(&rest[..start]);
        if dangling {
            skipped.push(
                attribute_values(element, "name")
                    .into_iter()
                    .next()
                    .unwrap_or_default(),
            );
        } else {
            output.push_str(element);
        }
        rest = &rest[start + length + 2..];
    }
    output.push_str(rest);
    (output, skipped)
}

fn check_date_system(has_1904_epoch: bool) -> Result<(), ImportError> {
    if has_1904_epoch {
        Err(ImportError::UnsupportedDateSystem { observed: "1904" })
    } else {
        Ok(())
    }
}

fn hash_file(path: &Path) -> Result<String, ImportError> {
    let mut source = File::open(path).map_err(|error| ImportError::Open(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| ImportError::Read(error.to_string()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
fn import_ranges(
    ranges: Vec<(String, Range<Data>, Range<String>)>,
    source_sha256: String,
    limits: ImportLimits,
) -> Result<ImportedWorkbook, ImportError> {
    import_ranges_with_names(ranges, Vec::new(), source_sha256, limits)
}

fn import_ranges_with_names(
    ranges: Vec<(String, Range<Data>, Range<String>)>,
    defined_names: Vec<DefinedName>,
    source_sha256: String,
    limits: ImportLimits,
) -> Result<ImportedWorkbook, ImportError> {
    if ranges.len() > limits.max_sheets {
        return Err(ImportError::TooManySheets {
            observed: ranges.len(),
            maximum: limits.max_sheets,
        });
    }
    let mut observed_cells = 0_usize;
    let mut observed_formulas = 0_usize;
    for (_, values, formulas) in &ranges {
        observed_cells =
            observed_cells.saturating_add(values.width().saturating_mul(values.height()));
        observed_formulas = observed_formulas.saturating_add(
            formulas
                .used_cells()
                .filter(|(_, _, formula)| !formula.is_empty())
                .count(),
        );
    }
    if observed_cells > limits.max_cells {
        return Err(ImportError::TooManyCells {
            observed: observed_cells,
            maximum: limits.max_cells,
        });
    }
    if observed_formulas > limits.max_formulas {
        return Err(ImportError::TooManyFormulas {
            observed: observed_formulas,
            maximum: limits.max_formulas,
        });
    }

    let sheets: Vec<SheetInfo> = ranges
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| SheetInfo {
            index: index as u32,
            name: name.clone(),
        })
        .collect();
    let mut workbook = Workbook::default();
    // One recalculation for the whole import instead of one per cell.
    workbook.begin_bulk();
    for sheet in &sheets {
        workbook.define_sheet(sheet.index, sheet.name.clone());
    }
    let sheet_indices: HashMap<&str, u32> = sheets
        .iter()
        .map(|sheet| (sheet.name.as_str(), sheet.index))
        .collect();
    for name in defined_names {
        match name.sheet {
            None => workbook.define_name(name.name, name.definition),
            // A scope naming a sheet the repair skipped goes with that sheet.
            Some(scope) => {
                if let Some(index) = sheet_indices.get(scope.as_str()) {
                    workbook.define_sheet_name(*index, name.name, name.definition);
                }
            }
        }
    }
    let mut formula_records = Vec::with_capacity(observed_formulas);

    for (sheet, (_, values, formulas)) in ranges.iter().enumerate() {
        let (value_row, value_column) = values.start().unwrap_or((0, 0));
        for (row, column, value) in values.used_cells() {
            let cell = CellId::new(
                sheet as u32,
                value_row + row as u32,
                value_column + column as u32,
            );
            set_source_value(&mut workbook, cell, value);
        }
        let (formula_row, formula_column) = formulas.start().unwrap_or((0, 0));
        for (row, column, formula) in formulas.used_cells() {
            if formula.is_empty() {
                continue;
            }
            let absolute = (formula_row + row as u32, formula_column + column as u32);
            let cell = CellId::new(sheet as u32, absolute.0, absolute.1);
            let stored = values
                .get_value(absolute)
                .map(source_value)
                .unwrap_or(Value::Blank);
            formula_records.push((cell, stored, formula.clone()));
        }
    }

    let mut unsupported = Vec::new();
    let mut stored_formula_values = Vec::new();
    for (cell, stored, formula) in formula_records {
        match workbook.set_formula(cell, &formula) {
            Ok(_) => stored_formula_values.push((cell, stored)),
            Err(error) => unsupported.push(UnsupportedFormula {
                cell,
                reason: bounded_formula_error(&error),
                error,
            }),
        }
    }
    let formula_cells_loaded = stored_formula_values.len();
    workbook.end_bulk();
    Ok(ImportedWorkbook {
        workbook,
        sheets,
        source_sha256,
        date_system: DATE_SYSTEM,
        unsupported,
        skipped_sheets: Vec::new(),
        stored_formula_values,
        formula_cells_observed: observed_formulas,
        formula_cells_loaded,
    })
}

fn set_source_value(workbook: &mut Workbook, cell: CellId, value: &Data) {
    match value {
        Data::Int(value) => {
            workbook.set_number(cell, *value as f64);
        }
        Data::Float(value) => {
            workbook.set_number(cell, *value);
        }
        Data::Bool(value) => {
            workbook.set_boolean(cell, *value);
        }
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => {
            workbook.set_text(cell, value.clone());
        }
        Data::DateTime(value) => {
            // The raw 1900-system serial; `check_date_system` has already
            // rejected 1904 workbooks, so no epoch shift is applied.
            workbook.set_number(cell, value.as_f64());
        }
        Data::Error(error) => {
            workbook.set_error(cell, source_error(error));
        }
        Data::Empty => {
            workbook.clear(cell);
        }
    }
}

fn source_error(error: &CellErrorType) -> CalcError {
    match error {
        CellErrorType::Div0 => CalcError::DivisionByZero,
        CellErrorType::NA => CalcError::NotAvailable,
        CellErrorType::Name => CalcError::InvalidName,
        CellErrorType::Null => CalcError::NullIntersection,
        CellErrorType::Num => CalcError::InvalidNumber,
        CellErrorType::Ref => CalcError::InvalidReference,
        CellErrorType::Value | CellErrorType::GettingData => CalcError::InvalidValue,
    }
}

fn source_value(value: &Data) -> Value {
    match value {
        Data::Int(value) => Value::Number(*value as f64),
        Data::Float(value) => Value::Number(*value),
        Data::Bool(value) => Value::Boolean(*value),
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => {
            Value::Text(value.clone())
        }
        Data::DateTime(value) => Value::Number(value.as_f64()),
        Data::Error(error) => Value::Error(source_error(error)),
        Data::Empty => Value::Blank,
    }
}

fn bounded_formula_error(error: &FormulaError) -> String {
    error.to_string().chars().take(256).collect()
}

fn values_match(stored: &Value, calculated: &Value) -> bool {
    match (stored, calculated) {
        (Value::Number(stored), Value::Number(calculated)) => {
            stored.is_finite()
                && calculated.is_finite()
                && (stored - calculated).abs() <= 1e-9 * stored.abs().max(1.0)
        }
        _ => stored == calculated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workbook_name(name: &str, definition: &str) -> DefinedName {
        DefinedName {
            sheet: None,
            name: name.into(),
            definition: definition.into(),
        }
    }

    /// A minimal package: one real worksheet plus `<sheet>` entries that
    /// point nowhere, the shape left behind by converted legacy macro sheets.
    fn package_with_dangling_sheets(dangling: &[(&str, &str)]) -> Vec<u8> {
        package(
            dangling,
            r#"<definedName name="Total">Data!$A$3</definedName>"#,
            "",
            "",
        )
    }

    /// A minimal package with the given dangling `<sheet>` entries, the given
    /// `<definedNames>` body, extra `<c>` cells appended to row 1 and extra
    /// `<row>` elements appended after row 3.
    fn package(
        dangling: &[(&str, &str)],
        defined_names: &str,
        extra_cells: &str,
        extra_rows: &str,
    ) -> Vec<u8> {
        package_with_chartsheet(dangling, defined_names, extra_cells, extra_rows, false)
    }

    /// As [`package`], optionally with a chartsheet named `Chart` after the
    /// worksheet.
    fn package_with_chartsheet(
        dangling: &[(&str, &str)],
        defined_names: &str,
        extra_cells: &str,
        extra_rows: &str,
        chartsheet: bool,
    ) -> Vec<u8> {
        let chart_sheet_entry = if chartsheet {
            r#"<sheet name="Chart" sheetId="2" r:id="rId2"/>"#
        } else {
            ""
        };
        let chart_relationship = if chartsheet {
            r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet" Target="chartsheets/sheet1.xml"/>"#
        } else {
            ""
        };
        let chart_override = if chartsheet {
            r#"<Override PartName="/xl/chartsheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.chartsheet+xml"/>"#
        } else {
            ""
        };
        let sheets: String = dangling
            .iter()
            .map(|(name, id)| {
                format!(r#"<sheet name="{name}" sheetId="9" state="veryHidden" r:id="{id}"/>"#)
            })
            .collect();
        let parts = [
            (
                "[Content_Types].xml",
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>{chart_override}</Types>"#
                ),
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
            ),
            (
                "xl/workbook.xml",
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/>{chart_sheet_entry}{sheets}</sheets><definedNames>{defined_names}</definedNames></workbook>"#
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>{chart_relationship}</Relationships>"#
                ),
            ),
            (
                "xl/chartsheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><chartsheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetPr/><sheetViews><sheetView workbookViewId="0"/></sheetViews></chartsheet>"#.to_string(),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>2</v></c>{extra_cells}</row><row r="2"><c r="A2"><v>3</v></c></row><row r="3"><c r="A3"><f>A1+A2</f><v>5</v></c></row>{extra_rows}</sheetData></worksheet>"#
                ),
            ),
        ];
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in parts {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn temporary_xlsx(bytes: &[u8]) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "omasheets-xlsx-{}-{nonce}.xlsx",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn defined_names_keep_their_sheet_scope_and_entities() {
        let xml = r#"<definedName name="Total">Data!$A$3</definedName><definedName name="Total" localSheetId="0" hidden="1">Data!$A$1</definedName><definedName name="Joined" comment="a &amp; b">Data!$A$1&amp;"x"</definedName><definedName name="Orphan" localSheetId="7">Data!$A$1</definedName><definedName name="Empty"/><definedName name="Quoted">'P &amp; L'!$B$2</definedName>"#;
        assert_eq!(
            parse_defined_names(&format!(
                r#"<workbook><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="P &amp; L" sheetId="2" r:id="rId2"/></sheets><definedNames>{xml}</definedNames></workbook>"#
            )),
            vec![
                workbook_name("Total", "Data!$A$3"),
                DefinedName {
                    sheet: Some("Data".into()),
                    name: "Total".into(),
                    definition: "Data!$A$1".into(),
                },
                workbook_name("Joined", "Data!$A$1&\"x\""),
                workbook_name("Empty", ""),
                workbook_name("Quoted", "'P & L'!$B$2"),
            ]
        );
        assert_eq!(unescape_xml("&lt;&#65;&#x42;&bogus;&amp"), "<AB&bogus;&amp");

        // On the sheet, the scoped `Total` (A1 = 2) wins over the workbook
        // `Total` (A3 = 5): B1 stores 20, as Excel computed it.
        let path = temporary_xlsx(&package(
            &[],
            xml,
            r#"<c r="B1"><f>Total*10</f><v>20</v></c><c r="C1" t="str"><f>Joined</f><v>2x</v></c>"#,
            "",
        ));
        let imported = import_xlsx(&path, ImportLimits::default()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 1)),
            Value::Number(20.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 2)),
            Value::Text("2x".into())
        );
        assert_eq!(imported.parity().stored_values_matched, 3);
        assert_eq!(imported.parity().stored_values_mismatched, 0);
    }

    #[test]
    fn shared_formulas_expand_from_their_anchor_cell() {
        // The shared group is anchored at B5 with ref A5:B6; A5 carries its
        // own formula. A6 is therefore B5's template shifted one row down
        // and one column left (A5*2 = 140), not the ref corner's (B5*2 = 44).
        let rows = r#"<row r="4"><c r="A4"><v>7</v></c><c r="B4"><v>11</v></c></row><row r="5"><c r="A5"><f>A4*10</f><v>70</v></c><c r="B5"><f t="shared" ref="A5:B6" si="0">B4*2</f><v>22</v></c></row><row r="6"><c r="A6"><f t="shared" si="0"/><v>140</v></c><c r="B6"><f t="shared" si="0"/><v>44</v></c></row>"#;
        let path = temporary_xlsx(&package(
            &[],
            r#"<definedName name="Total">Data!$A$3</definedName>"#,
            "",
            rows,
        ));
        let imported = import_xlsx(&path, ImportLimits::default()).unwrap();
        std::fs::remove_file(&path).unwrap();
        for (cell, expected) in [
            (CellId::new(0, 4, 0), 70.0),
            (CellId::new(0, 4, 1), 22.0),
            (CellId::new(0, 5, 0), 140.0),
            (CellId::new(0, 5, 1), 44.0),
        ] {
            assert_eq!(imported.workbook.value(cell), Value::Number(expected));
        }
        assert_eq!(imported.parity().formula_cells_loaded, 5);
        assert_eq!(imported.parity().stored_values_matched, 5);
        assert_eq!(imported.parity().stored_values_mismatched, 0);
    }

    #[test]
    fn chartsheets_import_as_empty_sheets() {
        let path = temporary_xlsx(&package_with_chartsheet(
            &[],
            r#"<definedName name="Total">Data!$A$3</definedName>"#,
            "",
            "",
            true,
        ));
        let imported = import_xlsx(&path, ImportLimits::default()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(imported.sheets.len(), 2);
        assert_eq!(imported.sheets[1].name, "Chart");
        assert_eq!(imported.parity().formula_cells_loaded, 1);
        assert_eq!(imported.parity().stored_values_matched, 1);
    }

    #[test]
    fn dangling_sheet_entries_are_skipped_in_memory_and_reported() {
        let path = temporary_xlsx(&package_with_dangling_sheets(&[
            ("Module1", ""),
            ("Code", "rId7"),
        ]));
        let imported = import_xlsx(&path, ImportLimits::default()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(imported.sheets.len(), 1);
        assert_eq!(imported.skipped_sheets, vec!["Module1", "Code"]);
        assert_eq!(
            imported.workbook.value(CellId::new(0, 2, 0)),
            Value::Number(5.0)
        );
        let report = imported.report();
        assert_eq!(report.skipped_sheets, vec!["Module1", "Code"]);
        assert_eq!(report.stored_values_matched, 1);
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"skipped_sheets\":[\"Module1\",\"Code\"]"));
    }

    #[test]
    fn well_formed_packages_are_not_rewritten() {
        let path = temporary_xlsx(&package_with_dangling_sheets(&[]));
        let imported = import_xlsx(&path, ImportLimits::default()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(imported.skipped_sheets.is_empty());
        assert_eq!(imported.report().skipped_sheets, Vec::<String>::new());
    }

    #[test]
    fn dropping_dangling_sheets_keeps_every_other_byte() {
        let known: std::collections::HashSet<String> = ["rId1".to_string()].into_iter().collect();
        let xml = r#"<sheets><sheet name="A" sheetId="1" r:id="rId1"/><sheet name="M" sheetId="2" state="veryHidden" r:id=""/><sheet name="N" sheetId="3" r:id="rId9"/></sheets><definedNames/>"#;
        let (rewritten, skipped) = drop_dangling_sheets(xml, &known);
        assert_eq!(skipped, vec!["M", "N"]);
        assert_eq!(
            rewritten,
            r#"<sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets><definedNames/>"#
        );
        assert_eq!(attribute_values(xml, "name"), vec!["A", "M", "N"]);
        let (unchanged, none) =
            drop_dangling_sheets(r#"<sheets><sheet name="A" r:id="rId1"/></sheets>"#, &known);
        assert!(none.is_empty());
        assert_eq!(
            unchanged,
            r#"<sheets><sheet name="A" r:id="rId1"/></sheets>"#
        );
    }
    use calamine::{Cell, ExcelDateTime, ExcelDateTimeType};

    fn date_cell(row: u32, column: u32, serial: f64) -> Cell<Data> {
        Cell::new(
            (row, column),
            Data::DateTime(ExcelDateTime::new(
                serial,
                ExcelDateTimeType::DateTime,
                false,
            )),
        )
    }

    fn ranges(
        values: Vec<Cell<Data>>,
        formulas: Vec<Cell<String>>,
    ) -> Vec<(String, Range<Data>, Range<String>)> {
        vec![(
            "Sheet1".into(),
            Range::from_sparse(values),
            Range::from_sparse(formulas),
        )]
    }

    #[test]
    fn imports_formulas_and_compares_calculated_values_with_cached_values() {
        let imported = import_ranges(
            ranges(
                vec![
                    Cell::new((0, 0), Data::Int(2)),
                    Cell::new((1, 0), Data::Int(3)),
                    Cell::new((2, 0), Data::Int(5)),
                ],
                vec![Cell::new((2, 0), "SUM(A1:A2)".into())],
            ),
            "a".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();

        assert_eq!(imported.sheets[0].name, "Sheet1");
        assert_eq!(
            imported.workbook.value(CellId::new(0, 2, 0)),
            Value::Number(5.0)
        );
        assert_eq!(
            imported.parity(),
            ParitySummary {
                formula_cells_observed: 1,
                formula_cells_loaded: 1,
                formula_cells_compared: 1,
                stored_values_matched: 1,
                stored_values_mismatched: 0,
                unsupported_formulas: 0,
            }
        );
    }

    #[test]
    fn keeps_cached_values_when_formulas_are_unsupported() {
        let imported = import_ranges(
            ranges(
                vec![Cell::new((0, 0), Data::Int(2))],
                vec![Cell::new((0, 0), "CUBEVALUE(1,2,3)".into())],
            ),
            "b".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();

        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(2.0)
        );
        assert_eq!(imported.unsupported.len(), 1);
        assert_eq!(imported.parity().formula_cells_compared, 0);
        assert_eq!(
            imported.unsupported[0].error,
            FormulaError::UnsupportedFunction("CUBEVALUE".into())
        );
        assert_eq!(imported.unsupported[0].kind(), "unsupported_function");
    }

    #[test]
    fn reports_bounded_unsupported_function_and_reason_distributions() {
        let imported = import_ranges(
            ranges(
                vec![Cell::new((0, 0), Data::Int(1))],
                vec![
                    Cell::new((0, 1), "TODAY()".into()),
                    Cell::new((0, 2), "today()+1".into()),
                    Cell::new((0, 3), "OFFSET(A1,1,1)".into()),
                    Cell::new((0, 4), "1+".into()),
                    Cell::new((0, 5), "Missing!A1".into()),
                    Cell::new((0, 6), "A1+1".into()),
                ],
            ),
            "i".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();
        let report = imported.report();
        assert_eq!(report.schema, 2);
        assert_eq!(report.engine, ENGINE_NAME);
        assert_eq!(report.date_system, "1900");
        assert_eq!(report.formula_cells_observed, 6);
        assert_eq!(report.formula_cells_loaded, 1);
        assert_eq!(report.unsupported_formulas, 5);
        assert_eq!(
            report.unsupported_functions,
            BTreeMap::from([("TODAY".to_string(), 2), ("OFFSET".to_string(), 1)])
        );
        assert_eq!(
            report.unsupported_reasons,
            BTreeMap::from([
                ("unsupported_function".to_string(), 3),
                ("syntax".to_string(), 1),
                ("unknown_sheet".to_string(), 1),
            ])
        );
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.starts_with("{\"schema\":2,\"engine\":\"omasheets-owned-m0\""));
        assert_eq!(serde_json::from_str::<ScoreReport>(&json).unwrap(), report);
    }

    #[test]
    fn rejects_large_used_ranges_before_materialising_cells() {
        let error = import_ranges(
            ranges(
                vec![
                    Cell::new((0, 0), Data::Int(1)),
                    Cell::new((10, 10), Data::Int(2)),
                ],
                vec![],
            ),
            "c".repeat(64),
            ImportLimits {
                max_cells: 100,
                ..ImportLimits::default()
            },
        )
        .err()
        .expect("range should be rejected");
        assert_eq!(
            error,
            ImportError::TooManyCells {
                observed: 121,
                maximum: 100,
            }
        );
    }

    #[test]
    fn preserves_absolute_coordinates_for_offset_ranges() {
        let imported = import_ranges(
            ranges(
                vec![
                    Cell::new((2, 2), Data::Int(1)),
                    Cell::new((4, 2), Data::Int(2)),
                ],
                vec![Cell::new((4, 2), "C3+1".into())],
            ),
            "d".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(0, 4, 2)),
            Value::Number(2.0)
        );
        assert_eq!(imported.parity().stored_values_matched, 1);
    }

    #[test]
    fn rejects_formula_counts_before_loading_the_owned_graph() {
        let error = import_ranges(
            ranges(
                vec![Cell::new((0, 0), Data::Int(1))],
                vec![Cell::new((0, 0), "1+1".into())],
            ),
            "e".repeat(64),
            ImportLimits {
                max_formulas: 0,
                ..ImportLimits::default()
            },
        )
        .err()
        .expect("formula count should be rejected");
        assert_eq!(
            error,
            ImportError::TooManyFormulas {
                observed: 1,
                maximum: 0,
            }
        );
    }

    #[test]
    fn imports_date_cells_as_serials_and_matches_stored_date_formula_values() {
        let imported = import_ranges(
            ranges(
                vec![
                    date_cell(0, 0, 45_322.0), // 2024-01-31
                    Cell::new((0, 1), Data::Int(2024)),
                    Cell::new((0, 2), Data::Int(1)),
                    Cell::new((0, 3), Data::Int(31)),
                    date_cell(0, 4, 45_351.0), // EDATE clamps to 2024-02-29
                    date_cell(0, 5, 45_351.0),
                    date_cell(0, 6, 45_322.0),
                    Cell::new((0, 7), Data::Int(4)), // Wednesday
                    date_cell(1, 0, 60.0),           // the fictitious 1900-02-29
                    Cell::new((1, 1), Data::Int(1900)),
                    Cell::new((1, 2), Data::Int(2)),
                    Cell::new((1, 3), Data::Int(29)),
                ],
                vec![
                    Cell::new((0, 1), "YEAR(A1)".into()),
                    Cell::new((0, 2), "MONTH(A1)".into()),
                    Cell::new((0, 3), "DAY(A1)".into()),
                    Cell::new((0, 4), "EDATE(A1,1)".into()),
                    Cell::new((0, 5), "EOMONTH(A1,1)".into()),
                    Cell::new((0, 6), "DATE(B1,C1,D1)".into()),
                    Cell::new((0, 7), "WEEKDAY(A1)".into()),
                    Cell::new((1, 1), "YEAR(A2)".into()),
                    Cell::new((1, 2), "MONTH(A2)".into()),
                    Cell::new((1, 3), "DAY(A2)".into()),
                ],
            ),
            "g".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();

        assert_eq!(imported.date_system, "1900");
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(45_322.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 6)),
            Value::Number(45_322.0)
        );
        assert_eq!(
            imported.parity(),
            ParitySummary {
                formula_cells_observed: 10,
                formula_cells_loaded: 10,
                formula_cells_compared: 10,
                stored_values_matched: 10,
                stored_values_mismatched: 0,
                unsupported_formulas: 0,
            }
        );
    }

    #[test]
    fn matches_stored_text_and_boolean_formula_results() {
        let imported = import_ranges(
            ranges(
                vec![
                    Cell::new((0, 0), Data::Int(3)),
                    Cell::new((0, 1), Data::Int(9)),
                    Cell::new((0, 2), Data::String("3|9".into())),
                    Cell::new((0, 3), Data::Bool(false)),
                    Cell::new((0, 4), Data::Float(0.03)),
                    Cell::new((0, 5), Data::Bool(true)),
                ],
                vec![
                    Cell::new((0, 1), "A1^2".into()),
                    Cell::new((0, 2), "A1&\"|\"&B1".into()),
                    Cell::new((0, 3), "ISBLANK(C1)".into()),
                    Cell::new((0, 4), "A1%".into()),
                    Cell::new((0, 5), "ISTEXT(C1)".into()),
                ],
            ),
            "h".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 2)),
            Value::Text("3|9".into())
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 3)),
            Value::Boolean(false)
        );
        assert_eq!(imported.parity().stored_values_matched, 5);
        assert_eq!(imported.parity().stored_values_mismatched, 0);
    }

    #[test]
    fn maps_source_errors_and_defined_names_into_the_owned_engine() {
        let imported = import_ranges_with_names(
            vec![(
                "Data".into(),
                Range::from_sparse(vec![
                    Cell::new((0, 0), Data::Int(10)),
                    Cell::new((1, 0), Data::Int(20)),
                    Cell::new((0, 1), Data::Error(CellErrorType::NA)),
                    Cell::new((0, 2), Data::Int(30)),
                    Cell::new((1, 2), Data::Error(CellErrorType::NA)),
                    Cell::new((2, 2), Data::Error(CellErrorType::Ref)),
                    Cell::new((3, 2), Data::Int(2)),
                ]),
                Range::from_sparse(vec![
                    Cell::new((0, 2), "SUM(Rates)".into()),
                    Cell::new((1, 2), "B1*2".into()),
                    Cell::new((2, 2), "#REF!+1".into()),
                    Cell::new((3, 2), "Missing+1".into()),
                    Cell::new((4, 2), "[1]Other!A1".into()),
                    Cell::new((5, 2), "Broken".into()),
                ]),
            )],
            vec![
                workbook_name("Rates", "Data!$A$1:$A$2"),
                workbook_name("Broken", "[2]External!A1"),
            ],
            "j".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 2)),
            Value::Number(30.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 1, 2)),
            Value::Error(CalcError::NotAvailable)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 2, 2)),
            Value::Error(CalcError::InvalidReference)
        );
        let parity = imported.parity();
        assert_eq!(parity.formula_cells_loaded, 3);
        assert_eq!(parity.stored_values_matched, 3);
        assert_eq!(
            imported.report().unsupported_reasons,
            BTreeMap::from([
                ("unknown_name".to_string(), 1),
                ("external_reference".to_string(), 1),
                ("unsupported_name".to_string(), 1),
            ])
        );
    }

    #[test]
    fn rejects_the_1904_date_system_before_reading_any_cell() {
        assert_eq!(check_date_system(false), Ok(()));
        let error = check_date_system(true).unwrap_err();
        assert_eq!(
            error,
            ImportError::UnsupportedDateSystem { observed: "1904" }
        );
        assert_eq!(
            error.to_string(),
            "workbook uses the 1904 date system; only the 1900 date system is supported"
        );
    }

    #[test]
    fn registers_sheet_names_before_compiling_cross_sheet_formulas() {
        let imported = import_ranges(
            vec![
                (
                    "Inputs".into(),
                    Range::from_sparse(vec![Cell::new((0, 0), Data::Int(2))]),
                    Range::empty(),
                ),
                (
                    "Summary".into(),
                    Range::from_sparse(vec![Cell::new((0, 0), Data::Int(4))]),
                    Range::from_sparse(vec![Cell::new((0, 0), "Inputs!A1*2".into())]),
                ),
            ],
            "f".repeat(64),
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(1, 0, 0)),
            Value::Number(4.0)
        );
        assert_eq!(imported.parity().stored_values_matched, 1);
        assert!(imported.unsupported.is_empty());
    }
}
