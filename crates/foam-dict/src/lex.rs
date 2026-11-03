//! Tokenizer. Every byte of the input ends up in exactly one token (trivia included), so
//! concatenating the token texts gives the input back.
//!
//! Word rules follow OpenFOAM's `ISstream`: a word that starts with a letter may contain
//! balanced parentheses (`div(phi,U)`, `grad(U)`), and a word that starts like a number stops at
//! `(` (`3(1 2 3)` is the word `3` then a list).

use crate::{Error, Limits};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Whitespace and comments.
    Trivia,
    /// A word, number, `$macro`, `${macro}`, `#directive`, …
    Word,
    /// A double-quoted string, quotes included.
    Str,
    /// A `#{ … #}` code block, delimiters included.
    Verbatim,
    /// `{ } ( ) [ ] ;`
    Punct(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
}

fn is_punct(b: u8) -> bool {
    matches!(b, b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';')
}

fn comment_starts(bytes: &[u8], i: usize) -> bool {
    bytes.get(i) == Some(&b'/') && matches!(bytes.get(i + 1), Some(b'/' | b'*'))
}

/// Line and column (both 1-based) of a byte offset, for error messages.
pub fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let before = src.get(..offset).unwrap_or(src);
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let col =
        before.rfind('\n').map_or(before.chars().count(), |nl| before.get(nl + 1..).unwrap_or("").chars().count()) + 1;
    (line, col)
}

pub fn tokenize(src: &str, limits: &Limits) -> Result<Vec<Token>, Error> {
    if src.len() > limits.max_bytes {
        return Err(Error::TooLarge { what: "file", limit: limits.max_bytes });
    }
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let start = i;
        let kind = if b.is_ascii_whitespace() || comment_starts(bytes, i) {
            i = skip_trivia(src, i)?;
            Kind::Trivia
        } else if is_punct(b) {
            i += 1;
            Kind::Punct(b)
        } else if b == b'"' {
            i = string_end(src, i)?;
            Kind::Str
        } else if b == b'#' && bytes.get(i + 1) == Some(&b'{') {
            i = verbatim_end(src, i)?;
            Kind::Verbatim
        } else {
            i = word_end(bytes, i);
            Kind::Word
        };
        out.push(Token { kind, start, end: i });
        if out.len() > limits.max_tokens {
            return Err(Error::TooLarge { what: "token count", limit: limits.max_tokens });
        }
    }
    Ok(out)
}

fn skip_trivia(src: &str, mut i: usize) -> Result<usize, Error> {
    let bytes = src.as_bytes();
    loop {
        match bytes.get(i) {
            Some(b) if b.is_ascii_whitespace() => i += 1,
            Some(b'/') if bytes.get(i + 1) == Some(&b'/') => {
                i = src.get(i..).and_then(|s| s.find('\n')).map_or(src.len(), |n| i + n);
            }
            Some(b'/') if bytes.get(i + 1) == Some(&b'*') => {
                let close = src.get(i + 2..).and_then(|s| s.find("*/"));
                match close {
                    Some(n) => i = i + 2 + n + 2,
                    None => return Err(Error::Unterminated { what: "comment", at: line_col(src, i) }),
                }
            }
            _ => return Ok(i),
        }
    }
}

fn string_end(src: &str, start: usize) -> Result<usize, Error> {
    let bytes = src.as_bytes();
    let mut i = start + 1;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err(Error::Unterminated { what: "string", at: line_col(src, start) })
}

fn verbatim_end(src: &str, start: usize) -> Result<usize, Error> {
    match src.get(start + 2..).and_then(|s| s.find("#}")) {
        Some(n) => Ok(start + 2 + n + 2),
        None => Err(Error::Unterminated { what: "#{ … #} code block", at: line_col(src, start) }),
    }
}

fn word_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    // `${name}` and `$:{…}`-style macros carry their own braces.
    if bytes.get(i) == Some(&b'$') && bytes.get(i + 1) == Some(&b'{') {
        let mut depth = 0usize;
        while let Some(&b) = bytes.get(i) {
            i += 1;
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let numeric = matches!(bytes.get(start), Some(b'0'..=b'9' | b'-' | b'+' | b'.'));
    let mut depth = 0usize;
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_whitespace() || b == b'"' || comment_starts(bytes, i) {
            break;
        }
        match b {
            b'(' if !numeric => depth += 1,
            b')' if depth > 0 => depth -= 1,
            b'(' | b')' | b'{' | b'}' | b'[' | b']' | b';' => break,
            _ => {}
        }
        i += 1;
    }
    // Never return an empty token: a lone stray character becomes a one-character word (kept on a
    // UTF-8 boundary).
    if i == start {
        i = start + 1;
        while bytes.get(i).is_some_and(|b| b & 0xC0 == 0x80) {
            i += 1;
        }
    }
    i.min(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(Kind, &str)> {
        tokenize(src, &Limits::default())
            .unwrap()
            .into_iter()
            .filter(|t| t.kind != Kind::Trivia)
            .map(|t| (t.kind, &src[t.start..t.end]))
            .collect()
    }

    #[test]
    fn words_with_parentheses() {
        assert_eq!(
            kinds("div(phi,U) bounded Gauss linearUpwind grad(U);"),
            vec![
                (Kind::Word, "div(phi,U)"),
                (Kind::Word, "bounded"),
                (Kind::Word, "Gauss"),
                (Kind::Word, "linearUpwind"),
                (Kind::Word, "grad(U)"),
                (Kind::Punct(b';'), ";"),
            ]
        );
        assert_eq!(kinds("div(((rho*nuEff)*dev2(T(grad(U)))))")[0].1, "div(((rho*nuEff)*dev2(T(grad(U)))))");
    }

    #[test]
    fn numbers_stop_at_parenthesis() {
        let k = kinds("3(1 -2.5e-3 +4)");
        assert_eq!(k[0], (Kind::Word, "3"));
        assert_eq!(k[1], (Kind::Punct(b'('), "("));
        assert_eq!(k[3], (Kind::Word, "-2.5e-3"));
    }

    #[test]
    fn macros_strings_code() {
        let k = kinds(r#"a ${x.y}; b "q\"x"; c #{ int i; } #};"#);
        assert_eq!(k[1], (Kind::Word, "${x.y}"));
        assert_eq!(k[4], (Kind::Str, r#""q\"x""#));
        assert_eq!(k[7], (Kind::Verbatim, "#{ int i; } #}"));
    }

    #[test]
    fn comments_are_trivia() {
        let src = "a 1;// c\n/* x\n y */b 2;";
        assert_eq!(kinds(src).len(), 6);
        let all = tokenize(src, &Limits::default()).unwrap();
        let joined: String = all.iter().map(|t| &src[t.start..t.end]).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn unterminated_reports_position() {
        let e = tokenize("a 1;\n  b \"oops", &Limits::default()).unwrap_err();
        assert!(matches!(e, Error::Unterminated { what: "string", at: (2, 5) }), "{e:?}");
        assert!(tokenize("/* x", &Limits::default()).is_err());
        assert!(tokenize("#{ x", &Limits::default()).is_err());
    }
}
