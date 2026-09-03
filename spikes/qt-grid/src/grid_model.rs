#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(i32, row_count)]
        #[qproperty(i32, column_count)]
        #[qproperty(u64, revision)]
        #[qproperty(bool, benchmark)]
        type GridModel = super::GridModelRust;

        #[qinvokable]
        #[cxx_name = "cellText"]
        fn cell_text(&self, row: i32, column: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "cellKind"]
        fn cell_kind(&self, row: i32, column: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "columnLabel"]
        fn column_label(&self, column: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "setCellText"]
        fn set_cell_text(self: Pin<&mut Self>, row: i32, column: i32, value: &QString);

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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const ROWS: i32 = 1_000_000;
const COLUMNS: i32 = 64;

pub struct GridModelRust {
    row_count: i32,
    column_count: i32,
    revision: u64,
    benchmark: bool,
    edits: BTreeMap<(i32, i32), String>,
    cell_reads: AtomicU64,
    created: Instant,
}

impl Default for GridModelRust {
    fn default() -> Self {
        Self {
            row_count: ROWS,
            column_count: COLUMNS,
            revision: 0,
            benchmark: std::env::var_os("OMASHEETS_GRID_BENCHMARK").is_some(),
            edits: BTreeMap::new(),
            cell_reads: AtomicU64::new(0),
            created: Instant::now(),
        }
    }
}

impl qobject::GridModel {
    pub fn cell_text(&self, row: i32, column: i32) -> QString {
        self.cell_reads.fetch_add(1, Ordering::Relaxed);
        if row < 0 || row >= self.row_count || column < 0 || column >= self.column_count {
            return QString::default();
        }
        if let Some(value) = self.edits.get(&(row, column)) {
            return value.as_str().into();
        }
        synthetic_cell(row, column).into()
    }

    pub fn cell_kind(&self, row: i32, column: i32) -> QString {
        if row < 0 || row >= self.row_count || column < 0 || column >= self.column_count {
            return "blank".into();
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

    pub fn set_cell_text(
        mut self: Pin<&mut Self>,
        row: i32,
        column: i32,
        value: &QString,
    ) {
        if row < 0 || row >= self.row_count || column < 0 || column >= self.column_count {
            return;
        }
        self.as_mut()
            .rust_mut()
            .edits
            .insert((row, column), value.to_string());
        let revision = *self.revision();
        self.set_revision(revision.wrapping_add(1));
    }

    pub fn report_benchmark(
        &self,
        frames: i32,
        elapsed_seconds: f64,
        p95_frame_ms: f64,
        worst_frame_ms: f64,
        visible_delegates: i32,
    ) {
        println!(
            concat!(
                "OMASHEETS_GRID_BENCHMARK ",
                "{{\"schema\":1,\"fixture\":\"synthetic-1000000x64\",",
                "\"rows\":{},\"columns\":{},\"frames\":{},",
                "\"elapsed_seconds\":{:.6},\"p95_frame_ms\":{:.6},",
                "\"worst_frame_ms\":{:.6},\"visible_delegates\":{},",
                "\"cell_reads\":{},\"startup_to_report_ms\":{:.3}}}"
            ),
            self.row_count,
            self.column_count,
            frames,
            elapsed_seconds,
            p95_frame_ms,
            worst_frame_ms,
            visible_delegates,
            self.cell_reads.load(Ordering::Relaxed),
            self.created.elapsed().as_secs_f64() * 1_000.0,
        );
    }
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
