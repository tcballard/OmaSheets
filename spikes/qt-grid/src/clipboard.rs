pub const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_CELLS: usize = 1000;

/// Shift only A1 tokens, preserving source spelling elsewhere. Unlike compiling
/// to stable IDs, this retains each axis's `$` flag and needs no live names.
pub fn translate(rows: &mut [Vec<String>], row_delta: i32, column_delta: i32) {
    for row in rows {
        for value in row {
            if value.starts_with('=') {
                *value = translate_formula(value, row_delta, column_delta);
            }
        }
    }
}

fn translate_formula(source: &str, row_delta: i32, column_delta: i32) -> String {
    let mut result = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' || ch == '\'' {
            result.push(ch);
            while let Some(next) = chars.next() {
                result.push(next);
                if next == ch {
                    if chars.peek() == Some(&ch) { result.push(chars.next().unwrap()); }
                    else { break; }
                }
            }
        } else if ch == '[' {
            // Table columns (including nested selectors) and external book names
            // are not A1 references, even when their labels look like cells.
            result.push(ch);
            let mut depth = 1;
            for next in chars.by_ref() {
                result.push(next);
                if next == '[' { depth += 1; }
                if next == ']' { depth -= 1; }
                if depth == 0 { break; }
            }
        } else if identifier_char(ch) {
            let mut token = String::from(ch);
            while chars.peek().is_some_and(|ch| identifier_char(*ch)) {
                token.push(chars.next().unwrap());
            }
            // Sheet names and function names such as LOG10 are not cells.
            let next = chars.clone().find(|ch| !ch.is_whitespace());
            if matches!(next, Some('!' | '(' | '[')) {
                result.push_str(&token);
            } else {
                result.push_str(&shift_reference(&token, row_delta, column_delta));
            }
        } else { result.push(ch); }
    }
    result
}

fn identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '.' | '$' | '\\')
}

fn shift_reference(token: &str, row_delta: i32, column_delta: i32) -> String {
    let bytes = token.as_bytes();
    let mut i = 0;
    let column_absolute = bytes.first() == Some(&b'$');
    if column_absolute { i += 1; }
    let letters = i;
    let mut column = 0i64;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        if i - letters >= 3 { return token.into(); }
        column = column * 26 + i64::from(bytes[i].to_ascii_uppercase() - b'A' + 1);
        i += 1;
    }
    if i == letters || column > 16384 { return token.into(); }
    let row_absolute = bytes.get(i) == Some(&b'$');
    if row_absolute { i += 1; }
    let digits = &token[i..];
    if digits.is_empty() || !digits.bytes().all(|ch| ch.is_ascii_digit()) { return token.into(); }
    let Ok(row) = digits.parse::<i64>() else { return token.into(); };
    if !(1..=1048576).contains(&row) { return token.into(); }
    let column = column + if column_absolute { 0 } else { i64::from(column_delta) };
    let row = row + if row_absolute { 0 } else { i64::from(row_delta) };
    if !(1..=16384).contains(&column) || !(1..=1048576).contains(&row) {
        return "#REF!".into();
    }
    let mut letters = Vec::new();
    let mut number = column;
    while number > 0 {
        number -= 1;
        letters.push(b'A' + (number % 26) as u8);
        number /= 26;
    }
    letters.reverse();
    format!("{}{}{}{row}", if column_absolute { "$" } else { "" },
        String::from_utf8(letters).unwrap(), if row_absolute { "$" } else { "" })
}

pub fn parse(text: &str) -> Result<Vec<Vec<String>>, String> {
    if text.len() > MAX_BYTES { return Err("clipboard exceeds 1 MiB".into()); }
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut closed = false;
    let mut count = 0;
    let mut ended_row = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        ended_row = false;
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') { chars.next(); field.push('"'); }
                else { quoted = false; closed = true; }
            } else { field.push(ch); }
            continue;
        }
        if ch == '\t' || ch == '\n' || ch == '\r' {
            row.push(std::mem::take(&mut field));
            count += 1;
            if count > MAX_CELLS { return Err("clipboard exceeds 1,000 cells".into()); }
            closed = false;
            if ch != '\t' {
                if ch == '\r' && chars.peek() == Some(&'\n') { chars.next(); }
                rows.push(std::mem::take(&mut row));
                ended_row = true;
            }
        } else if closed {
            return Err("unexpected text after a quoted clipboard field".into());
        } else if ch == '"' && field.is_empty() {
            quoted = true;
        } else { field.push(ch); }
    }
    if quoted { return Err("clipboard contains an unfinished quoted field".into()); }
    if !ended_row {
        row.push(field);
        count += 1;
        rows.push(row);
    }
    if count > MAX_CELLS { return Err("clipboard exceeds 1,000 cells".into()); }
    let width = rows.first().map_or(0, Vec::len);
    if rows.iter().any(|row| row.len() != width) {
        return Err("clipboard rows must have the same number of columns".into());
    }
    Ok(rows)
}

pub fn encode(rows: &[Vec<String>]) -> Result<String, String> {
    if rows.iter().map(Vec::len).sum::<usize>() > MAX_CELLS {
        return Err("copy is limited to 1,000 cells".into());
    }
    let text = rows.iter().map(|row| row.iter().map(|value| {
        if value.is_empty() || value.contains(['\t', '\r', '\n', '"']) {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else { value.clone() }
    }).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n");
    if text.len() > MAX_BYTES { return Err("copy exceeds 1 MiB".into()); }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_formulas_shift_relative_axes_only() {
        assert_eq!(translate_formula("=A1+$B2+C$3+$D$4+SUM(A1:B2)", 2, 3),
            "=D3+$B4+F$3+$D$4+SUM(D3:E4)");
        assert_eq!(translate_formula("='Bob''s A1'!B2+A1!C3+LOG10(A1)+1E10+Tax.A1+名A1", 1, 1),
            "='Bob''s A1'!C3+A1!D4+LOG10(B2)+1E10+Tax.A1+名A1");
        assert_eq!(translate_formula("=IF(A1=\"A1\",\"say \"\"B2\"\"\",T[[#All],[A1]])+A1[B2]", 1, 1),
            "=IF(B2=\"A1\",\"say \"\"B2\"\"\",T[[#All],[A1]])+A1[B2]");
        assert_eq!(translate_formula("=A1+$A1+A$1+$A$1", -1, -1), "=#REF!+#REF!+#REF!+$A$1");
        assert_eq!(translate_formula("=XFD1048576+XFE1+A1048577", 1, 1), "=#REF!+XFE1+A1048577");
        assert_eq!(translate_formula("=B2:C3", -1, -1), "=A1:B2");
        let mut rows = vec![vec!["=A1".into(), "'=A1".into(), "A1".into()], vec!["=A2".into()]];
        translate(&mut rows, 1, 2);
        assert_eq!(rows, vec![vec!["=C2", "'=A1", "A1"], vec!["=C3"]]);
    }

    #[test]
    fn quoted_clipboards_round_trip() {
        let rows = vec![vec!["a\tb".into(), "line\nnext".into(), "say \"hi\"".into()],
            vec!["'001".into(), "=A1+1".into(), "".into()]];
        assert_eq!(parse(&encode(&rows).unwrap()).unwrap(), rows);
        let trailing_blank = vec![vec!["a".into()], vec![String::new()]];
        assert_eq!(parse(&encode(&trailing_blank).unwrap()).unwrap(), trailing_blank);
        assert_eq!(parse("a\tb\r\nc\td\r\n").unwrap().len(), 2);
        assert_eq!(parse("a\t").unwrap()[0], vec!["a", ""]);
        assert!(parse("a\tb\nc").is_err());
        assert!(parse("\"unfinished").is_err());
        assert!(parse("\"a\"junk").is_err());
        assert!(parse(&"x\t".repeat(MAX_CELLS)).is_err());
        assert!(parse(&"x".repeat(MAX_BYTES + 1)).is_err());
    }
}
