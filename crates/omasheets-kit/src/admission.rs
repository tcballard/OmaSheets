//! Conservative admission for the subset the owned native conversion preserves.
//! This inspects the entire bounded package, including unused extension parts:
//! merely succeeding at formula parsing is not permission to discard objects.

use omasheets_service::XlsxImportManifest;
use quick_xml::{NsReader, events::Event, name::ResolveResult};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const DOC_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const PART_LIMIT: u64 = 32 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 128 * 1024 * 1024;
const MAX_PARTS: usize = 2_048;
const MAX_REASONS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Workbook,
    Sheet,
    Styles,
    Strings,
    Relationships,
    Types,
    Metadata,
}

fn classify(name: &str) -> Option<Kind> {
    match name {
        "[Content_Types].xml" => Some(Kind::Types),
        "_rels/.rels" | "xl/_rels/workbook.xml.rels" => Some(Kind::Relationships),
        "xl/workbook.xml" => Some(Kind::Workbook),
        "xl/styles.xml" => Some(Kind::Styles),
        "xl/sharedStrings.xml" => Some(Kind::Strings),
        "docProps/app.xml" | "docProps/core.xml" => Some(Kind::Metadata),
        _ if name.starts_with("xl/theme/") && name.ends_with(".xml") => Some(Kind::Metadata),
        _ if name.starts_with("xl/worksheets/")
            && name.ends_with(".xml")
            && !name[14..].contains('/') =>
        {
            Some(Kind::Sheet)
        }
        _ if name.starts_with("xl/worksheets/_rels/") && name.ends_with(".xml.rels") => {
            Some(Kind::Relationships)
        }
        _ => None,
    }
}

fn reason(reasons: &mut BTreeSet<String>, message: impl Into<String>) {
    if reasons.len() < MAX_REASONS {
        reasons.insert(message.into());
    }
}

pub(super) fn inspect(bytes: &[u8]) -> Vec<String> {
    let mut reasons = BTreeSet::new();
    let mut archive = match zip::ZipArchive::new(Cursor::new(bytes)) {
        Ok(archive) => archive,
        Err(error) => return vec![format!("Invalid XLSX package: {error}")],
    };
    if archive.len() > MAX_PARTS {
        return vec![format!("XLSX package exceeds {MAX_PARTS} parts")];
    }
    let mut names = BTreeSet::new();
    let mut parts = Vec::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut file = match archive.by_index(index) {
            Ok(file) => file,
            Err(error) => {
                reason(&mut reasons, format!("Unreadable package part: {error}"));
                continue;
            }
        };
        let name = file.name().to_owned();
        if !names.insert(name.clone()) {
            reason(&mut reasons, "Duplicate XLSX package part");
        }
        if name.starts_with('/')
            || name.contains('\\')
            || name.split('/').any(|part| part == ".." || part == ".")
        {
            reason(&mut reasons, "Noncanonical XLSX package path");
            continue;
        }
        if file.is_dir() {
            continue;
        }
        total = total.saturating_add(file.size());
        if file.size() > PART_LIMIT || total > TOTAL_LIMIT {
            return vec!["Expanded XLSX XML exceeds the bounded native package limit".into()];
        }
        let Some(kind) = classify(&name) else {
            let family = if name.contains("vba") || name.ends_with(".bin") {
                "Macros or binary workbook objects"
            } else if name.contains("externalLink") {
                "External workbook links"
            } else if name.contains("pivot") {
                "Pivot tables or pivot caches"
            } else if name.contains("table") {
                "Excel tables"
            } else if name.contains("drawing") || name.contains("chart") || name.contains("media") {
                "Source drawing/chart objects"
            } else if name.contains("comment") {
                "Source comments or threaded comments"
            } else {
                "Unknown XLSX package objects"
            };
            reason(
                &mut reasons,
                format!("{family} are outside this native-kit admission subset"),
            );
            continue;
        };
        let mut xml = Vec::new();
        if let Err(error) = (&mut file).take(PART_LIMIT + 1).read_to_end(&mut xml) {
            reason(&mut reasons, format!("Unreadable XLSX XML: {error}"));
            continue;
        }
        if xml.len() as u64 > PART_LIMIT {
            return vec!["Expanded XLSX part exceeds its size limit".into()];
        }
        parts.push((name, kind, xml));
    }
    if !names.contains("xl/workbook.xml")
        || !names.contains("[Content_Types].xml")
        || !names.contains("xl/_rels/workbook.xml.rels")
    {
        reason(
            &mut reasons,
            "XLSX is missing required workbook/package parts",
        );
    }
    let implicit_cells = parts
        .iter()
        .any(|(_, kind, xml)| *kind == Kind::Sheet && has_implicit_cell_style(xml));
    let style_requirements = parts
        .iter()
        .find(|(_, kind, _)| *kind == Kind::Styles)
        .map(|(_, _, xml)| inspect_style_defaults(xml))
        .unwrap_or_default();
    if implicit_cells && style_requirements.nondefault_style_zero {
        reason(
            &mut reasons,
            "Implicit source cell style 0 is nondefault; native conversion requires explicit cell styles",
        );
    }
    let themes = parts
        .iter()
        .filter(|(name, _, _)| name.starts_with("xl/theme/"))
        .collect::<Vec<_>>();
    if (style_requirements.theme_text || style_requirements.minor_font) && themes.is_empty() {
        reason(
            &mut reasons,
            "The source font references an unknown/missing theme",
        );
    }
    for (_, _, xml) in themes {
        inspect_theme(xml, style_requirements.minor_font, &mut reasons);
    }
    for (_, kind, xml) in parts {
        inspect_xml(kind, &xml, &mut reasons);
    }
    reasons.into_iter().collect()
}

fn allowed_element(kind: Kind, path: &[String], name: &str) -> bool {
    let known = match kind {
        Kind::Workbook => matches!(
            name,
            "workbook"
                | "fileVersion"
                | "workbookPr"
                | "bookViews"
                | "workbookView"
                | "sheets"
                | "sheet"
                | "calcPr"
                | "definedNames"
        ),
        Kind::Sheet => matches!(
            name,
            "worksheet"
                | "sheetPr"
                | "outlinePr"
                | "pageSetUpPr"
                | "dimension"
                | "sheetViews"
                | "sheetView"
                | "selection"
                | "pane"
                | "sheetFormatPr"
                | "cols"
                | "col"
                | "sheetData"
                | "row"
                | "c"
                | "f"
                | "v"
                | "is"
                | "t"
                | "mergeCells"
                | "mergeCell"
                | "pageMargins"
        ),
        Kind::Strings => matches!(name, "sst" | "si" | "t"),
        Kind::Styles => {
            matches!(
                name,
                "styleSheet"
                    | "numFmts"
                    | "numFmt"
                    | "fonts"
                    | "font"
                    | "name"
                    | "sz"
                    | "b"
                    | "i"
                    | "u"
                    | "color"
                    | "family"
                    | "scheme"
                    | "charset"
                    | "fills"
                    | "fill"
                    | "patternFill"
                    | "fgColor"
                    | "bgColor"
                    | "borders"
                    | "border"
                    | "left"
                    | "right"
                    | "top"
                    | "bottom"
                    | "diagonal"
                    | "cellStyleXfs"
                    | "cellXfs"
                    | "xf"
                    | "alignment"
                    | "protection"
                    | "cellStyles"
                    | "cellStyle"
                    | "dxfs"
                    | "tableStyles"
            ) && !(name == "xf" && path.iter().any(|p| p == "dxfs"))
        }
        Kind::Relationships => matches!(name, "Relationships" | "Relationship"),
        Kind::Types => matches!(name, "Types" | "Default" | "Override"),
        Kind::Metadata => true,
    };
    if !known {
        return false;
    }
    let parent = path.last().map(String::as_str).unwrap_or("");
    match kind {
        Kind::Workbook => match name {
            "workbook" => parent.is_empty(),
            "workbookView" => parent == "bookViews",
            "sheet" => parent == "sheets",
            _ => parent == "workbook",
        },
        Kind::Sheet => match name {
            "worksheet" => parent.is_empty(),
            "outlinePr" | "pageSetUpPr" => parent == "sheetPr",
            "sheetView" => parent == "sheetViews",
            "selection" | "pane" => parent == "sheetView",
            "col" => parent == "cols",
            "row" => parent == "sheetData",
            "c" => parent == "row",
            "f" | "v" | "is" => parent == "c",
            "t" => parent == "is",
            "mergeCell" => parent == "mergeCells",
            _ => parent == "worksheet",
        },
        Kind::Styles => match name {
            "styleSheet" => parent.is_empty(),
            "numFmt" => parent == "numFmts",
            "font" => parent == "fonts",
            "name" | "sz" | "b" | "i" | "u" | "family" | "scheme" | "charset" => parent == "font",
            "color" => ["font", "left", "right", "top", "bottom", "diagonal"].contains(&parent),
            "fill" => parent == "fills",
            "patternFill" => parent == "fill",
            "fgColor" | "bgColor" => parent == "patternFill",
            "border" => parent == "borders",
            "left" | "right" | "top" | "bottom" | "diagonal" => parent == "border",
            "xf" => ["cellStyleXfs", "cellXfs"].contains(&parent),
            "alignment" | "protection" => parent == "xf",
            "cellStyle" => parent == "cellStyles",
            _ => parent == "styleSheet",
        },
        Kind::Strings => match name {
            "sst" => parent.is_empty(),
            "si" => parent == "sst",
            "t" => parent == "si",
            _ => false,
        },
        Kind::Relationships => {
            if name == "Relationships" {
                parent.is_empty()
            } else {
                parent == "Relationships"
            }
        }
        Kind::Types => {
            if name == "Types" {
                parent.is_empty()
            } else {
                parent == "Types"
            }
        }
        Kind::Metadata => true,
    }
}

fn allowed_attributes(kind: Kind, name: &str) -> &'static [&'static str] {
    match (kind, name) {
        (Kind::Workbook, "workbook") => &[],
        (Kind::Workbook, "fileVersion") => &["appName", "lastEdited", "lowestEdited", "rupBuild"],
        (Kind::Workbook, "workbookPr") => &[
            "date1904",
            "defaultThemeVersion",
            "filterPrivacy",
            "saveExternalLinkValues",
        ],
        (Kind::Workbook, "workbookView") => &[
            "visibility",
            "minimized",
            "showHorizontalScroll",
            "showVerticalScroll",
            "showSheetTabs",
            "xWindow",
            "yWindow",
            "windowWidth",
            "windowHeight",
            "tabRatio",
            "firstSheet",
            "activeTab",
            "autoFilterDateGrouping",
        ],
        (Kind::Workbook, "sheet") => &["name", "sheetId", "state", "r:id"],
        (Kind::Workbook, "calcPr") => &[
            "calcId",
            "fullCalcOnLoad",
            "forceFullCalc",
            "calcMode",
            "calcCompleted",
            "fullPrecision",
            "refMode",
            "iterate",
            "iterateCount",
            "iterateDelta",
        ],
        (Kind::Sheet, "sheetPr") => &["filterMode", "published", "syncHorizontal", "syncVertical"],
        (Kind::Sheet, "outlinePr") => &[
            "applyStyles",
            "summaryBelow",
            "summaryRight",
            "showOutlineSymbols",
        ],
        (Kind::Sheet, "pageSetUpPr") => &["autoPageBreaks", "fitToPage"],
        (Kind::Sheet, "dimension") => &["ref"],
        (Kind::Sheet, "sheetView") => &[
            "workbookViewId",
            "tabSelected",
            "showGridLines",
            "showRowColHeaders",
            "showZeros",
            "rightToLeft",
            "showFormulas",
            "view",
            "zoomScale",
            "zoomScaleNormal",
            "zoomScaleSheetLayoutView",
            "zoomScalePageLayoutView",
            "topLeftCell",
            "defaultGridColor",
        ],
        (Kind::Sheet, "selection") => &["pane", "activeCell", "activeCellId", "sqref"],
        (Kind::Sheet, "pane") => &["xSplit", "ySplit", "topLeftCell", "activePane", "state"],
        (Kind::Sheet, "sheetFormatPr") => &[
            "baseColWidth",
            "defaultColWidth",
            "defaultRowHeight",
            "customHeight",
            "zeroHeight",
            "thickTop",
            "thickBottom",
            "outlineLevelRow",
            "outlineLevelCol",
        ],
        (Kind::Sheet, "col") => &[
            "min",
            "max",
            "width",
            "style",
            "hidden",
            "bestFit",
            "customWidth",
            "phonetic",
            "outlineLevel",
            "collapsed",
        ],
        (Kind::Sheet, "row") => &[
            "r",
            "spans",
            "s",
            "customFormat",
            "ht",
            "customHeight",
            "hidden",
            "outlineLevel",
            "collapsed",
            "thickTop",
            "thickBot",
            "ph",
        ],
        (Kind::Sheet, "c") => &["r", "s", "t"],
        (Kind::Sheet, "f") => &["t", "ref", "si", "ca"],
        (Kind::Sheet, "t") | (Kind::Strings, "t") => &["xml:space"],
        (Kind::Sheet, "mergeCells") => &["count"],
        (Kind::Sheet, "mergeCell") => &["ref"],
        (Kind::Sheet, "pageMargins") => &["left", "right", "top", "bottom", "header", "footer"],
        (Kind::Strings, "sst") => &["count", "uniqueCount"],
        (
            Kind::Styles,
            "numFmts" | "fonts" | "fills" | "borders" | "cellStyleXfs" | "cellXfs" | "cellStyles"
            | "dxfs",
        ) => &["count"],
        (Kind::Styles, "numFmt") => &["numFmtId", "formatCode"],
        (Kind::Styles, "name" | "sz" | "b" | "i" | "u" | "family" | "scheme" | "charset") => {
            &["val"]
        }
        (Kind::Styles, "color" | "fgColor" | "bgColor") => {
            &["rgb", "theme", "tint", "indexed", "auto"]
        }
        (Kind::Styles, "patternFill") => &["patternType"],
        (Kind::Styles, "border") => &["diagonalUp", "diagonalDown", "outline"],
        (Kind::Styles, "left" | "right" | "top" | "bottom" | "diagonal") => &["style"],
        (Kind::Styles, "xf") => &[
            "numFmtId",
            "fontId",
            "fillId",
            "borderId",
            "xfId",
            "applyNumberFormat",
            "applyFont",
            "applyFill",
            "applyBorder",
            "applyAlignment",
            "applyProtection",
            "pivotButton",
            "quotePrefix",
        ],
        (Kind::Styles, "alignment") => &[
            "horizontal",
            "vertical",
            "wrapText",
            "textRotation",
            "indent",
            "shrinkToFit",
            "relativeIndent",
            "justifyLastLine",
            "readingOrder",
        ],
        (Kind::Styles, "protection") => &["locked", "hidden"],
        (Kind::Styles, "cellStyle") => &["name", "xfId", "builtinId", "customBuiltin", "hidden"],
        (Kind::Styles, "tableStyles") => &["count", "defaultTableStyle", "defaultPivotStyle"],
        (Kind::Relationships, "Relationship") => &["Id", "Type", "Target", "TargetMode"],
        (Kind::Types, "Default") => &["Extension", "ContentType"],
        (Kind::Types, "Override") => &["PartName", "ContentType"],
        _ => &[],
    }
}

fn true_value(value: &str) -> bool {
    value == "1" || value == "true"
}

/// Visit bounded XML elements with their complete local-name path. Admission's
/// namespace/schema checks still run separately; malformed XML is a refusal.
fn visit_tags(
    xml: &[u8],
    mut visit: impl FnMut(&[String], &BTreeMap<String, String>),
) -> Result<(), ()> {
    let mut reader = NsReader::from_reader(xml);
    let mut path = Vec::new();
    loop {
        match reader.read_event().map_err(|_| ())? {
            Event::Start(tag) | Event::Empty(tag) => {
                let empty = xml.get(reader.buffer_position() as usize - 2) == Some(&b'/');
                path.push(String::from_utf8_lossy(tag.local_name().as_ref()).into_owned());
                if path.len() > 64 {
                    return Err(());
                }
                let mut attrs = BTreeMap::new();
                for attr in tag.attributes() {
                    let attr = attr.map_err(|_| ())?;
                    let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                    let value = attr
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|_| ())?
                        .into_owned();
                    attrs.insert(key, value);
                }
                visit(&path, &attrs);
                if empty {
                    path.pop();
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => return if path.is_empty() { Ok(()) } else { Err(()) },
            _ => {}
        }
    }
}

fn has_implicit_cell_style(xml: &[u8]) -> bool {
    let mut found = false;
    let _ = visit_tags(xml, |path, attrs| {
        if path.last().is_some_and(|name| name == "c") && !attrs.contains_key("s") {
            found = true;
        }
    });
    found
}

#[derive(Default)]
struct StyleRequirements {
    nondefault_style_zero: bool,
    theme_text: bool,
    minor_font: bool,
}

/// Excel gives cells without `s` the first cell XF. The older native converter
/// imports only explicitly assigned cell styles. Admit an implicit XF only
/// when it is the ordinary unformatted default, including its font/fill/border.
fn inspect_style_defaults(xml: &[u8]) -> StyleRequirements {
    let mut result = StyleRequirements::default();
    let (mut fonts, mut fills, mut borders, mut xfs) = (0usize, 0usize, 0usize, 0usize);
    let parsed = visit_tags(xml, |path, attrs| {
        let keys = path.iter().map(String::as_str).collect::<Vec<_>>();
        let value = |key: &str| attrs.get(key).map(String::as_str);
        match keys.as_slice() {
            ["styleSheet", "fonts", "font"] => fonts += 1,
            ["styleSheet", "fills", "fill"] => fills += 1,
            ["styleSheet", "borders", "border"] => borders += 1,
            ["styleSheet", "cellXfs", "xf"] => {
                xfs += 1;
                if xfs == 1
                    && ["fontId", "fillId", "borderId", "numFmtId", "xfId"]
                        .iter()
                        .any(|key| value(key).is_some_and(|v| v != "0"))
                {
                    result.nondefault_style_zero = true;
                }
            }
            ["styleSheet", "fonts", "font", property] => {
                if *property == "color" && value("theme") == Some("1") {
                    result.theme_text = true;
                }
                if *property == "scheme" && value("val") == Some("minor") {
                    result.minor_font = true;
                }
                if fonts == 1 {
                    let nondefault = match *property {
                        "b" | "i" | "u" => {
                            !value("val").is_some_and(|v| ["0", "false", "none"].contains(&v))
                        }
                        "sz" => value("val").and_then(|v| v.parse::<f64>().ok()) != Some(11.0),
                        "color" => {
                            attrs.contains_key("rgb")
                                || attrs.contains_key("indexed")
                                || value("theme").is_some_and(|v| v != "1")
                        }
                        "name" => value("val").is_some_and(|v| v != "Calibri"),
                        _ => false,
                    };
                    result.nondefault_style_zero |= nondefault;
                }
            }
            ["styleSheet", "fills", "fill", "patternFill"] if fills == 1 => {
                result.nondefault_style_zero |= value("patternType").is_some_and(|v| v != "none");
            }
            [
                "styleSheet",
                "fills",
                "fill",
                "patternFill",
                "fgColor" | "bgColor",
            ] if fills == 1 => result.nondefault_style_zero = true,
            ["styleSheet", "borders", "border", _] if borders == 1 => {
                result.nondefault_style_zero |= value("style").is_some_and(|v| v != "none");
            }
            ["styleSheet", "cellXfs", "xf", "alignment"] if xfs == 1 => {
                result.nondefault_style_zero |= value("horizontal").is_some_and(|v| v != "general")
                    || value("wrapText").is_some_and(true_value);
            }
            _ => {}
        }
    });
    if parsed.is_err() {
        result.nondefault_style_zero = true;
    }
    result
}

fn inspect_theme(xml: &[u8], require_minor_font: bool, reasons: &mut BTreeSet<String>) {
    const DRAWING: &[u8] = b"http://schemas.openxmlformats.org/drawingml/2006/main";
    let mut reader = NsReader::from_reader(xml);
    let mut path = Vec::new();
    let mut roots = 0usize;
    let mut dark_text = 0usize;
    let mut minor_font = 0usize;
    let mut understood = true;
    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(_) => {
                understood = false;
                break;
            }
        };
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let empty = xml.get(reader.buffer_position() as usize - 2) == Some(&b'/');
                let name = String::from_utf8_lossy(tag.local_name().as_ref()).into_owned();
                let (namespace, _) = reader.resolver().resolve_element(tag.name());
                if !matches!(namespace,ResolveResult::Bound(ns) if ns.as_ref() == DRAWING) {
                    understood = false;
                }
                if path.is_empty() {
                    roots += 1;
                    if name != "theme" {
                        understood = false;
                    }
                }
                path.push(name.clone());
                if path.len() > 64 {
                    understood = false;
                    break;
                }
                let mut attrs = BTreeMap::new();
                for attr in tag.attributes() {
                    let attr = match attr {
                        Ok(attr) => attr,
                        Err(_) => {
                            understood = false;
                            continue;
                        }
                    };
                    let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                    let value = match attr.decoded_and_normalized_value(
                        quick_xml::XmlVersion::Implicit1_0,
                        reader.decoder(),
                    ) {
                        Ok(value) => value.into_owned(),
                        Err(_) => {
                            understood = false;
                            continue;
                        }
                    };
                    attrs.insert(key, value);
                }
                let keys = path.iter().map(String::as_str).collect::<Vec<_>>();
                if keys.starts_with(&["theme", "themeElements", "clrScheme", "dk1"])
                    && keys.len() > 4
                {
                    if keys.len() != 5 {
                        understood = false;
                    } else {
                        dark_text += 1;
                        let black = match name.as_str() {
                            "sysClr" => {
                                attrs.get("val").is_some_and(|v| v == "windowText")
                                    && attrs
                                        .get("lastClr")
                                        .is_some_and(|v| v.eq_ignore_ascii_case("000000"))
                            }
                            "srgbClr" => attrs
                                .get("val")
                                .is_some_and(|v| v.eq_ignore_ascii_case("000000")),
                            _ => false,
                        };
                        if !black {
                            understood = false;
                        }
                    }
                }
                if require_minor_font
                    && keys.as_slice()
                        == ["theme", "themeElements", "fontScheme", "minorFont", "latin"]
                {
                    minor_font += 1;
                    if attrs.get("typeface").is_none_or(|v| v != "Calibri") {
                        understood = false;
                    }
                }
                if empty {
                    path.pop();
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => {
                if !path.is_empty() {
                    understood = false;
                }
                break;
            }
            Event::DocType(_) | Event::PI(_) | Event::CData(_) => understood = false,
            _ => {}
        }
    }
    if !understood || roots != 1 || dark_text != 1 || (require_minor_font && minor_font != 1) {
        reason(
            reasons,
            "Unknown/custom source theme cannot be resolved to default black text and the native Calibri font",
        );
    }
}
fn inspect_xml(kind: Kind, xml: &[u8], reasons: &mut BTreeSet<String>) {
    let mut reader = NsReader::from_reader(xml);
    let mut path = Vec::<String>::new();
    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(_) => {
                reason(reasons, "Malformed or unsupported XLSX XML");
                return;
            }
        };
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let empty = xml.get(reader.buffer_position() as usize - 2) == Some(&b'/');
                let name = String::from_utf8_lossy(tag.local_name().as_ref()).into_owned();
                let (namespace, _) = reader.resolver().resolve_element(tag.name());
                let namespace_ok = match namespace {
                    ResolveResult::Bound(ns) => match kind {
                        Kind::Workbook | Kind::Sheet | Kind::Styles | Kind::Strings => {
                            ns.as_ref() == MAIN.as_bytes()
                        }
                        Kind::Relationships => ns.as_ref() == REL.as_bytes(),
                        Kind::Types => ns.as_ref() == TYPES.as_bytes(),
                        Kind::Metadata => true,
                    },
                    ResolveResult::Unbound => kind == Kind::Metadata,
                    ResolveResult::Unknown(_) => false,
                };
                if !namespace_ok {
                    reason(reasons, "Unknown or unbound XLSX XML namespace");
                }
                if !allowed_element(kind, &path, &name) {
                    reason(reasons, format!("Unsupported XLSX XML feature: {name}"));
                }
                let mut attrs = BTreeMap::new();
                for attribute in tag.attributes() {
                    let attribute = match attribute {
                        Ok(a) => a,
                        Err(_) => {
                            reason(reasons, "Malformed or duplicate XML attributes");
                            continue;
                        }
                    };
                    let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
                    if key == "xmlns" || key.starts_with("xmlns:") {
                        continue;
                    }
                    let value = match attribute.decoded_and_normalized_value(
                        quick_xml::XmlVersion::Implicit1_0,
                        reader.decoder(),
                    ) {
                        Ok(value) => value.into_owned(),
                        Err(_) => {
                            reason(reasons, "Malformed XML attribute value");
                            continue;
                        }
                    };
                    if kind != Kind::Metadata
                        && key != "mc:Ignorable"
                        && !allowed_attributes(kind, &name).contains(&key.as_str())
                    {
                        reason(
                            reasons,
                            format!("Unsupported XLSX XML attribute: {name}.{key}"),
                        );
                    }
                    attrs.insert(key, value);
                }
                check_values(kind, &path, &name, &attrs, reasons);
                path.push(name);
                if path.len() > 64 {
                    reason(reasons, "XML nesting exceeds 64 elements");
                    return;
                }
                if empty {
                    path.pop();
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::DocType(_) | Event::PI(_) | Event::CData(_) => {
                reason(
                    reasons,
                    "DTD, processing instructions and CDATA are outside the native XML subset",
                );
            }
            Event::Eof => {
                if !path.is_empty() {
                    reason(reasons, "Unclosed XLSX XML elements");
                }
                return;
            }
            _ => {}
        }
    }
}

fn check_values(
    kind: Kind,
    path: &[String],
    name: &str,
    attrs: &BTreeMap<String, String>,
    reasons: &mut BTreeSet<String>,
) {
    let get = |key: &str| attrs.get(key).map(String::as_str);
    let flag = |key: &str| get(key).is_some_and(true_value);
    match (kind, name) {
        (Kind::Relationships, "Relationship") => {
            let target = get("Target").unwrap_or("");
            if get("TargetMode") == Some("External") || target.contains([':', '\\']) {
                reason(reasons, "External package relationships are not imported");
            }
            let kind = get("Type").unwrap_or("").rsplit('/').next().unwrap_or("");
            if ![
                "officeDocument",
                "worksheet",
                "styles",
                "sharedStrings",
                "theme",
                "core-properties",
                "extended-properties",
            ]
            .contains(&kind)
            {
                reason(
                    reasons,
                    "Unknown package relationship objects are not imported",
                );
            }
            if get("Type").is_some_and(|t| {
                !t.starts_with(DOC_REL)
                    && !t.starts_with(
                        "http://schemas.openxmlformats.org/package/2006/relationships/",
                    )
            }) {
                reason(reasons, "Unknown package relationship namespace");
            }
        }
        (Kind::Types, "Default" | "Override") => {
            if get("ContentType").is_some_and(|t| {
                t.contains("macroEnabled") || t.contains("vba") || t.contains("pivot")
            }) {
                reason(reasons, "Macro or pivot content types are not imported");
            }
        }
        (Kind::Workbook, "workbookPr") if flag("date1904") => {
            reason(reasons, "The 1904 date system is not supported");
        }
        (Kind::Workbook, "sheet") if get("state").is_some_and(|s| s != "visible") => {
            reason(reasons, "Hidden worksheet state is not preserved");
        }
        (Kind::Workbook, "calcPr") => {
            if flag("iterate")
                || get("fullPrecision").is_some_and(|v| !true_value(v))
                || get("refMode").is_some_and(|v| v != "A1")
            {
                reason(
                    reasons,
                    "Iterative, precision-as-displayed or R1C1 calculation is not implemented",
                );
            }
        }
        (Kind::Sheet, "c") => {
            if get("t") == Some("e") {
                reason(
                    reasons,
                    "Source error cells/caches cannot be preserved by native conversion",
                );
            }
            if get("t").is_some_and(|v| !["n", "b", "s", "str", "inlineStr", "e"].contains(&v)) {
                reason(
                    reasons,
                    "Unknown or ISO-date cell types are outside the native import subset",
                );
            }
        }
        (Kind::Sheet, "f") if get("t").is_some_and(|v| !["normal", "shared"].contains(&v)) => {
            reason(
                reasons,
                "Array/spilling and data-table formula metadata is not preserved",
            );
        }
        (Kind::Sheet, "row" | "col") => {
            if flag("hidden") || flag("collapsed") || get("outlineLevel").is_some_and(|v| v != "0")
            {
                reason(reasons, "Hidden or grouped rows/columns are not preserved");
            }
            if (name == "row" && attrs.contains_key("s"))
                || (name == "col" && attrs.contains_key("style"))
            {
                reason(reasons, "Row/column-wide style defaults are not preserved");
            }
            if flag("thickTop") || flag("thickBot") || flag("ph") || flag("phonetic") {
                reason(reasons, "Decorative row/column metadata is not preserved");
            }
        }
        (Kind::Sheet, "sheetView") => {
            if flag("rightToLeft")
                || flag("showFormulas")
                || get("showZeros").is_some_and(|v| !true_value(v))
                || get("showRowColHeaders").is_some_and(|v| !true_value(v))
                || get("view").is_some_and(|v| v != "normal")
                || [
                    "zoomScale",
                    "zoomScaleNormal",
                    "zoomScaleSheetLayoutView",
                    "zoomScalePageLayoutView",
                ]
                .iter()
                .any(|k| get(k).is_some_and(|v| v != "100"))
            {
                reason(
                    reasons,
                    "Custom worksheet view/layout settings are not preserved",
                );
            }
        }
        (Kind::Sheet, "pane")
            if get("state").is_some_and(|v| !["frozen", "frozenSplit"].contains(&v)) =>
        {
            reason(reasons, "Split panes are not preserved");
        }
        (Kind::Sheet, "sheetFormatPr") => {
            // Ordinary Excel defaults and the grid's own 27px/132px defaults
            // are known baseline layouts. Other authored defaults are losses.
            if get("defaultRowHeight")
                .is_some_and(|v| !matches!(v.parse::<f64>().ok(), Some(15.0 | 20.25)))
                || get("defaultColWidth")
                    .is_some_and(|v| v.parse::<f64>().ok() != Some(18.85546875))
                || flag("zeroHeight")
                || flag("thickTop")
                || flag("thickBottom")
            {
                reason(
                    reasons,
                    "Custom default worksheet dimensions are not preserved",
                );
            }
        }
        (Kind::Sheet, "pageMargins") => {
            for (key, expected) in [
                ("left", 0.7),
                ("right", 0.7),
                ("top", 0.75),
                ("bottom", 0.75),
                ("header", 0.3),
                ("footer", 0.3),
            ] {
                if get(key).is_some_and(|v| v.parse::<f64>().ok() != Some(expected)) {
                    reason(reasons, "Custom print margins are not preserved");
                }
            }
        }
        (Kind::Sheet, "pageSetUpPr") if flag("fitToPage") => {
            reason(reasons, "Print fitting settings are not preserved");
        }
        (Kind::Styles, "name") if get("val").is_some_and(|v| v != "Calibri") => {
            reason(reasons, "Custom font families are not preserved");
        }
        (Kind::Styles, "u") if get("val").is_some_and(|v| !["single", "none"].contains(&v)) => {
            reason(reasons, "Decorative underlining is not preserved");
        }
        (Kind::Styles, "scheme") if get("val").is_some_and(|v| !["minor", "none"].contains(&v)) => {
            reason(
                reasons,
                "Unknown/major theme font schemes are not preserved",
            );
        }
        (Kind::Styles, "xf") => {
            if [
                "applyFont",
                "applyFill",
                "applyBorder",
                "applyNumberFormat",
                "applyAlignment",
                "applyProtection",
            ]
            .iter()
            .any(|key| get(key).is_some_and(|v| !true_value(v)))
            {
                reason(
                    reasons,
                    "Explicit disabled style application flags are not preserved",
                );
            }
        }
        (Kind::Styles, "color" | "fgColor" | "bgColor") => {
            if attrs.contains_key("theme") && (name != "color" || !path.iter().any(|p| p == "font"))
            {
                reason(
                    reasons,
                    "Theme colours on fills and borders are not preserved",
                );
            }
            if get("tint").is_some_and(|v| v.parse::<f64>().ok() != Some(0.0))
                || get("theme").is_some_and(|v| v != "1")
                || get("indexed").is_some_and(|v| v != "64")
            {
                reason(
                    reasons,
                    "Theme, tinted or indexed colours beyond the default text colour are not preserved",
                );
            }
            if name == "color" && path.iter().any(|p| p == "border") && attrs.contains_key("rgb") {
                reason(reasons, "Explicit border colours are not preserved");
            }
            if let Some(rgb) = get("rgb") {
                if !matches!(rgb.len(), 6 | 8) || !rgb.bytes().all(|b| b.is_ascii_hexdigit()) {
                    reason(
                        reasons,
                        "Style RGB colours require six or eight ASCII hexadecimal digits",
                    );
                } else if rgb.len() == 8
                    && !rgb
                        .get(..2)
                        .is_some_and(|alpha| alpha.eq_ignore_ascii_case("ff"))
                {
                    reason(reasons, "Translucent style colours are not preserved");
                }
            }
        }
        (Kind::Styles, "patternFill")
            if get("patternType").is_some_and(|v| !["none", "gray125", "solid"].contains(&v)) =>
        {
            reason(reasons, "Pattern/gradient fills are not preserved");
        }
        (Kind::Styles, "border") if flag("diagonalUp") || flag("diagonalDown") => {
            reason(reasons, "Diagonal borders are not preserved");
        }
        (Kind::Styles, "left" | "right" | "top" | "bottom" | "diagonal")
            if get("style")
                .is_some_and(|v| !["thin", "none"].contains(&v) || name == "diagonal") =>
        {
            reason(reasons, "Custom border line styles are not preserved");
        }
        (Kind::Styles, "alignment") => {
            if get("horizontal")
                .is_some_and(|v| !["general", "left", "center", "right"].contains(&v))
                || get("vertical").is_some_and(|v| v != "bottom")
                || ["textRotation", "indent", "relativeIndent", "readingOrder"]
                    .iter()
                    .any(|k| get(k).is_some_and(|v| v != "0"))
                || flag("shrinkToFit")
                || flag("justifyLastLine")
            {
                reason(reasons, "Custom alignment effects are not preserved");
            }
        }
        (Kind::Styles, "tableStyles" | "dxfs") if get("count").is_some_and(|v| v != "0") => {
            reason(
                reasons,
                "Table or differential style objects are not preserved by this admission subset",
            );
        }
        (Kind::Styles, "protection")
            if flag("hidden") || get("locked").is_some_and(|v| !true_value(v)) =>
        {
            reason(reasons, "Cell protection metadata is not preserved");
        }
        _ => {}
    }
}

pub(super) fn check_manifest(manifest: &XlsxImportManifest, reasons: &mut Vec<String>) {
    for (count, message) in [
        (
            manifest.formula_cells_cached_only,
            "Some formulas would become cached literal values",
        ),
        (
            manifest.formula_cells_omitted,
            "Some formulas would be omitted",
        ),
        (
            manifest.error_cells_omitted,
            "Some source errors would be omitted",
        ),
        (
            manifest.rejected_value_cells_omitted,
            "Some source values would be rejected",
        ),
        (
            manifest.skipped_source_sheets,
            "Some source sheets would be skipped",
        ),
        (
            manifest.owned_engine_unsupported_formulas,
            "The owned calculation engine cannot compile every source formula",
        ),
    ] {
        if count != 0 {
            reasons.push(format!("{message}: {count}"));
        }
    }
    if manifest.formula_cells_native != manifest.formula_cells_observed {
        reasons.push("Not every source formula became a native formula".into());
    }
    // These are unconditional warnings emitted by the older conversion path.
    // The package/attribute inspection checks their actual occurrence instead.
    // All remaining manifest losses are specific refusals, not blanket policy.
    for limitation in &manifest.limitations {
        if !matches!(
            limitation.as_str(),
            "Font families and default sheet dimensions use the native defaults; custom dimensions assume a 7-pixel maximum digit width."
                | "Theme/indexed colours use the native palette; explicit RGB colours are preserved."
                | "Tables, charts, pivots and macros are not imported."
                | "defined_names_not_imported"
                | "unsupported_formulas_use_cached_values_when_available"
                | "cached_error_values_not_imported"
        ) {
            reasons.push(limitation.clone());
        }
    }
}
