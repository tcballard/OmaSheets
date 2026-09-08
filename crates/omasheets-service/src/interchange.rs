//! Bounded SpreadsheetML presentation conversion. Values remain the responsibility
//! of the calculation importer; this layer never extracts files or follows links.
use crate::{ServiceError, xml_text};
use omasheets_core::presentation::{
    Alignment, Border, CellStyle, PresentedCell, Region, SheetPresentation,
};
use omasheets_core::{Document, SheetId};
use quick_xml::{Reader, events::Event};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

type Attributes = BTreeMap<String, String>;
type Rectangle = (usize, usize, usize, usize);
const PART_LIMIT: u64 = 32 * 1024 * 1024;
const TOTAL_LIMIT: usize = 128 * 1024 * 1024;

fn invalid(message: impl ToString) -> ServiceError {
    ServiceError::new("xlsx_presentation", message.to_string())
}

fn tags(
    xml: &str,
    mut visit: impl FnMut(&[String], &Attributes) -> Result<(), ServiceError>,
) -> Result<(), ServiceError> {
    let mut reader = Reader::from_str(xml);
    let mut path = Vec::new();
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(tag) | Event::Empty(tag) => {
                let empty =
                    xml.as_bytes().get(reader.buffer_position() as usize - 2) == Some(&b'/');
                path.push(String::from_utf8_lossy(tag.local_name().as_ref()).into_owned());
                if path.len() > 64 {
                    return Err(invalid("XML nesting exceeds 64 elements"));
                }
                let mut attrs = Attributes::new();
                for attr in tag.attributes() {
                    let attr = attr.map_err(invalid)?;
                    attrs.insert(
                        String::from_utf8_lossy(attr.key.as_ref()).into_owned(),
                        attr.decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(invalid)?
                        .into_owned(),
                    );
                }
                visit(&path, &attrs)?;
                if empty {
                    path.pop();
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::DocType(_) => return Err(invalid("DTD declarations are not supported")),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

fn part(
    archive: &mut zip::ZipArchive<File>,
    name: &str,
    budget: &mut usize,
) -> Result<Option<String>, ServiceError> {
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(invalid(error)),
    };
    if file.size() > PART_LIMIT || file.size() as usize > *budget {
        return Err(invalid("Expanded presentation XML exceeds its size limit"));
    }
    let mut text = String::new();
    (&mut file)
        .take(PART_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(invalid)?;
    if text.len() > PART_LIMIT as usize || text.len() > *budget {
        return Err(invalid("Expanded XML exceeds its size limit"));
    }
    *budget -= text.len();
    Ok(Some(text))
}

fn target_path(target: &str) -> Result<String, ServiceError> {
    let mut parts = if target.starts_with('/') {
        Vec::new()
    } else {
        vec!["xl"]
    };
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(invalid("Invalid package relationship"));
                }
            }
            _ if segment.contains(['\\', ':']) => {
                return Err(invalid("External package relationship"));
            }
            _ => parts.push(segment),
        }
    }
    Ok(parts.join("/"))
}

fn index(attrs: &Attributes, name: &str, default: usize) -> Result<usize, ServiceError> {
    attrs
        .get(name)
        .map_or(Ok(default), |value| value.parse().map_err(invalid))
}
fn number(attrs: &Attributes, name: &str) -> Result<Option<f64>, ServiceError> {
    attrs
        .get(name)
        .map(|text| {
            text.parse::<f64>().map_err(invalid).and_then(|n| {
                if n.is_finite() {
                    Ok(n)
                } else {
                    Err(invalid("Non-finite presentation number"))
                }
            })
        })
        .transpose()
}
fn flag(attrs: &Attributes, name: &str) -> bool {
    attrs
        .get(name)
        .is_some_and(|value| value == "1" || value == "true")
}
fn enabled(attrs: &Attributes) -> bool {
    !attrs
        .get("val")
        .is_some_and(|value| value == "0" || value == "false" || value == "none")
}
fn colour(attrs: &Attributes, losses: &mut BTreeSet<String>) -> Option<String> {
    if let Some(rgb) = attrs
        .get("rgb")
        .filter(|rgb| matches!(rgb.len(), 6 | 8) && rgb.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        Some(format!("#{}", &rgb[rgb.len() - 6..]).to_lowercase())
    } else {
        if attrs.contains_key("theme") || attrs.contains_key("indexed") {
            losses.insert(
                "Theme/indexed colours use the native palette; explicit RGB colours are preserved."
                    .into(),
            );
        }
        None
    }
}

fn styles(
    xml: Option<&str>,
    losses: &mut BTreeSet<String>,
) -> Result<Vec<CellStyle>, ServiceError> {
    let Some(xml) = xml else {
        return Ok(vec![CellStyle::default()]);
    };
    let mut fonts: Vec<CellStyle> = Vec::new();
    let mut fills: Vec<Option<String>> = Vec::new();
    let mut borders: Vec<[bool; 4]> = Vec::new();
    let mut formats = BTreeMap::from([
        (0, "General".into()),
        (1, "0".into()),
        (2, "0.00".into()),
        (3, "#,##0".into()),
        (4, "#,##0.00".into()),
        (9, "0%".into()),
        (10, "0.00%".into()),
        (14, "mm/dd/yyyy".into()),
    ]);
    let mut output = Vec::new();
    tags(xml, |path, attrs| {
        let key = path.iter().map(String::as_str).collect::<Vec<_>>();
        match key.as_slice() {
            ["styleSheet", "numFmts", "numFmt"] => {
                formats.insert(
                    index(attrs, "numFmtId", 0)?,
                    attrs.get("formatCode").cloned().unwrap_or_default(),
                );
            }
            ["styleSheet", "fonts", "font"] => fonts.push(CellStyle::default()),
            ["styleSheet", "fonts", "font", name] => {
                let font = fonts
                    .last_mut()
                    .ok_or_else(|| invalid("Font element order"))?;
                match *name {
                    "b" => font.bold = enabled(attrs),
                    "i" => font.italic = enabled(attrs),
                    "u" => font.underline = enabled(attrs),
                    "sz" => {
                        font.font_size = number(attrs, "val")?;
                        if font
                            .font_size
                            .is_some_and(|size| !(6.0..=72.0).contains(&size))
                        {
                            font.font_size = None;
                            losses.insert(
                                "Font sizes outside 6–72 points use the native default.".into(),
                            );
                        }
                    }
                    "color" => font.foreground = colour(attrs, losses),
                    "strike" | "vertAlign" | "outline" | "shadow" => {
                        losses.insert(
                            "Strike-through, superscript and decorative font effects are omitted."
                                .into(),
                        );
                    }
                    _ => {}
                }
            }
            ["styleSheet", "fills", "fill"] => fills.push(None),
            ["styleSheet", "fills", "fill", "patternFill"] => {
                if attrs
                    .get("patternType")
                    .is_some_and(|kind| !["none", "gray125", "solid"].contains(&kind.as_str()))
                {
                    losses.insert("Pattern fills are reduced to their foreground colour.".into());
                }
            }
            ["styleSheet", "fills", "fill", "patternFill", "fgColor"] => {
                *fills
                    .last_mut()
                    .ok_or_else(|| invalid("Fill element order"))? = colour(attrs, losses)
            }
            ["styleSheet", "borders", "border"] => borders.push([false; 4]),
            ["styleSheet", "borders", "border", side] => {
                if let Some(i) = ["left", "right", "top", "bottom"]
                    .iter()
                    .position(|s| s == side)
                {
                    borders.last_mut().ok_or_else(|| invalid("Border order"))?[i] =
                        attrs.get("style").is_some_and(|style| style != "none");
                }
            }
            ["styleSheet", "cellXfs", "xf"] => {
                let mut style = fonts
                    .get(index(attrs, "fontId", 0)?)
                    .cloned()
                    .ok_or_else(|| invalid("Unknown font index"))?;
                style.background = fills
                    .get(index(attrs, "fillId", 0)?)
                    .cloned()
                    .ok_or_else(|| invalid("Unknown fill index"))?;
                let border = borders
                    .get(index(attrs, "borderId", 0)?)
                    .ok_or_else(|| invalid("Unknown border index"))?;
                style.border = if border.iter().all(|b| *b) {
                    Border::All
                } else if *border == [false, false, false, true] {
                    Border::Bottom
                } else {
                    if border.iter().any(|b| *b) {
                        losses.insert(
                            "Partial borders other than a bottom border are omitted.".into(),
                        );
                    }
                    Border::None
                };
                let id = index(attrs, "numFmtId", 0)?;
                style.number_format = formats.get(&id).cloned().unwrap_or_else(|| {
                    losses.insert(format!(
                        "Built-in number format {id} uses General formatting."
                    ));
                    String::new()
                });
                if style.number_format == "General" {
                    style.number_format.clear();
                }
                output.push(style);
            }
            ["styleSheet", "cellXfs", "xf", "alignment"] => {
                let style = output
                    .last_mut()
                    .ok_or_else(|| invalid("Alignment order"))?;
                style.wrap = flag(attrs, "wrapText");
                style.alignment = match attrs.get("horizontal").map(String::as_str) {
                    Some("left") => Alignment::Left,
                    Some("center") => Alignment::Center,
                    Some("right") => Alignment::Right,
                    _ => Alignment::General,
                };
                if attrs.contains_key("textRotation")
                    || attrs.contains_key("indent")
                    || flag(attrs, "shrinkToFit")
                {
                    losses
                        .insert("Text rotation, indentation and shrink-to-fit are omitted.".into());
                }
            }
            _ => {}
        }
        if fonts.len() + fills.len() + borders.len() + formats.len() + output.len() > 20000 {
            return Err(invalid("Workbook style table exceeds 20,000 records"));
        }
        Ok(())
    })?;
    if output.is_empty() {
        output.push(CellStyle::default());
    }
    for style in &output {
        style.validate().map_err(invalid)?;
    }
    Ok(output)
}

fn address(text: &str) -> Result<(usize, usize), ServiceError> {
    let (row, col) = omasheets_core::parse_a1(text)
        .ok_or_else(|| invalid("Invalid presentation cell address"))?;
    Ok((row, col))
}

#[derive(Default)]
pub(crate) struct Layout {
    pub rows: usize,
    pub columns: usize,
    cells: Vec<(usize, usize, CellStyle)>,
    heights: BTreeMap<usize, f64>,
    widths: BTreeMap<usize, f64>,
    merges: Vec<Rectangle>,
    frozen_rows: usize,
    frozen_columns: usize,
    hide_grid: bool,
}
impl Layout {
    pub fn bind(
        &self,
        document: &Document,
        sheet: SheetId,
    ) -> Result<SheetPresentation, ServiceError> {
        let rows = document.rows(sheet).unwrap_or(&[]);
        let cols = document.columns(sheet).unwrap_or(&[]);
        let mut presentation = SheetPresentation {
            frozen_rows: self.frozen_rows,
            frozen_columns: self.frozen_columns,
            show_grid_lines: !self.hide_grid,
            ..SheetPresentation::default()
        };
        for (r, c, style) in &self.cells {
            presentation.cells.push(PresentedCell {
                row: rows[*r],
                column: cols[*c],
                style: style.clone(),
                note: String::new(),
            });
        }
        for (r, size) in &self.heights {
            presentation.row_heights.insert(rows[*r], *size);
        }
        for (c, size) in &self.widths {
            presentation.column_widths.insert(cols[*c], *size);
        }
        for &(r, c, h, w) in &self.merges {
            presentation.merges.push(Region {
                rows: rows[r..r + h].to_vec(),
                columns: cols[c..c + w].to_vec(),
            });
        }
        presentation.validate(document, sheet).map_err(invalid)?;
        Ok(presentation)
    }
}
pub(crate) struct ImportedPresentation {
    pub sheets: BTreeMap<String, Layout>,
    pub losses: Vec<String>,
}
pub(crate) fn read(path: &Path) -> Result<ImportedPresentation, ServiceError> {
    let mut archive = zip::ZipArchive::new(File::open(path).map_err(invalid)?).map_err(invalid)?;
    let mut budget = TOTAL_LIMIT;
    let workbook = part(&mut archive, "xl/workbook.xml", &mut budget)?
        .ok_or_else(|| invalid("Missing workbook part"))?;
    let rels = part(&mut archive, "xl/_rels/workbook.xml.rels", &mut budget)?
        .ok_or_else(|| invalid("Missing workbook relationships"))?;
    let mut paths = BTreeMap::new();
    let mut styles_path = "xl/styles.xml".to_owned();
    tags(&rels, |path, attrs| {
        if path.last().is_some_and(|name| name == "Relationship")
            && attrs
                .get("TargetMode")
                .is_none_or(|mode| mode != "External")
        {
            if let (Some(id), Some(target), Some(kind)) =
                (attrs.get("Id"), attrs.get("Target"), attrs.get("Type"))
            {
                if kind.ends_with("/worksheet") {
                    paths.insert(id.clone(), target_path(target)?);
                }
                if kind.ends_with("/styles") {
                    styles_path = target_path(target)?;
                }
            }
        }
        Ok(())
    })?;
    let mut sheets = Vec::new();
    tags(&workbook, |path, attrs| {
        if path.last().is_some_and(|name| name == "sheet") {
            if let (Some(name), Some(id)) = (attrs.get("name"), attrs.get("r:id")) {
                if let Some(path) = paths.get(id) {
                    sheets.push((name.clone(), path.clone()));
                }
            }
        }
        Ok(())
    })?;
    let mut losses = BTreeSet::new();
    let style_xml = part(&mut archive, &styles_path, &mut budget)?;
    let styles = styles(style_xml.as_deref(), &mut losses)?;
    let mut output = BTreeMap::new();
    for (name, path) in sheets {
        let xml = part(&mut archive, &path, &mut budget)?
            .ok_or_else(|| invalid("Missing worksheet part"))?;
        let mut layout = Layout::default();
        tags(&xml, |path, attrs| {
            match path.last().map(String::as_str) {
                Some("c") if attrs.contains_key("s") => {
                    let (row, col) = address(
                        attrs
                            .get("r")
                            .ok_or_else(|| invalid("Styled cell needs an address"))?,
                    )?;
                    let style = styles
                        .get(index(attrs, "s", 0)?)
                        .ok_or_else(|| invalid("Unknown cell style"))?;
                    if *style != CellStyle::default() {
                        layout.cells.push((row, col, style.clone()));
                        layout.rows = layout.rows.max(row + 1);
                        layout.columns = layout.columns.max(col + 1);
                    }
                }
                Some("row") => {
                    if let Some(height) = number(attrs, "ht")? {
                        let row = index(attrs, "r", 0)?
                            .checked_sub(1)
                            .ok_or_else(|| invalid("Invalid row height address"))?;
                        let pixels = height * 96.0 / 72.0;
                        if (16.0..=600.0).contains(&pixels) {
                            layout.heights.insert(row, pixels);
                            layout.rows = layout.rows.max(row + 1);
                        } else {
                            losses.insert(
                                "Row heights outside 16–600 pixels use the native default.".into(),
                            );
                        }
                    }
                    if flag(attrs, "hidden") {
                        losses
                            .insert("Manually hidden rows and columns are shown on import.".into());
                    }
                    if attrs.contains_key("s") {
                        losses.insert("Row-wide style defaults are omitted; explicit cell styles are preserved.".into());
                    }
                }
                Some("col") => {
                    if let Some(width) = number(attrs, "width")? {
                        let start = index(attrs, "min", 0)?
                            .checked_sub(1)
                            .ok_or_else(|| invalid("Invalid column width address"))?;
                        let end = index(attrs, "max", 0)?;
                        if end <= start || end > 16384 || end - start > 1000 {
                            return Err(invalid("Column width range exceeds native limits"));
                        }
                        // ISO 29500 maximum-digit-width conversion at 96 dpi, Calibri 11.
                        let pixels =
                            (((256.0 * width + (128.0_f64 / 7.0).floor()) / 256.0) * 7.0).floor();
                        if (24.0..=1200.0).contains(&pixels) {
                            for col in start..end {
                                layout.widths.insert(col, pixels);
                            }
                            layout.columns = layout.columns.max(end);
                        } else {
                            losses.insert(
                                "Column widths outside 24–1,200 pixels use the native default."
                                    .into(),
                            );
                        }
                    }
                    if flag(attrs, "hidden") {
                        losses
                            .insert("Manually hidden rows and columns are shown on import.".into());
                    }
                    if attrs.contains_key("style") {
                        losses.insert("Column-wide style defaults are omitted; explicit cell styles are preserved.".into());
                    }
                }
                Some("mergeCell") => {
                    let reference = attrs
                        .get("ref")
                        .ok_or_else(|| invalid("Missing merge range"))?;
                    let (start, end) = reference
                        .split_once(':')
                        .ok_or_else(|| invalid("Invalid merge range"))?;
                    let (r, c) = address(start)?;
                    let (last_r, last_c) = address(end)?;
                    if last_r < r
                        || last_c < c
                        || (last_r - r + 1).saturating_mul(last_c - c + 1) > 10000
                    {
                        return Err(invalid("Merge range exceeds native limits"));
                    }
                    layout.merges.push((r, c, last_r - r + 1, last_c - c + 1));
                    layout.rows = layout.rows.max(last_r + 1);
                    layout.columns = layout.columns.max(last_c + 1);
                }
                Some("pane") => {
                    if attrs
                        .get("state")
                        .is_some_and(|state| state == "frozen" || state == "frozenSplit")
                    {
                        let r = number(attrs, "ySplit")?.unwrap_or(0.0);
                        let c = number(attrs, "xSplit")?.unwrap_or(0.0);
                        if r < 0.0
                            || c < 0.0
                            || r.fract() != 0.0
                            || c.fract() != 0.0
                            || r > 1048576.0
                            || c > 16384.0
                        {
                            return Err(invalid("Invalid frozen pane boundary"));
                        }
                        layout.frozen_rows = r as usize;
                        layout.frozen_columns = c as usize;
                        layout.rows = layout.rows.max(r as usize);
                        layout.columns = layout.columns.max(c as usize);
                    } else {
                        losses
                            .insert("Split panes are omitted; frozen panes are preserved.".into());
                    }
                }
                Some("sheetView") => {
                    layout.hide_grid = attrs
                        .get("showGridLines")
                        .is_some_and(|value| value == "0" || value == "false")
                }
                Some("conditionalFormatting") => {
                    losses.insert("Conditional formatting rules are omitted on import.".into());
                }
                Some("autoFilter") => {
                    losses.insert("Source filters are cleared on import.".into());
                }
                Some("drawing") | Some("legacyDrawing") => {
                    losses.insert(
                        "Charts, drawings, images and cell comments are omitted on import.".into(),
                    );
                }
                _ => {}
            }
            if layout.cells.len() > 10000
                || layout.heights.len() > 10000
                || layout.widths.len() > 1000
                || layout.merges.len() > 1000
            {
                return Err(invalid("Sheet presentation exceeds native record limits"));
            }
            Ok(())
        })?;
        output.insert(name, layout);
    }
    losses.insert("Font families and default sheet dimensions use the native defaults; custom dimensions assume a 7-pixel maximum digit width.".into());
    Ok(ImportedPresentation {
        sheets: output,
        losses: losses.into_iter().collect(),
    })
}

pub(crate) fn export_styles(styles: &[CellStyle]) -> Result<String, ServiceError> {
    use std::fmt::Write;
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
    );
    write!(out, "<numFmts count=\"{}\">", styles.len()).unwrap();
    for (i, s) in styles.iter().enumerate() {
        write!(
            out,
            "<numFmt numFmtId=\"{}\" formatCode=\"{}\"/>",
            164 + i,
            xml_text(if s.number_format.is_empty() {
                "General"
            } else {
                &s.number_format
            })?
        )
        .unwrap();
    }
    write!(out, "</numFmts><fonts count=\"{}\">", styles.len()).unwrap();
    for s in styles {
        out.push_str("<font><name val=\"Calibri\"/>");
        if s.bold {
            out.push_str("<b/>");
        }
        if s.italic {
            out.push_str("<i/>");
        }
        if s.underline {
            out.push_str("<u/>");
        }
        write!(out, "<sz val=\"{}\"/>", s.font_size.unwrap_or(11.0)).unwrap();
        if let Some(color) = &s.foreground {
            write!(out, "<color rgb=\"FF{}\"/>", &color[1..]).unwrap();
        }
        out.push_str("</font>");
    }
    write!(out,"</fonts><fills count=\"{}\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill>",styles.len()+2).unwrap();
    for s in styles {
        if let Some(color) = &s.background {
            write!(out,"<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF{}\"/><bgColor indexed=\"64\"/></patternFill></fill>",&color[1..]).unwrap();
        } else {
            out.push_str("<fill><patternFill patternType=\"none\"/></fill>");
        }
    }
    write!(out, "</fills><borders count=\"{}\">", styles.len()).unwrap();
    for s in styles {
        out.push_str("<border>");
        for side in ["left", "right", "top", "bottom"] {
            if s.border == Border::All || (s.border == Border::Bottom && side == "bottom") {
                write!(out, "<{side} style=\"thin\"><color auto=\"1\"/></{side}>").unwrap();
            } else {
                write!(out, "<{side}/>").unwrap();
            }
        }
        out.push_str("<diagonal/></border>");
    }
    write!(out,"</borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"{}\">",styles.len()).unwrap();
    for (i, s) in styles.iter().enumerate() {
        write!(out,"<xf numFmtId=\"{}\" fontId=\"{i}\" fillId=\"{}\" borderId=\"{i}\" xfId=\"0\" applyNumberFormat=\"1\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\" applyAlignment=\"1\"><alignment horizontal=\"{}\" wrapText=\"{}\"/></xf>",164+i,i+2,match s.alignment {Alignment::General=>"general",Alignment::Left=>"left",Alignment::Center=>"center",Alignment::Right=>"right"},u8::from(s.wrap)).unwrap();
    }
    out.push_str("</cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>");
    Ok(out)
}
