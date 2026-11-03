//! The list scanner. It reads `nonuniform List<T> N (…)` data straight into numbers (ASCII or
//! binary) and replaces it in the text with a placeholder word, so that the rest of the file —
//! header, `dimensions`, `boundaryField` with its small entries — is small enough for the
//! lossless dictionary parser.

use crate::{Arch, Error, Kind, Limits, ListData};

/// The placeholder for list `k` in the scanned text.
pub(crate) fn placeholder(k: usize) -> String {
    format!("__umcfList{k}")
}

pub(crate) fn placeholder_index(word: &str) -> Option<usize> {
    word.strip_prefix("__umcfList")?.parse().ok()
}

pub(crate) struct Scanned {
    pub text: Vec<u8>,
    pub lists: Vec<ListData>,
}

fn is_ws(b: u8) -> bool {
    b.is_ascii_whitespace()
}

fn is_delim(b: u8) -> bool {
    is_ws(b) || matches!(b, b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';' | b'"')
}

/// Line number (1-based) of a byte offset, for errors.
pub(crate) fn line_of(bytes: &[u8], at: usize) -> usize {
    bytes.get(..at).unwrap_or(bytes).iter().filter(|b| **b == b'\n').count() + 1
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| from + p)
}

pub(crate) fn scan(bytes: &[u8], binary: bool, arch: Arch, limits: &Limits) -> Result<Scanned, Error> {
    let mut text = Vec::with_capacity(bytes.len().min(1 << 20));
    let mut lists = Vec::new();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let next = bytes.get(i + 1).copied();
        let end = match (b, next) {
            (b'/', Some(b'/')) => find(bytes, i, b"\n").unwrap_or(bytes.len()),
            (b'/', Some(b'*')) => find(bytes, i + 2, b"*/").map_or(bytes.len(), |p| p + 2),
            (b'#', Some(b'{')) => find(bytes, i + 2, b"#}").map_or(bytes.len(), |p| p + 2),
            (b'"', _) => string_end(bytes, i),
            // `List<T> N (…)` without `nonuniform`: zone files (`cellLabels List<label> …`).
            _ if starts_typed_list(bytes, i) => {
                let (list, end) = read_list(bytes, i, binary, arch, limits)?;
                text.extend_from_slice(placeholder(lists.len()).as_bytes());
                lists.push(list);
                i = end;
                continue;
            }
            _ if starts_keyword(bytes, i, b"nonuniform") => {
                text.extend_from_slice(b"nonuniform ");
                let (list, end) = read_list(bytes, i + b"nonuniform".len(), binary, arch, limits)?;
                text.extend_from_slice(placeholder(lists.len()).as_bytes());
                lists.push(list);
                i = end;
                continue;
            }
            _ => i + 1,
        };
        text.extend_from_slice(bytes.get(i..end).unwrap_or(&[]));
        i = end.max(i + 1);
    }
    Ok(Scanned { text, lists })
}

fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

fn starts_keyword(bytes: &[u8], i: usize, word: &[u8]) -> bool {
    let before_ok = i == 0 || bytes.get(i - 1).is_some_and(|b| is_delim(*b));
    let after_ok = bytes.get(i + word.len()).is_some_and(|b| is_ws(*b));
    before_ok && after_ok && bytes.get(i..i + word.len()) == Some(word)
}

fn starts_typed_list(bytes: &[u8], i: usize) -> bool {
    (i == 0 || bytes.get(i - 1).is_some_and(|b| is_delim(*b))) && bytes.get(i..i + 5) == Some(b"List<")
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(|b| is_ws(*b)) {
        i += 1;
    }
    i
}

fn bad(bytes: &[u8], at: usize, msg: impl Into<String>) -> Error {
    Error::BadList { msg: msg.into(), line: line_of(bytes, at) }
}

/// After `nonuniform`: `List<T> N (…)`, `List<T> N{v}`, or the untyped `N (…)`.
fn read_list(
    bytes: &[u8],
    mut i: usize,
    binary: bool,
    arch: Arch,
    limits: &Limits,
) -> Result<(ListData, usize), Error> {
    i = skip_ws(bytes, i);
    let mut kind = None;
    if bytes.get(i..i + 5) == Some(b"List<") {
        let close = find(bytes, i, b">").ok_or_else(|| bad(bytes, i, "unclosed `List<`"))?;
        let name = std::str::from_utf8(bytes.get(i + 5..close).unwrap_or(&[])).unwrap_or("");
        kind = Some(Kind::from_name(name).ok_or_else(|| Error::Unsupported(format!("List<{name}>")))?);
        i = skip_ws(bytes, close + 1);
    }
    let digits_end = (i..).find(|j| !bytes.get(*j).is_some_and(u8::is_ascii_digit)).unwrap_or(i);
    if digits_end == i {
        return Err(bad(bytes, i, "expected the list length"));
    }
    let n: usize = std::str::from_utf8(bytes.get(i..digits_end).unwrap_or(&[]))
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad(bytes, i, "list length out of range"))?;
    i = skip_ws(bytes, digits_end);
    match bytes.get(i) {
        Some(b'{') => {
            // `N{value}`: every element the same.
            let (elem, kind, end) = read_ascii_elements(bytes, i + 1, 1, kind, limits)?;
            let end = skip_ws(bytes, end);
            if bytes.get(end) != Some(&b'}') {
                return Err(bad(bytes, end, "expected `}` after the uniform list value"));
            }
            check_count(n, kind.components(), limits)?;
            Ok((ListData::repeated(kind, n, elem), end + 1))
        }
        Some(b'(') => {
            let kind_or_scalar = kind.unwrap_or(Kind::Scalar);
            if n == 0 {
                let end = skip_ws(bytes, i + 1);
                if bytes.get(end) != Some(&b')') {
                    return Err(bad(bytes, end, "expected `)` closing an empty list"));
                }
                return Ok((ListData::flat(kind_or_scalar, 0, Vec::new()), end + 1));
            }
            check_count(n, kind_or_scalar.components(), limits)?;
            if binary {
                let kind = kind.ok_or_else(|| Error::Unsupported("binary list without a List<T> type".into()))?;
                read_binary(bytes, i + 1, n, kind, arch)
            } else {
                let (values, kind, end) = read_ascii_elements(bytes, i + 1, n, kind, limits)?;
                let end = skip_ws(bytes, end);
                if bytes.get(end) != Some(&b')') {
                    return Err(bad(bytes, end, format!("expected `)` after {n} elements")));
                }
                Ok((ListData::flat(kind, n, values), end + 1))
            }
        }
        _ => Err(bad(bytes, i, "expected `(` or `{` after the list length")),
    }
}

fn check_count(n: usize, comps: usize, limits: &Limits) -> Result<(), Error> {
    match n.checked_mul(comps) {
        Some(v) if v <= limits.max_values => Ok(()),
        _ => Err(Error::TooLarge { what: "list", limit: limits.max_values as u64 }),
    }
}

fn ends_number(b: Option<&u8>) -> bool {
    b.is_none_or(|b| is_ws(*b) || matches!(b, b'(' | b')' | b'}'))
}

fn number(bytes: &[u8], i: usize) -> Result<(f64, usize), Error> {
    let i = skip_ws(bytes, i);
    let rest = bytes.get(i..).unwrap_or(&[]);
    // Fast path: parse and measure in one go; the number must end at a delimiter.
    if let Ok((v, n)) = fast_float2::parse_partial::<f64, _>(rest)
        && n > 0
        && ends_number(rest.get(n))
    {
        return Ok((v, i + n));
    }
    // Slow path: words fast-float2 doesn't take (`nan`, `-inf`), and the error message.
    let end = (i..).find(|j| ends_number(bytes.get(*j))).unwrap_or(i);
    let word = bytes.get(i..end).unwrap_or(&[]);
    match std::str::from_utf8(word).ok().and_then(|s| s.parse::<f64>().ok()) {
        Some(v) => Ok((v, end)),
        None => Err(bad(bytes, i, format!("expected a number, found `{}`", String::from_utf8_lossy(word)))),
    }
}

/// `n` ASCII elements: numbers for scalars and labels, `(a b c)` for vectors and tensors. An
/// untyped list takes its component count from the first element.
fn read_ascii_elements(
    bytes: &[u8],
    mut i: usize,
    n: usize,
    kind: Option<Kind>,
    limits: &Limits,
) -> Result<(Vec<f64>, Kind, usize), Error> {
    let first = skip_ws(bytes, i);
    let kind = match kind {
        Some(k) => k,
        None if bytes.get(first) == Some(&b'(') => {
            let close = find(bytes, first, b")").ok_or_else(|| bad(bytes, first, "unclosed element"))?;
            let inner = bytes.get(first + 1..close).unwrap_or(&[]);
            let comps = inner.split(|b| is_ws(*b)).filter(|w| !w.is_empty()).count();
            // One component in parentheses is how a sphericalTensor is written.
            if comps == 1 {
                Kind::SphericalTensor
            } else {
                Kind::from_components(comps).ok_or_else(|| bad(bytes, first, format!("{comps}-component element")))?
            }
        }
        None => Kind::Scalar,
    };
    let comps = kind.components();
    // An ASCII value takes at least two bytes: never reserve more than the input could hold.
    let room = bytes.len().saturating_sub(i) / 2 + 1;
    let mut values = Vec::with_capacity(n.saturating_mul(comps).min(room).min(limits.max_values));
    for _ in 0..n {
        // Scalars and labels are bare numbers; every other type, sphericalTensor's one
        // component included, is written in parentheses: `(1)`.
        if matches!(kind, Kind::Scalar | Kind::Label) {
            let (v, end) = number(bytes, i)?;
            values.push(v);
            i = end;
        } else {
            i = skip_ws(bytes, i);
            if bytes.get(i) != Some(&b'(') {
                return Err(bad(bytes, i, format!("expected `(` starting a {}", kind.name())));
            }
            i += 1;
            for _ in 0..comps {
                let (v, end) = number(bytes, i)?;
                values.push(v);
                i = end;
            }
            i = skip_ws(bytes, i);
            if bytes.get(i) != Some(&b')') {
                return Err(bad(bytes, i, format!("expected `)` closing a {}", kind.name())));
            }
            i += 1;
        }
    }
    Ok((values, kind, i))
}

fn read_binary(bytes: &[u8], start: usize, n: usize, kind: Kind, arch: Arch) -> Result<(ListData, usize), Error> {
    let width = if kind == Kind::Label { arch.label_bytes } else { arch.scalar_bytes };
    let comps = kind.components();
    let size = n
        .checked_mul(comps)
        .and_then(|v| v.checked_mul(width))
        .ok_or(Error::TooLarge { what: "list", limit: usize::MAX as u64 })?;
    let end = start.checked_add(size).ok_or(Error::Truncated { line: line_of(bytes, start) })?;
    let data = bytes.get(start..end).ok_or(Error::Truncated { line: line_of(bytes, start) })?;
    if bytes.get(end) != Some(&b')') {
        return Err(bad(bytes, end, format!("expected `)` after {n} binary elements")));
    }
    let mut values = Vec::with_capacity(n * comps);
    for chunk in data.chunks_exact(width) {
        let v = match (kind == Kind::Label, width) {
            (false, 8) => f64::from_bits(u64_from(chunk, arch.big_endian)),
            (false, _) => f64::from(f32::from_bits(u32_from(chunk, arch.big_endian))),
            (true, 8) => u64_from(chunk, arch.big_endian) as i64 as f64,
            (true, _) => f64::from(u32_from(chunk, arch.big_endian) as i32),
        };
        values.push(v);
    }
    Ok((ListData::flat(kind, n, values), end + 1))
}

fn u64_from(c: &[u8], big: bool) -> u64 {
    let mut a = [0u8; 8];
    for (d, s) in a.iter_mut().zip(c) {
        *d = *s;
    }
    if big { u64::from_be_bytes(a) } else { u64::from_le_bytes(a) }
}

fn u32_from(c: &[u8], big: bool) -> u32 {
    let mut a = [0u8; 4];
    for (d, s) in a.iter_mut().zip(c) {
        *d = *s;
    }
    if big { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) }
}
