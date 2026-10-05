//! Bounded `.xlsx` import into the owned OmaSheets M0 calculation engine.
//!
//! Date-formatted cells are imported as the raw serial numbers the file stores,
//! matching `omasheets_calc::serial_date`; workbooks that declare the 1904 date
//! system are rejected rather than silently offset by 1462 days.

#[cfg(test)]
use calamine::Range;
use calamine::{Cell, CellErrorType, Data, DataType, Reader, Xlsx, XlsxFormulaMetadata};
use omasheets_calc::serial_date::DATE_SYSTEM;
use omasheets_calc::{CalcError, CellId, FormulaError, Value, Workbook};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

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
    /// Last occupied source row plus one, including formula-only cells.
    pub rows: usize,
    /// Last occupied source column plus one, including formula-only cells.
    pub columns: usize,
}

/// One occupied source cell, retained for bounded conversion into the native
/// event model. `stored` is the cached workbook value; `formula` is present
/// even when the owned calculation engine cannot compile it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedCell {
    pub cell: CellId,
    pub stored: Value,
    pub formula: Option<String>,
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
    /// Syntax failures grouped by a fixed token class, never formula text.
    #[serde(default)]
    pub syntax_failure_tokens: BTreeMap<String, usize>,
    /// Fixed failure classes, with no source references or cell values.
    #[serde(default)]
    pub reference_failure_kinds: BTreeMap<String, usize>,
    #[serde(default)]
    pub mismatch_value_kinds: BTreeMap<String, usize>,
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
    source_cells: Vec<ImportedCell>,
    compiled_cells: Vec<usize>,
    formula_cells_observed: usize,
    formula_cells_loaded: usize,
}

impl ImportedWorkbook {
    /// Occupied source cells in sheet/row/column order. This projection is
    /// bounded by the same import limits as the calculation workbook.
    pub fn source_cells(&self) -> &[ImportedCell] {
        &self.source_cells
    }

    /// The compared formula cells whose recalculated value differs from the
    /// stored one, with both values, for tooling that investigates
    /// mismatches; the score report itself stays aggregate.
    pub fn mismatched_cells(&self) -> impl Iterator<Item = (CellId, &Value, Value)> {
        self.compiled_cells
            .iter()
            .map(|index| &self.source_cells[*index])
            .map(|source| {
                (
                    source.cell,
                    &source.stored,
                    self.workbook.value(source.cell),
                )
            })
            .filter(|(_, stored, calculated)| !values_match(stored, calculated))
    }

    pub fn parity(&self) -> ParitySummary {
        let stored_values_matched = self
            .compiled_cells
            .iter()
            .map(|index| &self.source_cells[*index])
            .filter(|source| values_match(&source.stored, &self.workbook.value(source.cell)))
            .count();
        let formula_cells_compared = self.compiled_cells.len();
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
        let mut reference_failure_kinds = BTreeMap::new();
        for failure in &self.unsupported {
            if let FormulaError::InvalidReference(reference) = &failure.error {
                let kind = match reference.as_str() {
                    "range endpoint is number" => "numeric_range_endpoint",
                    "range endpoint is function" => "dynamic_range_endpoint",
                    "range endpoints must be references" => "non_reference_endpoint",
                    "range endpoints cross sheets" => "cross_sheet_range",
                    "" => "empty_reference",
                    token if token.bytes().all(|b| b.is_ascii_alphabetic() || b == b'$') => {
                        "column_without_row"
                    }
                    token if token.bytes().all(|b| b.is_ascii_digit() || b == b'$') => {
                        "row_without_column"
                    }
                    _ => "invalid_a1",
                };
                *reference_failure_kinds.entry(kind.to_string()).or_insert(0) += 1;
            }
        }
        let mut mismatch_value_kinds = BTreeMap::new();
        for (_, stored, calculated) in self.mismatched_cells() {
            let kind = format!("{} -> {}", value_kind(stored), value_kind(&calculated));
            *mismatch_value_kinds.entry(kind).or_insert(0) += 1;
        }
        let mut syntax_failure_tokens = BTreeMap::new();
        for failure in &self.unsupported {
            let FormulaError::UnexpectedToken(offset) = failure.error else {
                continue;
            };
            let Ok(index) = self
                .source_cells
                .binary_search_by_key(&failure.cell, |cell| cell.cell)
            else {
                continue;
            };
            let Some(source) = self.source_cells[index].formula.as_deref() else {
                continue;
            };
            let source = source.strip_prefix('=').unwrap_or(source);
            let token = match source.as_bytes().get(offset) {
                Some(b'{') | Some(b'}') => "array_brace",
                Some(b':') => "range_colon",
                Some(b',') => "comma",
                Some(b';') => "semicolon",
                Some(b'!') => "sheet_separator",
                Some(b'[') | Some(b']') => "square_bracket",
                Some(b'@') => "implicit_intersection",
                Some(b'\\') => "backslash_identifier",
                Some(b'(') | Some(b')') => "parenthesis",
                Some(byte) if !byte.is_ascii() => "non_ascii_identifier",
                Some(byte) if byte.is_ascii_alphabetic() => "identifier",
                None => "end_of_formula",
                _ => "other",
            };
            *syntax_failure_tokens.entry(token.to_string()).or_insert(0) += 1;
        }
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
            syntax_failure_tokens,
            reference_failure_kinds,
            mismatch_value_kinds,
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
                    "workbook has {observed} occupied cells; limit is {maximum}"
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

/// How many workbooks may be open along one external-link chain. One hop is
/// required; the cap stops a cycle or a long chain from recursing without
/// bound. A workbook already on the chain is not opened again.
const MAX_EXTERNAL_WORKBOOKS: usize = 8;

pub fn import_xlsx(path: &Path, limits: ImportLimits) -> Result<ImportedWorkbook, ImportError> {
    let mut opening = HashSet::new();
    let mut used_cells = 0_usize;
    import_xlsx_inner(path, limits, &mut opening, &mut used_cells)
}

fn import_xlsx_inner(
    path: &Path,
    limits: ImportLimits,
    opening: &mut HashSet<PathBuf>,
    used_cells: &mut usize,
) -> Result<ImportedWorkbook, ImportError> {
    if opening.len() >= MAX_EXTERNAL_WORKBOOKS {
        return Err(ImportError::Open(
            "external workbook chain is too deep".into(),
        ));
    }
    let key = canonical_key(path);
    if !opening.insert(key.clone()) {
        return Err(ImportError::Open("external workbook cycle".into()));
    }
    let imported = import_xlsx_body(path, limits, opening, used_cells);
    opening.remove(&key);
    imported
}

fn import_xlsx_body(
    path: &Path,
    limits: ImportLimits,
    opening: &mut HashSet<PathBuf>,
    used_cells: &mut usize,
) -> Result<ImportedWorkbook, ImportError> {
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
    // External targets are loaded before this workbook's formulas compile, so
    // a reference sees the calculated cell. The stored link cache is used
    // when that file is absent, already on the chain, or only a base-name
    // collision of an absolute target whose cache is already populated.
    let external = external_cells_for_import(path, limits, opening, used_cells)?;
    let mut sheets = Vec::with_capacity(sheet_names.len());
    let mut observed_formulas = 0_usize;
    for name in &sheet_names {
        sheets.push(read_occupied_sheet(
            &mut source,
            name,
            limits,
            used_cells,
            &mut observed_formulas,
        )?);
    }
    let mut imported = import_ranges_with_names(
        sheets,
        defined_names,
        external.cells,
        external.sheets,
        source_sha256,
        limits,
    )?;
    imported.skipped_sheets = skipped_sheets;
    Ok(imported)
}

/// Occupied cells of one sheet, in absolute worksheet coordinates.
///
/// The bounding box is not allocated. A value and a formula on the same cell
/// are one occupied cell.
struct OccupiedSheet {
    name: String,
    values: Vec<Cell<Data>>,
    formulas: Vec<Cell<String>>,
}

/// Reads values and formulas in one pass. Shared formulas expand from the
/// anchor cell Excel stored, not from the top-left of the shared `ref`.
/// A derived cell whose anchor never appears is skipped. Chart and dialog
/// sheets have no cells. The occupied-cell and formula budgets are checked
/// as cells arrive, before any bounding box is built.
fn read_occupied_sheet<RS: Read + Seek>(
    source: &mut Xlsx<RS>,
    name: &str,
    limits: ImportLimits,
    used_cells: &mut usize,
    observed_formulas: &mut usize,
) -> Result<OccupiedSheet, ImportError> {
    let read_error = |error: calamine::XlsxError| ImportError::Read(error.to_string());
    let mut reader = match source.worksheet_cells_reader(name) {
        Ok(reader) => reader,
        Err(calamine::XlsxError::NotAWorksheet(_)) => {
            return Ok(OccupiedSheet {
                name: name.to_string(),
                values: Vec::new(),
                formulas: Vec::new(),
            });
        }
        Err(error) => return Err(read_error(error)),
    };
    let mut anchors: HashMap<usize, ((u32, u32), String)> = HashMap::new();
    let mut values = Vec::new();
    let mut occupied_positions = HashSet::new();
    let mut formulas = Vec::new();
    let mut pending = Vec::new();
    while let Some(record) = reader
        .next_cell_with_formula_metadata()
        .map_err(read_error)?
    {
        let has_value = !record.value.is_empty();
        let has_formula = record.formula.is_some();
        if has_value || has_formula {
            if !occupied_positions.insert(record.pos) {
                return Err(ImportError::Read(
                    "duplicate occupied worksheet cell".into(),
                ));
            }
            charge_cells(used_cells, 1, limits.max_cells)?;
        }
        if has_formula {
            *observed_formulas = observed_formulas.saturating_add(1);
            if *observed_formulas > limits.max_formulas {
                return Err(ImportError::TooManyFormulas {
                    observed: *observed_formulas,
                    maximum: limits.max_formulas,
                });
            }
        }
        if has_value {
            values.push(Cell::new(record.pos, Data::from(record.value)));
        }
        match record.formula {
            Some(XlsxFormulaMetadata::Normal { formula }) => {
                formulas.push(Cell::new(record.pos, formula));
            }
            Some(XlsxFormulaMetadata::Shared {
                shared_index,
                formula,
                ..
            }) => {
                anchors.insert(shared_index, (record.pos, formula.clone()));
                formulas.push(Cell::new(record.pos, formula));
            }
            Some(XlsxFormulaMetadata::SharedDerived { shared_index }) => {
                pending.push((record.pos, shared_index));
            }
            Some(_) | None => {}
        }
    }
    for (position, shared_index) in pending {
        let Some((anchor, template)) = anchors.get(&shared_index) else {
            continue;
        };
        let formula =
            calamine::expand_shared_formula(template, *anchor, position).map_err(read_error)?;
        if formula.is_empty() {
            continue;
        }
        formulas.push(Cell::new(position, formula));
    }
    Ok(OccupiedSheet {
        name: name.to_string(),
        values,
        formulas,
    })
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

/// One cell registered on the workbook's external cache before formulas compile.
struct CachedExternalCell {
    link_index: u32,
    book_file: Option<String>,
    sheet: String,
    row: u32,
    column: u32,
    value: Value,
}

struct ExternalLinkRecord {
    index: u32,
    book_file: Option<String>,
    /// Local file this link may open. Never a network URL or an absolute path
    /// outside the source workbook's directory.
    path: Option<PathBuf>,
    /// `path` is only the file name of an absolute or `file://` target. A
    /// populated cache must not be replaced by whatever happens to share that
    /// name in a flat directory.
    basename_only: bool,
    xml: String,
}

struct ExternalSheetNote {
    link_index: u32,
    book_file: Option<String>,
    sheet: String,
}

struct ExternalCacheLoad {
    cells: Vec<CachedExternalCell>,
    sheets: Vec<ExternalSheetNote>,
}

fn canonical_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn charge_cells(used: &mut usize, add: usize, maximum: usize) -> Result<(), ImportError> {
    let observed = used.checked_add(add).ok_or(ImportError::TooManyCells {
        observed: usize::MAX,
        maximum,
    })?;
    if observed > maximum {
        return Err(ImportError::TooManyCells { observed, maximum });
    }
    *used = observed;
    Ok(())
}

fn is_limit_error(error: &ImportError) -> bool {
    matches!(
        error,
        ImportError::TooManyCells { .. }
            | ImportError::TooManyFormulas { .. }
            | ImportError::TooManySheets { .. }
    )
}

/// Cells and known sheets for every external link of `path`. A relative
/// target that exists next to the source is calculated and wins over the link
/// part. A missing file, a network target, a workbook already being imported,
/// or a populated cache whose only local hit is the base name of an absolute
/// target keeps the part's cache. Cached records, opened targets and this
/// workbook's occupied cells share `used_cells`.
fn external_cells_for_import(
    path: &Path,
    limits: ImportLimits,
    opening: &mut HashSet<PathBuf>,
    used_cells: &mut usize,
) -> Result<ExternalCacheLoad, ImportError> {
    let Ok(links) = read_external_links(path) else {
        return Ok(ExternalCacheLoad {
            cells: Vec::new(),
            sheets: Vec::new(),
        });
    };
    let mut cells = Vec::new();
    let mut sheets = Vec::new();
    for link in links {
        let cache_sheets = external_sheet_names(&link.xml);
        let keep_cache = link.basename_only && external_cache_has_cell(&link.xml);
        let calculated = if keep_cache {
            None
        } else {
            link.path.as_deref().and_then(|target| {
                let key = canonical_key(target);
                if opening.contains(&key) {
                    return None;
                }
                Some(target)
            })
        };
        if let Some(target) = calculated {
            let snapshot = *used_cells;
            match import_xlsx_inner(target, limits, opening, used_cells) {
                Ok(imported) => {
                    *used_cells = snapshot;
                    let book_file = link.book_file.clone();
                    for sheet in &imported.sheets {
                        sheets.push(ExternalSheetNote {
                            link_index: link.index,
                            book_file: book_file.clone(),
                            sheet: sheet.name.clone(),
                        });
                    }
                    let values = cells_from_imported(&imported, link.index, book_file);
                    charge_cells(used_cells, values.len(), limits.max_cells)?;
                    cells.extend(values);
                    continue;
                }
                Err(error) if is_limit_error(&error) => return Err(error),
                Err(_) => {
                    *used_cells = snapshot;
                }
            }
        }
        for sheet in cache_sheets {
            sheets.push(ExternalSheetNote {
                link_index: link.index,
                book_file: link.book_file.clone(),
                sheet,
            });
        }
        append_external_cache(
            &link.xml,
            link.index,
            link.book_file,
            used_cells,
            limits.max_cells,
            &mut cells,
        )?;
    }
    Ok(ExternalCacheLoad { cells, sheets })
}

fn cells_from_imported(
    imported: &ImportedWorkbook,
    link_index: u32,
    book_file: Option<String>,
) -> Vec<CachedExternalCell> {
    imported
        .source_cells()
        .iter()
        .filter_map(|source| {
            let sheet = imported.sheets.get(source.cell.sheet as usize)?;
            Some(CachedExternalCell {
                link_index,
                book_file: book_file.clone(),
                sheet: sheet.name.clone(),
                row: source.cell.row,
                column: source.cell.column,
                value: imported.workbook.value(source.cell),
            })
        })
        .collect()
}

fn read_external_links(path: &Path) -> Result<Vec<ExternalLinkRecord>, ImportError> {
    let file = File::open(path).map_err(|error| ImportError::Open(error.to_string()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| ImportError::Open(error.to_string()))?;
    let workbook = read_part(&mut archive, "xl/workbook.xml")?;
    let relationships = parse_relationships(
        &read_optional_part(&mut archive, "xl/_rels/workbook.xml.rels").unwrap_or_default(),
    );
    let source_dir = path.parent().unwrap_or(Path::new(""));
    let mut links = Vec::new();
    for (offset, rel_id) in external_reference_ids(&workbook).into_iter().enumerate() {
        let index = (offset + 1) as u32;
        let part_target = relationships
            .iter()
            .find(|relationship| relationship.id == rel_id)
            .map(|relationship| relationship.target.as_str());
        let Some(part_target) = part_target else {
            links.push(ExternalLinkRecord {
                index,
                book_file: None,
                path: None,
                basename_only: false,
                xml: String::new(),
            });
            continue;
        };
        let part = resolve_package_part("xl/workbook.xml", part_target);
        let xml = read_optional_part(&mut archive, &part).unwrap_or_default();
        let link_relationships = parse_relationships(
            &read_optional_part(&mut archive, &package_rels_path(&part)).unwrap_or_default(),
        );
        let book_rel = external_book_relationship_id(&xml);
        let raw_target = book_rel
            .as_ref()
            .and_then(|id| {
                link_relationships
                    .iter()
                    .find(|relationship| relationship.id == *id)
            })
            .or_else(|| {
                link_relationships
                    .iter()
                    .find(|relationship| relationship.kind.ends_with("/externalLinkPath"))
            })
            .map(|relationship| relationship.target.clone());
        let book_file = raw_target.as_deref().and_then(external_book_label);
        let resolved = raw_target
            .as_deref()
            .and_then(|target| resolve_external_target(source_dir, target));
        let (path, basename_only) = match resolved {
            Some((path, basename_only)) => (Some(path), basename_only),
            None => (None, false),
        };
        links.push(ExternalLinkRecord {
            index,
            book_file,
            path,
            basename_only,
            xml,
        });
    }
    Ok(links)
}

fn read_optional_part(archive: &mut zip::ZipArchive<File>, name: &str) -> Option<String> {
    read_part(archive, name).ok()
}

struct PackageRelationship {
    id: String,
    kind: String,
    target: String,
}

fn parse_relationships(xml: &str) -> Vec<PackageRelationship> {
    let mut relationships = Vec::new();
    scan_elements(xml, "Relationship", |tag, _| {
        let Some(id) = attribute(tag, "Id") else {
            return;
        };
        let Some(target) = attribute(tag, "Target") else {
            return;
        };
        relationships.push(PackageRelationship {
            id,
            kind: attribute(tag, "Type").unwrap_or_default(),
            target,
        });
    });
    relationships
}

fn external_reference_ids(workbook_xml: &str) -> Vec<String> {
    let mut ids = Vec::new();
    scan_elements(workbook_xml, "externalReference", |tag, _| {
        if let Some(id) = attribute(tag, "r:id") {
            ids.push(id);
        }
    });
    ids
}

fn external_book_relationship_id(external_link_xml: &str) -> Option<String> {
    let mut found = None;
    scan_elements(external_link_xml, "externalBook", |tag, _| {
        if found.is_none() {
            found = attribute(tag, "r:id");
        }
    });
    found
}

fn resolve_package_part(source_part: &str, target: &str) -> String {
    let target = target.replace('\\', "/");
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.trim_start_matches('/').to_string();
    }
    let mut parts: Vec<&str> = source_part
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn package_rels_path(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/_rels/{name}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// Path of a link target that may be opened: a relative path under the source
/// directory, or, for an absolute path or `file://` URL, the file name next
/// to the source. Network targets, `..`, and a symlink that leaves that
/// directory are not opened. The boolean is true when the path is only that
/// base name.
fn resolve_external_path(source_dir: &Path, raw_target: &str) -> Option<PathBuf> {
    resolve_external_target(source_dir, raw_target).map(|(path, _)| path)
}

fn resolve_external_target(source_dir: &Path, raw_target: &str) -> Option<(PathBuf, bool)> {
    let (relative, basename_only) = local_relative_target(raw_target)?;
    let candidate = source_dir.join(relative);
    if !candidate.is_file() {
        return None;
    }
    let canonical_source = std::fs::canonicalize(source_dir).ok()?;
    let canonical_candidate = std::fs::canonicalize(&candidate).ok()?;
    canonical_candidate
        .starts_with(&canonical_source)
        .then_some((candidate, basename_only))
}

fn local_relative_target(raw_target: &str) -> Option<(PathBuf, bool)> {
    let decoded = percent_decode(raw_target.trim());
    if decoded.is_empty() {
        return None;
    }
    let lower = decoded.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("mailto:")
        || (lower.contains("://") && !lower.starts_with("file:"))
    {
        return None;
    }
    let path_text = if lower.starts_with("file:") {
        file_url_path(&decoded)?
    } else {
        decoded.replace('\\', "/")
    };
    let path = Path::new(path_text.trim());
    if lower.starts_with("file:") || path.is_absolute() {
        return path
            .file_name()
            .filter(|name| !name.is_empty())
            .map(|name| (PathBuf::from(name), true));
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let relative: PathBuf = path
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .collect();
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some((relative, false))
    }
}

fn file_url_path(url: &str) -> Option<String> {
    let rest = strip_ascii_prefix(url, "file:")?;
    let rest = rest.strip_prefix("//")?;
    let rest = strip_ascii_prefix(rest, "localhost").unwrap_or(rest);
    let rest = rest.replace('\\', "/");
    if rest.is_empty() || rest == "/" {
        None
    } else {
        Some(rest)
    }
}

fn strip_ascii_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        text.get(prefix.len()..)
    } else {
        None
    }
}

fn external_book_label(raw_target: &str) -> Option<String> {
    let decoded = percent_decode(raw_target.trim());
    let trimmed = decoded.split(['?', '#']).next().unwrap_or(decoded.as_str());
    let segment = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    if segment.is_empty() || segment == "." || segment == ".." || segment.contains(':') {
        None
    } else {
        Some(segment.to_string())
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    decoded.push(value);
                    index += 3;
                    continue;
                }
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn external_sheet_names(xml: &str) -> Vec<String> {
    let mut sheet_names = Vec::new();
    scan_elements(xml, "sheetNames", |_tag, body| {
        scan_elements(body, "sheetName", |tag, _| {
            if let Some(name) = attribute(tag, "val") {
                sheet_names.push(name);
            }
        });
    });
    sheet_names
}

fn external_cache_has_cell(xml: &str) -> bool {
    xml.contains("<cell ")
}

fn append_external_cache(
    xml: &str,
    link_index: u32,
    book_file: Option<String>,
    used_cells: &mut usize,
    maximum: usize,
    cells: &mut Vec<CachedExternalCell>,
) -> Result<(), ImportError> {
    let sheet_names = external_sheet_names(xml);
    let mut error = None;
    scan_elements(xml, "sheetDataSet", |_tag, body| {
        if error.is_some() {
            return;
        }
        scan_elements(body, "sheetData", |tag, data| {
            if error.is_some() {
                return;
            }
            let Some(sheet_id) =
                attribute(tag, "sheetId").and_then(|value| value.parse::<usize>().ok())
            else {
                return;
            };
            let Some(sheet) = sheet_names.get(sheet_id).cloned() else {
                return;
            };
            scan_elements(data, "row", |_row_tag, row_body| {
                if error.is_some() {
                    return;
                }
                scan_elements(row_body, "cell", |cell_tag, cell_body| {
                    if error.is_some() {
                        return;
                    }
                    let Some(reference) = attribute(cell_tag, "r") else {
                        return;
                    };
                    let Some((row, column)) = parse_cell_reference(&reference) else {
                        return;
                    };
                    let Some(value) = cached_cell_value(cell_tag, cell_body) else {
                        return;
                    };
                    if let Err(failed) = charge_cells(used_cells, 1, maximum) {
                        error = Some(failed);
                        return;
                    }
                    cells.push(CachedExternalCell {
                        link_index,
                        book_file: book_file.clone(),
                        sheet: sheet.clone(),
                        row,
                        column,
                        value,
                    });
                });
            });
        });
    });
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn cached_cell_value(tag: &str, body: &str) -> Option<Value> {
    let kind = attribute(tag, "t").unwrap_or_default();
    if kind == "s" {
        return None;
    }
    let raw = xml_text_element(body, "v").or_else(|| xml_text_element(body, "t"))?;
    match kind.as_str() {
        "b" => {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            Some(Value::Boolean(
                raw == "1" || raw.eq_ignore_ascii_case("true"),
            ))
        }
        "e" => {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            Some(Value::Error(excel_error(raw)))
        }
        "str" | "inlineStr" => Some(Value::Text(raw)),
        _ => {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            raw.parse::<f64>().ok().map(Value::Number)
        }
    }
}

fn excel_error(raw: &str) -> CalcError {
    match raw {
        "#DIV/0!" => CalcError::DivisionByZero,
        "#N/A" => CalcError::NotAvailable,
        "#NAME?" => CalcError::InvalidName,
        "#NULL!" => CalcError::NullIntersection,
        "#NUM!" => CalcError::InvalidNumber,
        "#VALUE!" => CalcError::InvalidValue,
        _ => CalcError::InvalidReference,
    }
}

fn xml_text_element(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let after = &xml[start + open.len()..];
    if !after.starts_with([' ', '>', '/', '\n', '\r', '\t']) {
        return None;
    }
    let tag_end = after.find('>')?;
    if after[..tag_end].trim_end().ends_with('/') {
        return Some(String::new());
    }
    let content = &after[tag_end + 1..];
    let close = format!("</{tag}>");
    let end = content.find(&close)?;
    Some(unescape_xml(&content[..end]))
}

fn parse_cell_reference(reference: &str) -> Option<(u32, u32)> {
    let normalized = reference.replace('$', "").to_ascii_uppercase();
    let split = normalized.find(|character: char| character.is_ascii_digit())?;
    let (column_text, row_text) = normalized.split_at(split);
    if column_text.is_empty()
        || row_text.is_empty()
        || !column_text.bytes().all(|byte| byte.is_ascii_uppercase())
        || !row_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut column = 0_u32;
    for byte in column_text.bytes() {
        column = column
            .checked_mul(26)?
            .checked_add(u32::from(byte - b'A') + 1)?;
    }
    let row = row_text.parse::<u32>().ok().filter(|value| *value > 0)?;
    if column > 16_384 || row > 1_048_576 {
        return None;
    }
    Some((row - 1, column - 1))
}

fn scan_elements(xml: &str, tag: &str, mut visit: impl FnMut(&str, &str)) {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        if !after.starts_with([' ', '>', '/', '\n', '\r', '\t']) {
            rest = after;
            continue;
        }
        let Some(tag_end) = after.find('>') else {
            break;
        };
        let start_tag = &after[..tag_end];
        if start_tag.trim_end().ends_with('/') {
            visit(start_tag, "");
            rest = &after[tag_end + 1..];
            continue;
        }
        let content = &after[tag_end + 1..];
        let Some(end) = content.find(&close) else {
            break;
        };
        visit(start_tag, &content[..end]);
        rest = &content[end + close.len()..];
    }
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
    import_ranges_with_names(
        ranges.into_iter().map(occupied_from_ranges).collect(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        source_sha256,
        limits,
    )
}

fn import_ranges_with_names(
    ranges: Vec<OccupiedSheet>,
    defined_names: Vec<DefinedName>,
    external_cells: Vec<CachedExternalCell>,
    external_sheets: Vec<ExternalSheetNote>,
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
    for sheet in &ranges {
        observed_cells = observed_cells.saturating_add(occupied_count(sheet));
        observed_formulas = observed_formulas.saturating_add(
            sheet
                .formulas
                .iter()
                .filter(|cell| !cell.get_value().is_empty())
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
        .map(|(index, sheet)| {
            let (value_rows, value_columns) = range_extent(span_end(
                sheet.values.iter().map(|cell| cell.get_position()),
            ));
            let (formula_rows, formula_columns) = range_extent(span_end(
                sheet.formulas.iter().map(|cell| cell.get_position()),
            ));
            SheetInfo {
                index: index as u32,
                name: sheet.name.clone(),
                rows: value_rows.max(formula_rows),
                columns: value_columns.max(formula_columns),
            }
        })
        .collect();
    let mut workbook = Workbook::default();
    workbook.require_external_inputs();
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
    for sheet in external_sheets {
        workbook.note_external_sheet(sheet.link_index, sheet.book_file.as_deref(), &sheet.sheet);
    }
    for external in external_cells {
        workbook.cache_external_cell(
            external.link_index,
            external.book_file.as_deref(),
            &external.sheet,
            external.row,
            external.column,
            external.value,
        );
    }
    let mut source_cells = BTreeMap::new();

    for (sheet_index, sheet) in ranges.into_iter().enumerate() {
        for cell in sheet.values {
            if cell.get_value().is_empty() {
                continue;
            }
            let (row, column) = cell.get_position();
            let id = CellId::new(sheet_index as u32, row, column);
            set_source_value(&mut workbook, id, cell.get_value());
            source_cells.insert(
                id,
                ImportedCell {
                    cell: id,
                    stored: source_value(cell.get_value()),
                    formula: None,
                },
            );
        }
        for cell in sheet.formulas {
            if cell.get_value().is_empty() {
                continue;
            }
            let (row, column) = cell.get_position();
            let id = CellId::new(sheet_index as u32, row, column);
            source_cells
                .entry(id)
                .or_insert_with(|| ImportedCell {
                    cell: id,
                    stored: Value::Blank,
                    formula: None,
                })
                .formula = Some(cell.get_value().clone());
        }
    }

    let source_cells: Vec<_> = source_cells.into_values().collect();
    if let Some(at) = tick_from_cached_volatile(&source_cells) {
        // Before formulas are installed, so TODAY() and NOW() replay this
        // serial instead of staying #N/A. Does not read the system clock.
        workbook.set_tick(1, at);
    }
    let mut unsupported = Vec::new();
    let mut compiled_cells = Vec::with_capacity(observed_formulas);
    for (index, source) in source_cells.iter().enumerate() {
        let Some(formula) = &source.formula else {
            continue;
        };
        let cell = source.cell;
        match workbook.set_formula(cell, formula) {
            Ok(_) => compiled_cells.push(index),
            Err(error) => unsupported.push(UnsupportedFormula {
                cell,
                reason: bounded_formula_error(&error),
                error,
            }),
        }
    }
    let formula_cells_loaded = compiled_cells.len();
    workbook.end_bulk();
    Ok(ImportedWorkbook {
        workbook,
        sheets,
        source_sha256,
        date_system: DATE_SYSTEM,
        unsupported,
        skipped_sheets: Vec::new(),
        source_cells,
        compiled_cells,
        formula_cells_observed: observed_formulas,
        formula_cells_loaded,
    })
}

/// First cached `NOW()` serial, else the first cached `TODAY()` serial, as
/// UTC Unix milliseconds. Anything that is not exactly that call is ignored,
/// so `TODAY()+1` cannot move the tick. No cached volatile leaves the
/// workbook without a tick.
fn tick_from_cached_volatile(cells: &[ImportedCell]) -> Option<i64> {
    let mut today = None;
    let mut now = None;
    for cell in cells {
        let Some(formula) = cell.formula.as_deref() else {
            continue;
        };
        let Value::Number(serial) = cell.stored else {
            continue;
        };
        match cached_clock_formula(formula) {
            Some(true) if now.is_none() => now = Some(serial),
            Some(false) if today.is_none() => today = Some(serial),
            _ => {}
        }
    }
    for serial in [now, today].into_iter().flatten() {
        if let Ok(millis) = omasheets_calc::serial_date::unix_millis_from_serial(serial) {
            return Some(millis);
        }
    }
    None
}

/// `Some(true)` is `NOW()`, `Some(false)` is `TODAY()`. Optional `=` and
/// unary `+`, plus whitespace, are ignored. Any other spelling is not a tick.
fn cached_clock_formula(formula: &str) -> Option<bool> {
    let mut text = formula.trim();
    if let Some(rest) = text.strip_prefix('=') {
        text = rest.trim_start();
    }
    while text.starts_with('+') {
        text = text[1..].trim_start();
    }
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if compact.eq_ignore_ascii_case("NOW()") {
        Some(true)
    } else if compact.eq_ignore_ascii_case("TODAY()") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
fn occupied_from_ranges(
    (name, values, formulas): (String, Range<Data>, Range<String>),
) -> OccupiedSheet {
    OccupiedSheet {
        name,
        values: absolute_used(values),
        formulas: absolute_used(formulas),
    }
}

#[cfg(test)]
fn absolute_used<T: calamine::CellType + Clone>(range: Range<T>) -> Vec<Cell<T>> {
    let (origin_row, origin_column) = range.start().unwrap_or((0, 0));
    range
        .used_cells()
        .map(|(row, column, value)| {
            Cell::new(
                (origin_row + row as u32, origin_column + column as u32),
                value.clone(),
            )
        })
        .collect()
}

fn occupied_count(sheet: &OccupiedSheet) -> usize {
    let mut positions = HashSet::new();
    for cell in &sheet.values {
        if !cell.get_value().is_empty() {
            positions.insert(cell.get_position());
        }
    }
    for cell in &sheet.formulas {
        if !cell.get_value().is_empty() {
            positions.insert(cell.get_position());
        }
    }
    positions.len()
}

fn span_end(positions: impl Iterator<Item = (u32, u32)>) -> Option<(u32, u32)> {
    let mut end: Option<(u32, u32)> = None;
    for (row, column) in positions {
        end = Some(match end {
            None => (row, column),
            Some((end_row, end_column)) => (end_row.max(row), end_column.max(column)),
        });
    }
    end
}

fn range_extent(end: Option<(u32, u32)>) -> (usize, usize) {
    end.map(|(row, column)| (row as usize + 1, column as usize + 1))
        .unwrap_or((0, 0))
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

fn value_kind(value: &Value) -> &str {
    match value {
        Value::Blank => "blank",
        Value::Number(_) => "number",
        Value::Boolean(_) => "boolean",
        Value::Text(_) => "text",
        Value::Error(error) => error.label(),
    }
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
        assert_eq!(
            (imported.sheets[0].rows, imported.sheets[0].columns),
            (3, 3)
        );
        assert_eq!(imported.source_cells().len(), 5);
        let formula = imported
            .source_cells()
            .iter()
            .find(|source| source.cell == CellId::new(0, 2, 0))
            .unwrap();
        assert_eq!(formula.stored, Value::Number(5.0));
        assert_eq!(formula.formula.as_deref(), Some("A1+A2"));
    }

    #[test]
    fn reference_diagnostics_expose_only_fixed_failure_classes() {
        let bytes = package(
            &[],
            "",
            r#"<c r="B1"><f>SUM(1:2)</f><v>0</v></c><c r="C1"><f>SUM(Data!A:A)</f><v>0</v></c><c r="D1"><f>SUM(A1:SUM(A1:A2))</f><v>0</v></c><c r="E1"><f>1+1</f><v>9</v></c>"#,
            "",
        );
        let path = temporary_xlsx(&bytes);
        let report = import_xlsx(&path, ImportLimits::default())
            .unwrap()
            .report();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            report.reference_failure_kinds,
            BTreeMap::from([
                ("column_without_row".into(), 1),
                ("dynamic_range_endpoint".into(), 1),
                ("numeric_range_endpoint".into(), 1),
            ])
        );
        assert_eq!(
            report.mismatch_value_kinds,
            BTreeMap::from([("number -> number".into(), 1)])
        );
    }

    #[test]
    fn syntax_diagnostics_expose_only_fixed_token_classes() {
        let path = temporary_xlsx(&package(
            &[],
            "",
            r#"<c r="B1"><f>SUM(1;2)</f><v>3</v></c><c r="C1"><f>SUM({1,})</f><v>1</v></c>"#,
            "",
        ));
        let imported = import_xlsx(&path, ImportLimits::default()).unwrap();
        std::fs::remove_file(path).unwrap();
        let report = imported.report();
        assert_eq!(
            report.syntax_failure_tokens,
            BTreeMap::from([("semicolon".into(), 1), ("array_brace".into(), 1),])
        );
        assert_eq!(report.unsupported_reasons["syntax"], 2);
        assert_eq!(report.formula_cells_compared, 1);
    }

    #[test]
    fn matrix_and_database_formulas_match_independent_caches() {
        let path = temporary_xlsx(&package(
            &[],
            "",
            r#"<c r="B1"><f>MMULT(TRANSPOSE(A1:A2),A1:A2)</f><v>13</v></c><c r="C1"><f>INDEX(TRANSPOSE({1,2,3;4,5,6}),3,2)</f><v>6</v></c><c r="D1"><f>DAVERAGE({"Kind","Value";"A",10;"B",50;"A",20},"Value",{"Kind";"=A"})</f><v>15</v></c><c r="E1"><f>DMAX({"Kind","Value";"A",10;"B",50;"A",20},2,{"Kind";"=A"})</f><v>20</v></c><c r="F1"><f>DMIN({"Kind","Value";"A",10;"B",50;"A",20},2,{"Kind";"=A"})</f><v>10</v></c><c r="G1"><f>DSTDEV({"Kind","Value";"A",10;"B",50;"A",20},2,{"Kind";"=A"})</f><v>7.0710678118654755</v></c>"#,
            "",
        ));
        let report = import_xlsx(&path, ImportLimits::default())
            .unwrap()
            .report();
        std::fs::remove_file(path).unwrap();
        assert_eq!(report.formula_cells_loaded, 7);
        assert_eq!(report.stored_values_matched, 7);
        assert_eq!(report.stored_values_mismatched, 0);
    }

    #[test]
    fn reference_valued_index_matches_xlsx_caches() {
        let bytes = package(
            &[],
            "",
            r#"<c r="B1"><f>SUM(A1:INDEX(A1:A2,2))</f><v>5</v></c><c r="C1"><f>SUM(INDEX(A1:A2,0))</f><v>5</v></c><c r="D1"><f>MATCH(3,INDEX(A1:A2,0),0)</f><v>2</v></c>"#,
            "",
        );
        let path = temporary_xlsx(&bytes);
        let report = import_xlsx(&path, ImportLimits::default())
            .unwrap()
            .report();
        std::fs::remove_file(path).unwrap();
        assert_eq!(report.formula_cells_loaded, 4);
        assert_eq!(report.stored_values_matched, 4);
        assert_eq!(report.stored_values_mismatched, 0);
    }

    #[test]
    fn array_financial_and_deleted_reference_formulas_match_xlsx_caches() {
        let path = temporary_xlsx(&package(
            &[],
            r#"<definedName name="weights">{2;3}</definedName><definedName name="rate" localSheetId="0">0.1</definedName>"#,
            r#"<c r="B1"><f>SUMPRODUCT({10;20},weights)</f><v>80</v></c><c r="C1"><f>PV(Data!rate,1,-110)</f><v>100</v></c><c r="D1"><f>_xlfn.XLOOKUP(2,{1,2},{10,20})</f><v>20</v></c><c r="E1" t="e"><f>SUM(#REF!:#REF!)</f><v>#REF!</v></c><c r="F1"><f>IFERROR(SUM(A1:#REF!),17)</f><v>17</v></c>"#,
            "",
        ));
        let imported = import_xlsx(&path, ImportLimits::default()).unwrap();
        std::fs::remove_file(path).unwrap();
        let report = imported.report();
        assert_eq!(report.formula_cells_observed, 6);
        assert_eq!(report.formula_cells_loaded, 6);
        assert_eq!(report.stored_values_matched, 6);
        assert_eq!(report.stored_values_mismatched, 0);
        assert!(report.unsupported_reasons.is_empty());
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
        assert_eq!(report.formula_cells_loaded, 2);
        assert_eq!(report.unsupported_formulas, 4);
        assert_eq!(
            report.unsupported_functions,
            BTreeMap::from([("TODAY".to_string(), 2)])
        );
        assert_eq!(
            report.unsupported_reasons,
            BTreeMap::from([
                ("unsupported_function".to_string(), 2),
                ("syntax".to_string(), 1),
                ("unknown_sheet".to_string(), 1),
            ])
        );
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.starts_with("{\"schema\":2,\"engine\":\"omasheets-owned-m0\""));
        assert_eq!(serde_json::from_str::<ScoreReport>(&json).unwrap(), report);
    }

    #[test]
    fn occupied_cell_budget_ignores_the_bounding_box() {
        let limits = ImportLimits {
            max_cells: 100,
            ..ImportLimits::default()
        };
        let imported = import_ranges(
            ranges(
                vec![
                    Cell::new((0, 0), Data::Int(1)),
                    Cell::new((10, 10), Data::Int(2)),
                ],
                vec![Cell::new((0, 0), "1+1".into())],
            ),
            "c".repeat(64),
            limits,
        )
        .expect("two occupied cells inside an 11 by 11 box");
        assert_eq!(
            imported.workbook.value(CellId::new(0, 10, 10)),
            Value::Number(2.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(2.0)
        );
        let crowded: Vec<_> = (0..101)
            .map(|row| Cell::new((row, 0), Data::Int(1)))
            .collect();
        let error = import_ranges(ranges(crowded, vec![]), "c".repeat(64), limits)
            .err()
            .expect("101 cells exceed the budget");
        assert_eq!(
            error,
            ImportError::TooManyCells {
                observed: 101,
                maximum: 100,
            }
        );
    }

    #[test]
    fn sparse_sheet_does_not_allocate_its_used_range_box() {
        let path = std::env::temp_dir().join(format!(
            "omasheets-sparse-{}-{}.xlsx",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        write_dimension_workbook(&path);
        let imported = import_xlsx(
            &path,
            ImportLimits {
                max_cells: 100,
                ..ImportLimits::default()
            },
        )
        .expect("two cells under a full-grid dimension");
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(1.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 1_048_575, 16_383)),
            Value::Number(2.0)
        );
    }

    #[test]
    fn streaming_import_enforces_cell_and_formula_budgets() {
        let cases = [
            (
                "cached-formula",
                "<c r=\"A1\"><f>1+1</f><v>2</v></c>",
                1,
                1,
                None,
                1,
            ),
            ("formula-only", "<c r=\"A1\"><f>1+1</f></c>", 1, 1, None, 1),
            ("styled-empty", "<c r=\"A1\" s=\"0\"/>", 0, 0, None, 0),
            (
                "cell-overflow",
                "<c r=\"A1\"><v>1</v></c><c r=\"B1\"><v>2</v></c>",
                1,
                1,
                Some(ImportError::TooManyCells {
                    observed: 2,
                    maximum: 1,
                }),
                0,
            ),
            (
                "formula-overflow",
                "<c r=\"A1\"><f>1</f></c><c r=\"B1\"><f>2</f></c>",
                2,
                1,
                Some(ImportError::TooManyFormulas {
                    observed: 2,
                    maximum: 1,
                }),
                0,
            ),
            (
                "pending-cell-overflow",
                "<c r=\"A1\"><f t=\"shared\" si=\"0\"/></c><c r=\"B1\"><f t=\"shared\" si=\"0\"/></c>",
                1,
                2,
                Some(ImportError::TooManyCells {
                    observed: 2,
                    maximum: 1,
                }),
                0,
            ),
            (
                "pending-formula-overflow",
                "<c r=\"A1\"><f t=\"shared\" si=\"0\"/></c><c r=\"B1\"><f t=\"shared\" si=\"0\"/></c>",
                2,
                1,
                Some(ImportError::TooManyFormulas {
                    observed: 2,
                    maximum: 1,
                }),
                0,
            ),
            (
                "duplicate-cell",
                "<c r=\"A1\"><v>1</v></c><c r=\"A1\"><v>2</v></c>",
                2,
                0,
                Some(ImportError::Read(
                    "duplicate occupied worksheet cell".into(),
                )),
                0,
            ),
        ];
        for (name, cells, max_cells, max_formulas, error, occupied) in cases {
            let path = std::env::temp_dir().join(format!(
                "omasheets-budget-{}-{name}.xlsx",
                std::process::id()
            ));
            let sheet = format!(
                "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\">{cells}</row></sheetData></worksheet>"
            );
            write_plain_workbook_xml(&path, &sheet);
            let result = import_xlsx(
                &path,
                ImportLimits {
                    max_cells,
                    max_formulas,
                    ..ImportLimits::default()
                },
            );
            std::fs::remove_file(&path).unwrap();
            match error {
                Some(expected) => assert_eq!(result.err(), Some(expected), "{name}"),
                None => {
                    let imported = result.unwrap_or_else(|error| panic!("{name}: {error}"));
                    assert_eq!(imported.source_cells.len(), occupied, "{name}");
                    if occupied == 1 {
                        assert_eq!(
                            imported.workbook.value(CellId::new(0, 0, 0)),
                            Value::Number(2.0),
                            "{name}"
                        );
                    }
                }
            }
        }
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
            vec![occupied_from_ranges((
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
            ))],
            vec![
                workbook_name("Rates", "Data!$A$1:$A$2"),
                workbook_name("Broken", "[2]External!A1"),
            ],
            Vec::new(),
            Vec::new(),
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
        // No linked input: refuse compilation and retain the source cache
        // (blank here), rather than inventing an error for dependent formulas.
        assert_eq!(imported.workbook.value(CellId::new(0, 4, 2)), Value::Blank);
        assert_eq!(imported.workbook.value(CellId::new(0, 5, 2)), Value::Blank);
        let parity = imported.parity();
        assert_eq!(parity.formula_cells_loaded, 3);
        assert_eq!(parity.stored_values_matched, 3);
        assert_eq!(
            imported.report().unsupported_reasons,
            BTreeMap::from([
                ("unknown_name".to_string(), 1),
                ("external_reference".to_string(), 1),
                ("unsupported_name".to_string(), 1)
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

    fn write_owned(path: &Path, parts: &[(&str, String)]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, body) in parts {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    fn content_types(extra: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>{extra}</Types>"#
        )
    }

    fn write_dimension_workbook(path: &Path) {
        let sheet = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:XFD1048576"/><sheetData><row r="1"><c r="A1"><v>1</v></c></row><row r="1048576"><c r="XFD1048576"><v>2</v></c></row></sheetData></worksheet>"#;
        write_plain_workbook_xml(path, sheet);
    }

    fn write_plain_workbook_xml(path: &Path, sheet_xml: &str) {
        let workbook = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
        write_owned(
            path,
            &[
                ("[Content_Types].xml", content_types("")),
                (
                    "_rels/.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/workbook.xml", workbook.to_string()),
                (
                    "xl/_rels/workbook.xml.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/worksheets/sheet1.xml", sheet_xml.to_string()),
            ],
        );
    }

    fn worksheet_xml(cells: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1">{cells}</row></sheetData></worksheet>"#
        )
    }

    fn xml_attr(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
    }

    fn write_plain_workbook(path: &Path, sheet: &str, cells: &str) {
        let workbook = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="{sheet}" sheetId="1" r:id="rId1"/></sheets></workbook>"#
        );
        write_owned(
            path,
            &[
                ("[Content_Types].xml", content_types("")),
                (
                    "_rels/.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/workbook.xml", workbook),
                (
                    "xl/_rels/workbook.xml.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/worksheets/sheet1.xml", worksheet_xml(cells)),
            ],
        );
    }

    /// `link_target` is the relationship target. `cache_value` is the
    /// external-link value for `Inputs!A1`, used only when the file is absent.
    fn write_linked_workbook(
        path: &Path,
        sheet: &str,
        cells: &str,
        defined_names: &str,
        link_target: &str,
        cache_value: &str,
    ) {
        write_linked_workbook_cache(
            path,
            sheet,
            cells,
            defined_names,
            link_target,
            &format!(r#"<row r="1"><cell r="A1"><v>{cache_value}</v></cell></row>"#),
        );
    }

    #[test]
    fn unavailable_link_inputs_preserve_source_caches_and_dependents() {
        let root = std::env::temp_dir().join(format!(
            "omasheets-link-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _cleanup = TempCleanup(root.clone());
        let source = root.join("Source.xlsx");
        write_linked_workbook(
            &source,
            "Report",
            r#"<c r="A1"><f>[1]Inputs!B1+2</f><v>12</v></c><c r="B1"><f>SUM([2]Missing!A1:A3)</f><v>15</v></c><c r="C1"><f>A1+B1</f><v>27</v></c><c r="D1"><f>SUM([1]Inputs!A1:A3)</f><v>10</v></c>"#,
            "",
            "Missing.xlsx",
            "10",
        );
        let imported = import_xlsx(&source, ImportLimits::default()).unwrap();
        assert_eq!(imported.unsupported.len(), 2);
        for (column, expected) in [12.0, 15.0, 27.0, 10.0].into_iter().enumerate() {
            assert_eq!(
                imported.workbook.value(CellId::new(0, 0, column as u32)),
                Value::Number(expected)
            );
        }
    }

    #[test]
    fn external_reference_uses_calculated_target_cell() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("omasheets-external-{}-{nonce}", std::process::id()));
        let _cleanup = TempCleanup(root.clone());
        let linked = root.join("linked");
        let book = linked.join("Book.xlsx");
        let source = linked.join("Source.xlsx");
        write_plain_workbook(&book, "Inputs", r#"<c r="A1"><f>3+7</f><v>99</v></c>"#);
        write_linked_workbook(
            &source,
            "Report",
            r#"<c r="A1"><f>[Book.xlsx]Inputs!A1+2</f><v>0</v></c><c r="B1"><f>Linked+2</f><v>0</v></c><c r="C1"><f>[1]Inputs!A1+2</f><v>0</v></c>"#,
            r#"<definedName name="Linked">[Book.xlsx]Inputs!A1</definedName>"#,
            "Book.xlsx",
            "1",
        );
        assert_eq!(
            resolve_external_path(linked.as_path(), "Book.xlsx").as_deref(),
            Some(book.as_path())
        );
        assert!(resolve_external_path(linked.as_path(), "https://example.com/Book.xlsx").is_none());
        assert!(resolve_external_path(linked.as_path(), "../Book.xlsx").is_none());

        let imported = import_xlsx(&source, ImportLimits::default()).unwrap();
        assert!(imported.unsupported.is_empty());
        // The link cache says 1. The file's formula calculates to 10, so the
        // source formula is 12, not 3.
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(12.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 1)),
            Value::Number(12.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 2)),
            Value::Number(12.0)
        );

        let absent_dir = root.join("absent");
        let absent = absent_dir.join("Absent.xlsx");
        write_linked_workbook(
            &absent,
            "Report",
            r#"<c r="A1"><f>[Missing.xlsx]Inputs!A1+2</f><v>0</v></c><c r="B1"><f>Linked+2</f><v>0</v></c>"#,
            r#"<definedName name="Linked">[Missing.xlsx]Inputs!A1</definedName>"#,
            "Missing.xlsx",
            "10",
        );
        let absent_imported = import_xlsx(&absent, ImportLimits::default()).unwrap();
        assert!(absent_imported.unsupported.is_empty());
        assert_eq!(
            absent_imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(12.0)
        );
        assert_eq!(
            absent_imported.workbook.value(CellId::new(0, 0, 1)),
            Value::Number(12.0)
        );

        let outside = root.join("outside");
        let secret = outside.join("Secret.xlsx");
        write_plain_workbook(&secret, "Inputs", r#"<c r="A1"><f>3+7</f><v>99</v></c>"#);
        let absolute = secret.canonicalize().unwrap();
        let absolute_dir = root.join("absolute");
        let absolute_source = absolute_dir.join("Source.xlsx");
        write_linked_workbook(
            &absolute_source,
            "Report",
            r#"<c r="A1"><f>[Secret.xlsx]Inputs!A1+2</f><v>0</v></c>"#,
            "",
            absolute.to_str().unwrap(),
            "4",
        );
        assert!(
            resolve_external_path(absolute_dir.as_path(), absolute.to_str().unwrap()).is_none()
        );
        assert!(
            resolve_external_path(
                absolute_dir.as_path(),
                &format!("file://{}", absolute.display())
            )
            .is_none()
        );
        let absolute_imported = import_xlsx(&absolute_source, ImportLimits::default()).unwrap();
        assert_eq!(
            absolute_imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(6.0)
        );

        let cycle_dir = root.join("cycle");
        let cycle = cycle_dir.join("Self.xlsx");
        write_linked_workbook(
            &cycle,
            "Report",
            r#"<c r="A1"><f>[Self.xlsx]Inputs!A1+2</f><v>0</v></c>"#,
            "",
            "Self.xlsx",
            "10",
        );
        let cycle_imported = import_xlsx(&cycle, ImportLimits::default()).unwrap();
        assert_eq!(
            cycle_imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(12.0)
        );
    }

    fn write_linked_workbook_cache(
        path: &Path,
        sheet: &str,
        cells: &str,
        defined_names: &str,
        link_target: &str,
        cache_rows: &str,
    ) {
        let workbook = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="{sheet}" sheetId="1" r:id="rId1"/></sheets><externalReferences><externalReference r:id="rId2"/></externalReferences><definedNames>{defined_names}</definedNames></workbook>"#
        );
        let link = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><externalBook r:id="rId1"><sheetNames><sheetName val="Inputs"/></sheetNames><sheetDataSet><sheetData sheetId="0">{cache_rows}</sheetData></sheetDataSet></externalBook></externalLink>"#
        );
        let link_rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLinkPath" Target="{}" TargetMode="External"/></Relationships>"#,
            xml_attr(link_target)
        );
        write_owned(
            path,
            &[
                (
                    "[Content_Types].xml",
                    content_types(
                        r#"<Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/>"#,
                    ),
                ),
                (
                    "_rels/.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/workbook.xml", workbook),
                (
                    "xl/_rels/workbook.xml.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="externalLinks/externalLink1.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/externalLinks/externalLink1.xml", link),
                ("xl/externalLinks/_rels/externalLink1.xml.rels", link_rels),
                ("xl/worksheets/sheet1.xml", worksheet_xml(cells)),
            ],
        );
    }

    fn cache_numeric_rows(count: usize) -> String {
        (1..=count)
            .map(|row| format!(r#"<row r="{row}"><cell r="A{row}"><v>{row}</v></cell></row>"#))
            .collect()
    }

    fn numbered_sheet_cells(count: usize) -> String {
        (1..=count)
            .map(|row| format!(r#"<c r="A{row}"><v>{row}</v></c>"#))
            .collect::<Vec<_>>()
            .join("")
    }

    fn numbered_sheet_xml(count: usize) -> String {
        let rows: String = (1..=count)
            .map(|row| format!(r#"<row r="{row}"><c r="A{row}"><v>{row}</v></c></row>"#))
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
        )
    }

    fn write_plain_numbered(path: &Path, count: usize) {
        write_plain_workbook_xml(path, &numbered_sheet_xml(count));
    }

    fn write_two_link_workbook(
        path: &Path,
        first_target: &str,
        second_target: &str,
        first_cache: &str,
        second_cache: &str,
        cells: &str,
    ) {
        let workbook = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Report" sheetId="1" r:id="rId1"/></sheets><externalReferences><externalReference r:id="rId2"/><externalReference r:id="rId3"/></externalReferences></workbook>"#.to_string();
        let link = |cache: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><externalBook r:id="rId1"><sheetNames><sheetName val="Inputs"/></sheetNames><sheetDataSet><sheetData sheetId="0">{cache}</sheetData></sheetDataSet></externalBook></externalLink>"#
            )
        };
        let rels = |target: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLinkPath" Target="{}" TargetMode="External"/></Relationships>"#,
                xml_attr(target)
            )
        };
        write_owned(
            path,
            &[
                (
                    "[Content_Types].xml",
                    content_types(
                        r#"<Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/><Override PartName="/xl/externalLinks/externalLink2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/>"#,
                    ),
                ),
                (
                    "_rels/.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/workbook.xml", workbook),
                (
                    "xl/_rels/workbook.xml.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="externalLinks/externalLink1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="externalLinks/externalLink2.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/externalLinks/externalLink1.xml", link(first_cache)),
                (
                    "xl/externalLinks/_rels/externalLink1.xml.rels",
                    rels(first_target),
                ),
                ("xl/externalLinks/externalLink2.xml", link(second_cache)),
                (
                    "xl/externalLinks/_rels/externalLink2.xml.rels",
                    rels(second_target),
                ),
                ("xl/worksheets/sheet1.xml", worksheet_xml(cells)),
            ],
        );
    }

    fn temp_root(label: &str) -> (std::path::PathBuf, TempCleanup) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("omasheets-{label}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        (root.clone(), TempCleanup(root))
    }

    #[test]
    fn external_file_urls_handle_short_and_unicode_targets_without_panicking() {
        for target in ["file://a", "file://é.xlsx", "file://東京.xlsx", "file://"] {
            let _ = local_relative_target(target);
        }
        assert_eq!(strip_ascii_prefix("é", "localhost"), None);
        assert_eq!(strip_ascii_prefix("12345678é", "localhost"), None);
        assert_eq!(
            strip_ascii_prefix("LOCALHOST/a.xlsx", "localhost"),
            Some("/a.xlsx")
        );
    }

    #[test]
    fn external_path_rejects_symlinks_that_escape_the_source_directory() {
        let (root, _cleanup) = temp_root("symlink");
        let source_dir = root.join("source");
        let outside = root.join("outside");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("Secret.xlsx");
        write_plain_numbered(&secret, 1);
        let file_link = source_dir.join("Book.xlsx");
        std::os::unix::fs::symlink(&secret, &file_link).unwrap();
        assert!(resolve_external_path(&source_dir, "Book.xlsx").is_none());
        assert!(resolve_external_path(&source_dir, "Book%2exlsx").is_none());

        let nested = source_dir.join("links");
        std::os::unix::fs::symlink(&outside, &nested).unwrap();
        let via_dir = nested.join("Secret.xlsx");
        assert!(via_dir.is_file());
        assert!(resolve_external_path(&source_dir, "links/Secret.xlsx").is_none());
        assert!(resolve_external_path(&source_dir, "links%2fSecret.xlsx").is_none());

        let local = source_dir.join("Local.xlsx");
        write_plain_numbered(&local, 1);
        assert_eq!(
            resolve_external_path(&source_dir, "Local.xlsx").as_deref(),
            Some(local.as_path())
        );
        assert!(
            resolve_external_path(
                &source_dir,
                secret.canonicalize().unwrap().to_str().unwrap()
            )
            .is_none()
        );
        let escaped_absolute_name = source_dir.join("Secret.xlsx");
        std::os::unix::fs::symlink(&secret, &escaped_absolute_name).unwrap();
        assert!(
            resolve_external_path(
                &source_dir,
                secret.canonicalize().unwrap().to_str().unwrap()
            )
            .is_none()
        );
        assert!(
            resolve_external_path(
                &source_dir,
                &format!("file://{}", secret.canonicalize().unwrap().display())
            )
            .is_none()
        );
    }

    #[test]
    fn cached_external_strings_keep_decoded_whitespace() {
        let (root, _cleanup) = temp_root("cache-ws");
        let source = root.join("Source.xlsx");
        write_linked_workbook_cache(
            &source,
            "Report",
            r#"<c r="A1"><f>[Missing.xlsx]Inputs!A1</f><v>0</v></c><c r="B1"><f>[Missing.xlsx]Inputs!A2</f><v>0</v></c><c r="C1"><f>[Missing.xlsx]Inputs!A3</f><v>0</v></c>"#,
            "",
            "Missing.xlsx",
            r#"<row r="1"><cell r="A1" t="str"><v>  padded  </v></cell></row><row r="2"><cell r="A2" t="str"><v>   </v></cell></row><row r="3"><cell r="A3" t="str"><v></v></cell></row>"#,
        );
        let imported = import_xlsx(&source, ImportLimits::default()).unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Text("  padded  ".into())
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 1)),
            Value::Text("   ".into())
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 2)),
            Value::Text(String::new())
        );
    }

    #[test]
    fn shared_import_budget_covers_cached_links_opened_targets_and_root_cells() {
        let (root, _cleanup) = temp_root("budget");
        let tight = ImportLimits {
            max_cells: 2,
            ..ImportLimits::default()
        };
        let exact_four = ImportLimits {
            max_cells: 4,
            ..ImportLimits::default()
        };

        let oversized = root.join("oversized.xlsx");
        write_linked_workbook_cache(
            &oversized,
            "Report",
            r#"<c r="A1"><f>[Missing.xlsx]Inputs!A1</f><v>0</v></c>"#,
            "",
            "Missing.xlsx",
            &cache_numeric_rows(3),
        );
        assert_eq!(
            import_xlsx(&oversized, tight).err(),
            Some(ImportError::TooManyCells {
                observed: 3,
                maximum: 2,
            })
        );

        let multi = root.join("multi.xlsx");
        write_two_link_workbook(
            &multi,
            "MissingA.xlsx",
            "MissingB.xlsx",
            &cache_numeric_rows(2),
            &cache_numeric_rows(2),
            r#"<c r="A1"><f>[MissingA.xlsx]Inputs!A1</f><v>0</v></c>"#,
        );
        assert_eq!(
            import_xlsx(&multi, tight).err(),
            Some(ImportError::TooManyCells {
                observed: 3,
                maximum: 2,
            })
        );
        let multi_ok = import_xlsx(
            &multi,
            ImportLimits {
                max_cells: 5,
                ..ImportLimits::default()
            },
        )
        .unwrap();
        assert_eq!(
            multi_ok.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(1.0)
        );

        let opened = root.join("opened");
        std::fs::create_dir_all(&opened).unwrap();
        write_plain_numbered(&opened.join("One.xlsx"), 2);
        write_plain_numbered(&opened.join("Two.xlsx"), 2);
        let opened_source = opened.join("Source.xlsx");
        write_two_link_workbook(
            &opened_source,
            "One.xlsx",
            "Two.xlsx",
            &cache_numeric_rows(1),
            &cache_numeric_rows(1),
            r#"<c r="A1"><f>[One.xlsx]Sheet1!A1+[Two.xlsx]Sheet1!A1</f><v>0</v></c>"#,
        );
        assert_eq!(
            import_xlsx(&opened_source, exact_four).err(),
            Some(ImportError::TooManyCells {
                observed: 5,
                maximum: 4,
            })
        );
        let opened_ok = import_xlsx(
            &opened_source,
            ImportLimits {
                max_cells: 5,
                ..ImportLimits::default()
            },
        )
        .unwrap();
        assert_eq!(
            opened_ok.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(2.0)
        );

        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        write_plain_numbered(&nested.join("Leaf.xlsx"), 2);
        write_linked_workbook(
            &nested.join("Mid.xlsx"),
            "Inputs",
            r#"<c r="A1"><f>[Leaf.xlsx]Sheet1!A1+[Leaf.xlsx]Sheet1!A2</f><v>99</v></c>"#,
            "",
            "Leaf.xlsx",
            "1",
        );
        write_linked_workbook(
            &nested.join("Top.xlsx"),
            "Report",
            r#"<c r="A1"><f>[Mid.xlsx]Inputs!A1+1</f><v>0</v></c>"#,
            "",
            "Mid.xlsx",
            "0",
        );
        assert_eq!(
            import_xlsx(&nested.join("Top.xlsx"), tight).err(),
            Some(ImportError::TooManyCells {
                observed: 3,
                maximum: 2,
            })
        );
        let nested_ok = import_xlsx(
            &nested.join("Top.xlsx"),
            ImportLimits {
                max_cells: 4,
                ..ImportLimits::default()
            },
        )
        .unwrap();
        assert_eq!(
            nested_ok.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(4.0)
        );

        let mix = root.join("mix");
        std::fs::create_dir_all(&mix).unwrap();
        write_plain_numbered(&mix.join("Book.xlsx"), 2);
        let mix_source = mix.join("Source.xlsx");
        write_linked_workbook(
            &mix_source,
            "Report",
            &numbered_sheet_cells(2),
            "",
            "Book.xlsx",
            "1",
        );
        assert_eq!(
            import_xlsx(&mix_source, exact_four)
                .unwrap()
                .workbook
                .value(CellId::new(0, 0, 0)),
            Value::Number(1.0)
        );
        write_linked_workbook(
            &mix_source,
            "Report",
            &numbered_sheet_cells(3),
            "",
            "Book.xlsx",
            "1",
        );
        assert_eq!(
            import_xlsx(&mix_source, exact_four).err(),
            Some(ImportError::TooManyCells {
                observed: 5,
                maximum: 4,
            })
        );

        let swallow = root.join("swallow");
        std::fs::create_dir_all(&swallow).unwrap();
        write_plain_numbered(&swallow.join("Huge.xlsx"), 3);
        let swallow_source = swallow.join("Source.xlsx");
        write_linked_workbook(
            &swallow_source,
            "Report",
            r#"<c r="A1"><f>[Huge.xlsx]Sheet1!A1</f><v>0</v></c>"#,
            "",
            "Huge.xlsx",
            "9",
        );
        assert_eq!(
            import_xlsx(&swallow_source, tight).err(),
            Some(ImportError::TooManyCells {
                observed: 3,
                maximum: 2,
            })
        );
    }

    /// Empty self-closing `sheetData` elements, a cached `E10` of 33926, and
    /// `TODAY()`/`NOW()` replayed from their cached serials. Import requires
    /// linked inputs, so a missing cell keeps its source cache and is counted
    /// unsupported instead of being installed as a blank or `#REF!`. A
    /// same-named file for an absolute target must not replace that cache.
    #[test]
    fn enron_mismatch_imports_cached_today_and_external_cell() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "omasheets-enron-mismatch-{}-{nonce}",
            std::process::id()
        ));
        let _cleanup = TempCleanup(root.clone());
        let source_dir = root.join("source");
        let source = source_dir.join("Source.xlsx");
        let cells = r#"<row r="1"><c r="A1"><f>TODAY()</f><v>41885</v></c><c r="B1"><f>A1+1</f><v>41886</v></c><c r="C1"><f>NOW()</f><v>41885.25</v></c><c r="D1"><f>[1]Missing!A1</f><v>0</v></c></row><row r="10"><c r="E10"><f>[1]Nominations!E$10</f><v>33926</v></c><c r="Z10"><f>[1]Nominations!Z$10</f><v>0</v></c></row>"#;
        let link = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><externalBook r:id="rId1"><sheetNames><sheetName val="Summary"/><sheetName val="FUGG OBA"/><sheetName val="Nominations"/></sheetNames><sheetDataSet><sheetData sheetId="0"/><sheetData sheetId="1"/><sheetData sheetId="2"><row r="10"><cell r="E10"><v>33926</v></cell></row></sheetData></sheetDataSet></externalBook></externalLink>"#;
        write_custom_linked_workbook(
            &source,
            "Nominations",
            &format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{cells}</sheetData></worksheet>"#
            ),
            "",
            "Nov%20OBA%20Balance.xls",
            link,
        );
        let imported = import_xlsx(&source, ImportLimits::default()).unwrap();
        let mut unresolved: Vec<_> = imported.unsupported.iter().map(|item| item.cell).collect();
        unresolved.sort();
        assert_eq!(
            unresolved,
            vec![CellId::new(0, 0, 3), CellId::new(0, 9, 25)]
        );
        assert!(
            imported
                .unsupported
                .iter()
                .all(|item| { matches!(item.error, FormulaError::ExternalReference(_)) })
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 9, 4)),
            Value::Number(33926.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 9, 25)),
            Value::Number(0.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 0)),
            Value::Number(41885.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 1)),
            Value::Number(41886.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 2)),
            Value::Number(41885.25)
        );
        // `[1]Missing!A1` names a sheet the link does not contain. Import keeps
        // the cached 0 rather than installing `#REF!`.
        assert_eq!(
            imported.workbook.value(CellId::new(0, 0, 3)),
            Value::Number(0.0)
        );

        let decoy_dir = root.join("decoy");
        let decoy = decoy_dir.join("Nominations.xlsx");
        write_plain_workbook(&decoy, "Nominations", r#"<c r="E10"><v>7</v></c>"#);
        let flat = root.join("flat");
        let flat_decoy = flat.join("Nominations.xlsx");
        write_plain_workbook(&flat_decoy, "Nominations", r#"<c r="E10"><v>7</v></c>"#);
        let absolute = decoy.canonicalize().unwrap();
        let book = flat.join("Book.xlsx");
        write_custom_linked_workbook(
            &book,
            "Report",
            &worksheet_xml(r#"<c r="E10"><f>[1]Nominations!E$10</f><v>33926</v></c>"#),
            "",
            absolute.to_str().unwrap(),
            link,
        );
        let kept = import_xlsx(&book, ImportLimits::default()).unwrap();
        assert_eq!(
            kept.workbook.value(CellId::new(0, 9, 4)),
            Value::Number(33926.0),
            "a same-named file must not replace the populated cache"
        );
    }

    fn write_custom_linked_workbook(
        path: &Path,
        sheet: &str,
        worksheet: &str,
        defined_names: &str,
        link_target: &str,
        link: &str,
    ) {
        let workbook = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="{sheet}" sheetId="1" r:id="rId1"/></sheets><externalReferences><externalReference r:id="rId2"/></externalReferences><definedNames>{defined_names}</definedNames></workbook>"#
        );
        let link_rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLinkPath" Target="{}" TargetMode="External"/></Relationships>"#,
            xml_attr(link_target)
        );
        write_owned(
            path,
            &[
                (
                    "[Content_Types].xml",
                    content_types(
                        r#"<Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/>"#,
                    ),
                ),
                (
                    "_rels/.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/workbook.xml", workbook),
                (
                    "xl/_rels/workbook.xml.rels",
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="externalLinks/externalLink1.xml"/></Relationships>"#.to_string(),
                ),
                ("xl/externalLinks/externalLink1.xml", link.to_string()),
                ("xl/externalLinks/_rels/externalLink1.xml.rels", link_rels),
                ("xl/worksheets/sheet1.xml", worksheet.to_string()),
            ],
        );
    }

    #[test]
    fn enron_mismatch_real_enron_files_replay_33926_and_today_serial() {
        let root = Path::new("/Users/markwatts/omasheets-corpus/enron-figshare/enron-figshare");
        let external = root.join("paul_lucci__28399__Enron Daily Email Nov '01.xlsx");
        let today = root.join("kevin_presto__19778__Crude.xlsx");
        if !external.is_file() || !today.is_file() {
            return;
        }
        let imported = import_xlsx(&external, ImportLimits::default()).unwrap();
        assert_eq!(
            imported.workbook.value(CellId::new(0, 9, 4)),
            Value::Number(33926.0)
        );
        assert_eq!(
            imported.workbook.value(CellId::new(0, 9, 25)),
            Value::Number(0.0)
        );
        assert_eq!(imported.parity().stored_values_mismatched, 0);
        let crude = import_xlsx(&today, ImportLimits::default()).unwrap();
        let main = crude
            .sheets
            .iter()
            .find(|sheet| sheet.name == "MAIN")
            .expect("MAIN");
        assert_eq!(
            crude.workbook.value(CellId::new(main.index, 30, 6)),
            Value::Number(41885.0)
        );
    }

    struct TempCleanup(std::path::PathBuf);

    impl Drop for TempCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
