//! Present stable bindings at their current A1 addresses without changing events.
use crate::{CellRef, CompiledFormula, Document, parse_a1};

struct Span<'a> {
    start: usize,
    end: usize,
    first: &'a str,
    last: Option<&'a str>,
    qualified: bool,
}
fn space(source: &str, mut at: usize) -> usize {
    while source
        .as_bytes()
        .get(at)
        .is_some_and(u8::is_ascii_whitespace)
    {
        at += 1;
    }
    at
}
fn token(source: &str, mut at: usize) -> usize {
    while source
        .as_bytes()
        .get(at)
        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'$' | b'_' | b'.'))
    {
        at += 1;
    }
    at
}
fn quoted(source: &str, mut at: usize, quote: u8) -> usize {
    at += 1;
    while let Some(byte) = source.as_bytes().get(at) {
        at += 1;
        if *byte == quote {
            if source.as_bytes().get(at) == Some(&quote) {
                at += 1;
            } else {
                break;
            }
        }
    }
    at
}
fn spans(source: &str) -> Option<Vec<Span<'_>>> {
    let mut output = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let start = at;
        let byte = source.as_bytes()[at];
        if byte == b'"' {
            at = quoted(source, at, b'"');
            continue;
        }
        if byte == b'[' {
            return None;
        }
        let mut qualified = false;
        let mut first_start = at;
        if byte == b'\'' {
            at = space(source, quoted(source, at, b'\''));
            if source.as_bytes().get(at) != Some(&b'!') {
                return None;
            }
            qualified = true;
            first_start = space(source, at + 1);
            at = token(source, first_start);
        } else if byte.is_ascii_alphanumeric() || matches!(byte, b'$' | b'_' | b'.') {
            at = token(source, at);
            let next = space(source, at);
            if source.as_bytes().get(next) == Some(&b'!') {
                qualified = true;
                first_start = space(source, next + 1);
                at = token(source, first_start);
            } else if source.as_bytes().get(next) == Some(&b'(') {
                continue;
            }
        } else {
            at += source[at..].chars().next()?.len_utf8();
            continue;
        }
        let first = &source[first_start..at];
        if parse_a1(first).is_none() {
            if at == start {
                at += 1;
            }
            continue;
        }
        let mut last = None;
        let next = space(source, at);
        if source.as_bytes().get(next) == Some(&b':') {
            let second_start = space(source, next + 1);
            at = token(source, second_start);
            let second = &source[second_start..at];
            parse_a1(second)?;
            last = Some(second);
        }
        output.push(Span {
            start,
            end: at,
            first,
            last,
            qualified,
        });
    }
    Some(output)
}
fn with_markers(address: &str, original: &str) -> String {
    let letters = address.bytes().take_while(u8::is_ascii_alphabetic).count();
    format!(
        "{}{}{}{}",
        if original.starts_with('$') { "$" } else { "" },
        &address[..letters],
        if original.trim_start_matches('$').contains('$') {
            "$"
        } else {
            ""
        },
        &address[letters..]
    )
}

impl Document {
    /// Exact editable/exportable source, or `None` when a stable range no longer
    /// has a faithful rectangular spelling. The original source remains in history.
    pub fn project_formula(&self, cell: CellRef, formula: &CompiledFormula) -> Option<String> {
        if formula.current_table.is_some() || !formula.table_bindings.is_empty() {
            return None;
        }
        let references = formula.references();
        if self
            .compile_formula(cell.sheet, &formula.source)
            .is_ok_and(|current| current.references() == references)
        {
            return Some(formula.source.clone());
        }
        let spans = spans(&formula.source)?;
        let mut output = String::new();
        let mut offset = 0;
        let mut reference = 0;
        for span in spans {
            let first = *references.get(reference)?;
            let count = if let Some(last) = span.last {
                let (r, c) = parse_a1(span.first)?;
                let (last_r, last_c) = parse_a1(last)?;
                if last_r < r || last_c < c {
                    return None;
                }
                (last_r - r + 1).checked_mul(last_c - c + 1)?
            } else {
                1
            };
            let last = *references.get(reference.checked_add(count)?.checked_sub(1)?)?;
            if first.sheet != last.sheet {
                return None;
            }
            output.push_str(&formula.source[offset..span.start]);
            if span.qualified || first.sheet != cell.sheet {
                output.push('\'');
                output.push_str(&self.sheet_name(first.sheet)?.replace('\'', "''"));
                output.push_str("'!");
            }
            output.push_str(&with_markers(&self.project_a1(first)?, span.first));
            if let Some(original) = span.last {
                output.push(':');
                output.push_str(&with_markers(&self.project_a1(last)?, original));
            }
            offset = span.end;
            reference += count;
        }
        if reference != references.len() {
            return None;
        }
        output.push_str(&formula.source[offset..]);
        self.compile_formula(cell.sheet, &output)
            .ok()
            .filter(|current| current.references() == references)
            .map(|_| output)
    }
}
