//! The data tables the solver reads by file name (`Function1s::Table` with a `file`): the sun's
//! positions, the sky's longwave irradiation and the outdoor air. Only their first and last
//! times are kept, for the case check to compare with the run's.

use umcf_core::CaseFs;

/// The tables the solver may read, case-relative (each also as `.gz`, which OpenFOAM reads in
/// its place): `viewFactorSky` (`constant/LW`, else `0/air/Tambient` and `0/air/cloudCover`),
/// `directAndDiffuse` and `simplifiedVegetation` (`constant/sunPosVector`), the solid
/// temperature conditions (`0/air/Tambient`, `0/air/wambient`, `0/air/rainTemp`).
pub const TABLES: [&str; 6] =
    ["constant/LW", "constant/sunPosVector", "0/air/Tambient", "0/air/wambient", "0/air/cloudCover", "0/air/rainTemp"];

/// What a table file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Case-relative, as found (`0/air/Tambient` or `0/air/Tambient.gz`).
    pub path: String,
    /// Its first and last times and how many rows, or what is wrong with it.
    pub span: Result<Span, Problem>,
}

/// What is wrong with a table.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// OpenFOAM's table reader stops on it (else it is only not read here).
    pub fatal: bool,
    pub message: String,
}

impl Problem {
    fn unread(message: impl Into<String>) -> Problem {
        Problem { fatal: false, message: message.into() }
    }

    fn fatal(message: impl Into<String>) -> Problem {
        Problem { fatal: true, message: message.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub first: f64,
    pub last: f64,
    pub rows: usize,
}

/// The tables of [`TABLES`] that are there, within `max_bytes` each (decompressed).
pub fn read(fs: &dyn CaseFs, max_bytes: u64) -> Vec<Table> {
    let mut out = Vec::new();
    for name in TABLES {
        for path in [name.to_string(), format!("{name}.gz")] {
            let span = match fs.read(&path, max_bytes) {
                Ok(None) => continue,
                Ok(Some(bytes)) => {
                    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
                        match umcf_foam_field::gunzip(&bytes, max_bytes) {
                            Ok(b) => b,
                            Err(e) => {
                                out.push(Table { path, span: Err(Problem::unread(e.to_string())) });
                                break;
                            }
                        }
                    } else {
                        bytes
                    };
                    span(&bytes)
                }
                Err(e) => Err(Problem::unread(e.to_string())),
            };
            out.push(Table { path, span });
            break;
        }
    }
    out
}

/// The first and last times of a table as OpenFOAM's table reader reads it (`is >> data`, both
/// versions): `( (t value) … )`, values a number or a `( … )` vector, comments skipped, the times
/// strictly increasing. What it stops on (a `FoamFile` header, times out of order or repeated, a
/// row without a value) is fatal; what isn't read here otherwise is not.
pub fn span(bytes: &[u8]) -> Result<Span, Problem> {
    let text = String::from_utf8_lossy(bytes);
    let tokens = tokens(&text);
    let mut it = tokens.iter().copied().peekable();
    if it.peek().is_some_and(|t| *t == "FoamFile") {
        return Err(Problem::fatal("it has a FoamFile header, which OpenFOAM's table reader stops on"));
    }
    // An optional count, which must be the rows', then the list.
    let count: Option<usize> = match it.peek() {
        Some(t) if t.chars().all(|c| c.is_ascii_digit()) => {
            let n = it.next().and_then(|t| t.parse().ok());
            Some(n.ok_or_else(|| Problem::fatal("its count is too big for a list"))?)
        }
        _ => None,
    };
    if it.next() != Some("(") {
        return Err(Problem::unread("not a list of ( time value ) rows"));
    }
    let (mut first, mut last, mut rows) = (None, None, 0usize);
    loop {
        match it.next() {
            Some(")") => break,
            Some("(") => {
                let t: f64 = it
                    .next()
                    .and_then(|t| t.parse().ok())
                    .filter(|t: &f64| t.is_finite())
                    .ok_or_else(|| Problem::unread(format!("row {} has no time", rows + 1)))?;
                if last.is_some_and(|l| t <= l) {
                    return Err(Problem::fatal(format!(
                        "row {} has time {t}, not after the one before: OpenFOAM stops on a time out of order",
                        rows + 1
                    )));
                }
                // The rest of the row, a vector's parentheses included: at least a value.
                if it.peek() == Some(&")") {
                    return Err(Problem::fatal(format!("row {} has no value", rows + 1)));
                }
                let mut depth = 1usize;
                for t in it.by_ref() {
                    match t {
                        "(" => depth += 1,
                        ")" => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if depth != 0 {
                    return Err(Problem::unread("the list isn't closed"));
                }
                first.get_or_insert(t);
                last = Some(t);
                rows += 1;
            }
            _ => return Err(Problem::unread("not a list of ( time value ) rows")),
        }
    }
    if let Some(n) = count.filter(|n| *n != rows) {
        return Err(Problem::fatal(format!(
            "it says {n} rows and has {rows}: OpenFOAM stops on a list of another length"
        )));
    }
    match (first, last) {
        (Some(first), Some(last)) => Ok(Span { first, last, rows }),
        _ => Err(Problem::fatal("the table has no rows")),
    }
}

/// Words, numbers and parentheses and braces, comments (`//`, `/* */`) and `;` left out.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b.get(i).copied().unwrap_or(b' ');
        if c.is_ascii_whitespace() || c == b';' {
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b.get(i) != Some(&b'\n') {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b.get(i) == Some(&b'*') && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i += 2;
        } else if matches!(c, b'(' | b')' | b'{' | b'}') {
            out.push(text.get(i..i + 1).unwrap_or(""));
            i += 1;
        } else {
            let start = i;
            while i < b.len()
                && b.get(i).is_some_and(|c| !c.is_ascii_whitespace() && !matches!(c, b'(' | b')' | b'{' | b'}' | b';'))
            {
                i += 1;
            }
            out.push(text.get(start..i).unwrap_or(""));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_of_tables() {
        let s = span(b"(\n( 0 290 )\n( 3600 295 )\n( 7200 291 )\n)\n").unwrap();
        assert_eq!((s.first, s.last, s.rows), (0.0, 7200.0, 3));
        let v = span(b"// sun\n(\n( 0 ( 0.3 -0.3 0.8 ) )\n( 86400 ( 0.3 -0.3 0.8 ) )\n)\n").unwrap();
        assert_eq!((v.first, v.last, v.rows), (0.0, 86400.0, 2));
        let n = span(b"2\n( (10 1) (20 2e1) )\n").unwrap();
        assert_eq!((n.first, n.last), (10.0, 20.0));
        // What OpenFOAM stops on is fatal; what isn't read here, not.
        for fatal in [
            &b"FoamFile { version 2.0; }\n( (10 1) (20 2) )\n"[..],
            b"( (10 1) (5 2) )",
            b"( (10 1) (10 2) )",
            b"( (0) (1 2) )",
            b"3 ( (0 1) (1 2) )",
            b"99999999999999999999999 ( (0 1) )",
            b"( )",
        ] {
            assert!(span(fatal).is_err_and(|p| p.fatal), "{}", String::from_utf8_lossy(fatal));
        }
        for unread in [&b""[..], b"( (a 1) )", b"( (0 1)", b"x"] {
            assert!(span(unread).is_err_and(|p| !p.fatal), "{}", String::from_utf8_lossy(unread));
        }
    }
}
