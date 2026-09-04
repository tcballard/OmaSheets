#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(i32, row_count, cxx_name = "rowCount")]
        #[qproperty(i32, column_count, cxx_name = "columnCount")]
        #[qproperty(u64, revision, cxx_name = "revision")]
        #[qproperty(bool, benchmark, cxx_name = "benchmark")]
        #[qproperty(bool, document_mode, cxx_name = "documentMode")]
        #[qproperty(QString, document_name, cxx_name = "documentName")]
        #[qproperty(QString, sheet_name, cxx_name = "sheetName")]
        #[qproperty(i32, sheet_count, cxx_name = "sheetCount")]
        #[qproperty(i32, current_sheet, cxx_name = "currentSheet")]
        #[qproperty(QString, source_status, cxx_name = "sourceStatus")]
        #[qproperty(QString, theme_name, cxx_name = "themeName")]
        #[qproperty(QString, theme_background, cxx_name = "themeBackground")]
        #[qproperty(QString, theme_foreground, cxx_name = "themeForeground")]
        #[qproperty(QString, theme_accent, cxx_name = "themeAccent")]
        #[qproperty(QString, theme_muted, cxx_name = "themeMuted")]
        #[qproperty(QString, theme_red, cxx_name = "themeRed")]
        #[qproperty(QString, theme_green, cxx_name = "themeGreen")]
        #[qproperty(QString, theme_yellow, cxx_name = "themeYellow")]
        #[qproperty(QString, theme_blue, cxx_name = "themeBlue")]
        #[qproperty(QString, theme_magenta, cxx_name = "themeMagenta")]
        type GridModel = super::GridModelRust;

        #[qinvokable]
        #[cxx_name = "cellText"]
        fn cell_text(&self, row: i32, column: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "cellInput"]
        fn cell_input(&self, row: i32, column: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "cellKind"]
        fn cell_kind(&self, row: i32, column: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "columnLabel"]
        fn column_label(&self, column: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "sheetLabel"]
        fn sheet_label(&self, index: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "selectSheet"]
        fn select_sheet(self: Pin<&mut Self>, index: i32);

        #[qinvokable]
        #[cxx_name = "setCellText"]
        fn set_cell_text(self: Pin<&mut Self>, row: i32, column: i32, value: &QString) -> bool;

        #[qinvokable]
        #[cxx_name = "prepareCellEdit"]
        fn prepare_cell_edit(self: Pin<&mut Self>, row: i32, column: i32) -> bool;

        #[qinvokable]
        #[cxx_name = "refreshTheme"]
        fn refresh_theme(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "reportBenchmark"]
        fn report_benchmark(
            &self,
            frames: i32,
            elapsed_seconds: f64,
            p95_frame_ms: f64,
            worst_frame_ms: f64,
            visible_delegates: i32,
        );
    }
}

use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::service_client::GridDocument;
use crate::theme::load_active_theme;

const ROWS: i32 = 1_000_000;
const COLUMNS: i32 = 64;

pub struct GridModelRust {
    row_count: i32,
    column_count: i32,
    revision: u64,
    benchmark: bool,
    document_mode: bool,
    document_name: QString,
    sheet_name: QString,
    sheet_count: i32,
    current_sheet: i32,
    source_status: QString,
    theme_name: QString,
    theme_background: QString,
    theme_foreground: QString,
    theme_accent: QString,
    theme_muted: QString,
    theme_red: QString,
    theme_green: QString,
    theme_yellow: QString,
    theme_blue: QString,
    theme_magenta: QString,
    theme_signature: u64,
    document: Option<GridDocument>,
    edits: BTreeMap<(i32, i32), String>,
    cell_reads: AtomicU64,
    created: Instant,
}

impl Default for GridModelRust {
    fn default() -> Self {
        let theme = load_active_theme();
        let requested = requested_document_path();
        let (
            document,
            row_count,
            column_count,
            document_name,
            sheet_name,
            sheet_count,
            source_status,
        ) = match requested.as_deref().map(|path| {
            GridDocument::open(path, std::env::var("OMASHEETS_BRANCH").ok())
        }) {
            Some(Ok(document)) => {
                let sheet = document
                    .current_sheet()
                    .expect("an opened document has a sheet");
                let rows = sheet.rows.min(i32::MAX as usize) as i32;
                let columns = sheet.columns.min(i32::MAX as usize) as i32;
                let name = document.name.clone();
                let sheet_count = document.sheets.len().min(i32::MAX as usize) as i32;
                (
                    Some(document),
                    rows.max(1),
                    columns.max(1),
                    name,
                    sheet.name,
                    sheet_count,
                    "Connected to the local service".to_string(),
                )
            }
            Some(Err(error)) => (
                None,
                1,
                1,
                "Document unavailable".into(),
                String::new(),
                0,
                error,
            ),
            None => (
                None,
                ROWS,
                COLUMNS,
                "Synthetic operations".into(),
                "Fixture".into(),
                1,
                "Synthetic fixture".into(),
            ),
        };
        Self {
            row_count,
            column_count,
            revision: 0,
            benchmark: std::env::var_os("OMASHEETS_GRID_BENCHMARK").is_some(),
            document_mode: requested.is_some(),
            document_name: document_name.as_str().into(),
            sheet_name: sheet_name.as_str().into(),
            sheet_count,
            current_sheet: 0,
            source_status: source_status.as_str().into(),
            theme_name: theme.name.as_str().into(),
            theme_background: theme.palette.background.as_str().into(),
            theme_foreground: theme.palette.foreground.as_str().into(),
            theme_accent: theme.palette.accent.as_str().into(),
            theme_muted: theme.palette.muted.as_str().into(),
            theme_red: theme.palette.red.as_str().into(),
            theme_green: theme.palette.green.as_str().into(),
            theme_yellow: theme.palette.yellow.as_str().into(),
            theme_blue: theme.palette.blue.as_str().into(),
            theme_magenta: theme.palette.magenta.as_str().into(),
            theme_signature: theme.signature,
            document,
            edits: BTreeMap::new(),
            cell_reads: AtomicU64::new(0),
            created: Instant::now(),
        }
    }
}

impl qobject::GridModel {
    pub fn prepare_cell_edit(mut self: Pin<&mut Self>, row: i32, column: i32) -> bool {
        if row < 0 || row >= self.row_count || column < 0 || column >= self.column_count {
            return false;
        }
        if let Some(document) = &self.document {
            if let Err(error) = document.cell(row as usize, column as usize) {
                self.as_mut().set_source_status(error.as_str().into());
                return false;
            }
            return true;
        }
        !self.document_mode
    }

    pub fn cell_text(&self, row: i32, column: i32) -> QString {
        self.cell_reads.fetch_add(1, Ordering::Relaxed);
        if row < 0 || row >= self.row_count || column < 0 || column >= self.column_count {
            return QString::default();
        }
        if let Some(document) = &self.document {
            return document
                .cell(row as usize, column as usize)
                .map(|cell| cell.display.into())
                .unwrap_or_else(|_| "#SERVICE!".into());
        }
        if self.document_mode {
            return "#SERVICE!".into();
        }
        if let Some(value) = self.edits.get(&(row, column)) {
            return value.as_str().into();
        }
        synthetic_cell(row, column).into()
    }

    pub fn cell_input(&self, row: i32, column: i32) -> QString {
        if row < 0 || row >= self.row_count || column < 0 || column >= self.column_count {
            return QString::default();
        }
        if let Some(document) = &self.document {
            return document
                .cell(row as usize, column as usize)
                .map(|cell| cell.input.into())
                .unwrap_or_else(|_| "#SERVICE!".into());
        }
        self.cell_text(row, column)
    }

    pub fn cell_kind(&self, row: i32, column: i32) -> QString {
        if row < 0 || row >= self.row_count || column < 0 || column >= self.column_count {
            return "blank".into();
        }
        if let Some(document) = &self.document {
            return document
                .cell(row as usize, column as usize)
                .map(|cell| cell.kind.into())
                .unwrap_or_else(|_| "error".into());
        }
        if self.document_mode {
            return "error".into();
        }
        match column % 6 {
            0 | 3 => "number".into(),
            1 => "date".into(),
            4 => "formula".into(),
            _ => "text".into(),
        }
    }

    pub fn column_label(&self, column: i32) -> QString {
        column_letters(column).into()
    }

    pub fn sheet_label(&self, index: i32) -> QString {
        if index < 0 {
            return QString::default();
        }
        if let Some(document) = &self.document {
            return document
                .sheets
                .get(index as usize)
                .map(|sheet| sheet.name.as_str().into())
                .unwrap_or_default();
        }
        if !self.document_mode && index == 0 {
            return "Fixture".into();
        }
        QString::default()
    }

    pub fn select_sheet(mut self: Pin<&mut Self>, index: i32) {
        if index < 0 || index >= self.sheet_count || index == self.current_sheet {
            return;
        }
        let result = self
            .document
            .as_ref()
            .ok_or_else(|| "sheet switching requires a native document".to_string())
            .and_then(|document| document.select_sheet(index as usize));
        match result {
            Ok(sheet) => {
                self.as_mut()
                    .set_row_count(sheet.rows.min(i32::MAX as usize).max(1) as i32);
                self.as_mut()
                    .set_column_count(sheet.columns.min(i32::MAX as usize).max(1) as i32);
                self.as_mut().set_sheet_name(sheet.name.as_str().into());
                self.as_mut().set_current_sheet(index);
                let revision = *self.revision();
                self.as_mut().set_revision(revision.wrapping_add(1));
                self.as_mut()
                    .set_source_status("Switched sheets through stable IDs".into());
            }
            Err(error) => self.as_mut().set_source_status(error.as_str().into()),
        }
    }

    pub fn set_cell_text(
        mut self: Pin<&mut Self>,
        row: i32,
        column: i32,
        value: &QString,
    ) -> bool {
        if row < 0 || row >= self.row_count || column < 0 || column >= self.column_count {
            return false;
        }
        if let Some(document) = &self.document {
            let result = document.set_text(row as usize, column as usize, &value.to_string());
            match result {
                Ok(()) => {
                    let revision = *self.revision();
                    self.as_mut().set_revision(revision.wrapping_add(1));
                    self.as_mut()
                        .set_source_status("Saved through the local service".into());
                    return true;
                }
                Err(error) => self.as_mut().set_source_status(error.as_str().into()),
            }
            return false;
        }
        if self.document_mode {
            return false;
        }
        self.as_mut()
            .rust_mut()
            .edits
            .insert((row, column), value.to_string());
        let revision = *self.revision();
        self.set_revision(revision.wrapping_add(1));
        true
    }

    pub fn refresh_theme(mut self: Pin<&mut Self>) {
        let theme = load_active_theme();
        if theme.signature == self.theme_signature {
            return;
        }
        self.as_mut().set_theme_name(theme.name.as_str().into());
        self.as_mut()
            .set_theme_background(theme.palette.background.as_str().into());
        self.as_mut()
            .set_theme_foreground(theme.palette.foreground.as_str().into());
        self.as_mut()
            .set_theme_accent(theme.palette.accent.as_str().into());
        self.as_mut()
            .set_theme_muted(theme.palette.muted.as_str().into());
        self.as_mut()
            .set_theme_red(theme.palette.red.as_str().into());
        self.as_mut()
            .set_theme_green(theme.palette.green.as_str().into());
        self.as_mut()
            .set_theme_yellow(theme.palette.yellow.as_str().into());
        self.as_mut()
            .set_theme_blue(theme.palette.blue.as_str().into());
        self.as_mut()
            .set_theme_magenta(theme.palette.magenta.as_str().into());
        self.as_mut().rust_mut().theme_signature = theme.signature;
    }

    pub fn report_benchmark(
        &self,
        frames: i32,
        elapsed_seconds: f64,
        p95_frame_ms: f64,
        worst_frame_ms: f64,
        visible_delegates: i32,
    ) {
        let source = if self.document.is_some() {
            "native-document"
        } else if self.document_mode {
            "document-error"
        } else {
            "synthetic"
        };
        println!(
            concat!(
                "OMASHEETS_GRID_BENCHMARK ",
                "{{\"schema\":1,\"fixture\":\"{}\",",
                "\"rows\":{},\"columns\":{},\"frames\":{},",
                "\"elapsed_seconds\":{:.6},\"p95_frame_ms\":{:.6},",
                "\"worst_frame_ms\":{:.6},\"visible_delegates\":{},",
                "\"cell_reads\":{},\"startup_to_report_ms\":{:.3},",
                "\"theme_source\":\"{}\",\"source\":\"{}\",",
                "\"service_requests\":{},\"sheet_count\":{},",
                "\"current_sheet\":{}}}"
            ),
            if source == "synthetic" {
                "synthetic-1000000x64"
            } else {
                "native-document-grid"
            },
            self.row_count,
            self.column_count,
            frames,
            elapsed_seconds,
            p95_frame_ms,
            worst_frame_ms,
            visible_delegates,
            self.cell_reads.load(Ordering::Relaxed),
            self.created.elapsed().as_secs_f64() * 1_000.0,
            if self.theme_signature == 0 {
                "fallback"
            } else {
                "omarchy"
            },
            source,
            self.document
                .as_ref()
                .map_or(0, GridDocument::request_count),
            self.sheet_count,
            self.document
                .as_ref()
                .map_or(self.current_sheet, |document| {
                    document.current_sheet_index() as i32
                }),
        );
    }
}

fn requested_document_path() -> Option<PathBuf> {
    std::env::var_os("OMASHEETS_DOCUMENT")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::args_os()
                .skip(1)
                .map(PathBuf::from)
                .find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == std::ffi::OsStr::new("omasheets"))
                })
        })
}

fn synthetic_cell(row: i32, column: i32) -> String {
    match column % 6 {
        0 => (row + 1).to_string(),
        1 => format!("2026-{:02}-{:02}", row % 12 + 1, row % 28 + 1),
        2 => format!("Account {:05}", row % 10_000),
        3 => format!("{:.2}", ((row * 97 + column * 13) % 100_000) as f64 / 100.0),
        4 => format!("={}{:+}", column_letters((column - 1).max(0)), row % 17),
        _ => {
            if row % 11 == 0 {
                "Reviewed".into()
            } else {
                "Open".into()
            }
        }
    }
}

fn column_letters(column: i32) -> String {
    if column < 0 {
        return String::new();
    }
    let mut index = column as usize;
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).expect("ASCII column label")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_labels_cover_excel_boundaries() {
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(16_383), "XFD");
        assert_eq!(column_letters(-1), "");
    }

    #[test]
    fn synthetic_fixture_is_deterministic() {
        assert_eq!(synthetic_cell(0, 0), "1");
        assert_eq!(synthetic_cell(0, 1), "2026-01-01");
        assert_eq!(synthetic_cell(0, 2), "Account 00000");
        assert_eq!(synthetic_cell(10, 5), "Open");
        assert_eq!(synthetic_cell(11, 5), "Reviewed");
    }
}
