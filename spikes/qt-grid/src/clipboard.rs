pub const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_CELLS: usize = 1000;

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
