use super::*;

const ROWS: usize = 50;
const COLUMNS: usize = 12;
const SHEETS: usize = 8;
const CELL_CHARACTERS: usize = 18;

fn expanded(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            if character.is_ascii() && !character.is_control() {
                character.to_string().chars().collect::<Vec<_>>()
            } else {
                format!("U+{:04X}", character as u32).chars().collect()
            }
        })
        .collect()
}

fn escaped(text: &str, maximum: usize) -> String {
    let mut text = expanded(text);
    if text.len() > maximum {
        text.truncate(maximum.saturating_sub(3));
        text.push_str(&"..."[..maximum.min(3)]);
    }
    text.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn pdf(pages: &[String]) -> Vec<u8> {
    let mut objects = vec![String::from("<< /Type /Catalog /Pages 2 0 R >>")];
    let kids = (0..pages.len())
        .map(|index| format!("{} 0 R", 5 + index * 2))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push(format!(
        "<< /Type /Pages /Count {} /Kids [{kids}] >>",
        pages.len()
    ));
    objects.push(String::from(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ));
    for (index, page) in pages.iter().enumerate() {
        objects.push(format!(
            "<< /Length {} >>\nstream\n{page}\nendstream",
            page.len()
        ));
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 842 595] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>", 4 + index * 2));
    }
    let mut bytes = b"%PDF-1.4\n%OmaSheets owned preview\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in offsets.iter().skip(1) {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    bytes
}

pub(super) fn write(workbook: &Workbook, output: &Path) -> Result<Value, KitError> {
    let mut pages = Vec::new();
    let mut truncated = workbook.sheets.len() > SHEETS;
    let mut text_truncated = false;
    let mut unicode_escaped = false;
    for sheet in workbook.sheets.iter().take(SHEETS) {
        truncated |= sheet.rows > ROWS || sheet.columns > COLUMNS;
        let mut content = format!(
            "BT /F1 14 Tf 24 571 Td (OmaSheets: {}) Tj ET\n",
            escaped(&sheet.summary.name, 90)
        );
        content.push_str("BT /F1 7 Tf 24 554 Td (Calculated values; top 50 rows x 12 columns. Styles, charts and print layout are not rendered.) Tj ET\n");
        content.push_str("BT /F1 7 Tf 24 18 Td (Non-ASCII text uses U+ escapes; cell text is limited to 18 characters. Read the workbook for full content.) Tj ET\n");
        for column in 0..sheet.columns.min(COLUMNS) {
            content.push_str(&format!(
                "BT /F1 8 Tf {} 539 Td ({}) Tj ET\n",
                48 + column * 64,
                column_name(column)
            ));
        }
        for row in 0..sheet.rows.min(ROWS) {
            let y = 526 - row * 10;
            content.push_str(&format!("BT /F1 6 Tf 24 {y} Td ({}) Tj ET\n", row + 1));
            for column in 0..sheet.columns.min(COLUMNS) {
                let text = sheet
                    .cells
                    .get(&(row, column))
                    .map_or_else(String::new, |cell| {
                        if cell.display.is_empty() {
                            plain(&cell.value)
                        } else {
                            cell.display.clone()
                        }
                    });
                text_truncated |= expanded(&text).len() > CELL_CHARACTERS;
                unicode_escaped |= !text.is_ascii();
                content.push_str(&format!(
                    "BT /F1 6 Tf {} {y} Td ({}) Tj ET\n",
                    48 + column * 64,
                    escaped(&text, CELL_CHARACTERS)
                ));
            }
        }
        pages.push(content);
    }
    if pages.is_empty() {
        pages.push(String::from(
            "BT /F1 14 Tf 24 571 Td (OmaSheets: no sheets) Tj ET",
        ));
    }
    let bytes = pdf(&pages);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(
        json!({"format": "pdf", "engine": {"name": "OmaSheets Kit"}, "presentation": "calculated_values",
        "viewport": {"rows": ROWS, "columns": COLUMNS, "max_sheets": SHEETS, "max_characters_per_cell": CELL_CHARACTERS},
        "pages": pages.len(), "truncated": truncated, "cell_text_truncated": text_truncated,
        "unicode_escaped": unicode_escaped,
        "warnings": ["Bounded calculated-value preview; styles, charts and print layout are not rendered",
            "Non-ASCII text is represented by U+ code point escapes; long cell text is cropped"]}),
    )
}
