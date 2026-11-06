//! ASCII list files read as a stream: the file (compressed or not) is decompressed and parsed as
//! it comes, in chunks, so neither its compressed bytes nor its text are ever held whole. Binary
//! files, and lists a stream doesn't take, are left to the whole-file readers (`Ok(None)`). A
//! file the stream finds malformed (`Error::Bad`, `Error::Truncated`) is read again whole by the
//! caller, whose reader makes the error and names its line.

use std::io::Read;

use umcf_core::CaseFs;
use umcf_foam_field::{Format, Header};

use crate::lists::Reader;
use crate::{Error, FaceTail, Limits, Result};

/// How much is read from the file at once.
const CHUNK: usize = 1 << 20;
/// How far into the file the header and the list's `N (` may be.
const PREAMBLE: usize = 4 << 20;
/// How long one element of a list may be, as text: a stream that runs this far without one
/// ending is not a list this reader takes.
const ELEMENT: usize = 16 << 20;

/// A list's start: the file's header, the bytes read so far, the index just after the list's
/// `(`, and its count.
type ListStart = (Header, Vec<u8>, usize, usize);

/// A mesh file as a stream of its (decompressed) bytes.
pub(crate) struct Stream {
    pub path: String,
    src: Box<dyn Read + Send>,
    compressed: bool,
    chunk: Vec<u8>,
    /// Every byte that came out, against the limit.
    seen: u64,
    max: u64,
}

impl Stream {
    /// `dir/name`, else `dir/name.gz`, as `read_file` picks them; `None` if neither exists. A
    /// file that starts as gzip does is decompressed, whatever its name.
    pub fn open(fs: &dyn CaseFs, dir: &str, name: &str, limits: &Limits) -> Result<Option<Stream>> {
        for path in [format!("{dir}/{name}"), format!("{dir}/{name}.gz")] {
            let Some(mut r) = fs.reader(&path, limits.max_bytes)? else { continue };
            let mut magic = Vec::with_capacity(2);
            (&mut r).take(2).read_to_end(&mut magic).map_err(|e| io(&path, e))?;
            let compressed = magic == [0x1f, 0x8b];
            let head = std::io::Cursor::new(magic).chain(r);
            let src: Box<dyn Read + Send> =
                if compressed { Box::new(flate2::read::MultiGzDecoder::new(head)) } else { Box::new(head) };
            return Ok(Some(Stream { path, src, compressed, chunk: vec![0; CHUNK], seen: 0, max: limits.max_bytes }));
        }
        Ok(None)
    }

    /// The next chunk appended to `buf`; `false` at the end of the file.
    pub fn fill(&mut self, buf: &mut Vec<u8>) -> Result<bool> {
        let n = loop {
            match self.src.read(&mut self.chunk) {
                Ok(n) => break n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if self.compressed => {
                    return Err(Error::Field(umcf_foam_field::Error::Gzip(format!("{}: {e}", self.path))));
                }
                Err(e) => return Err(io(&self.path, e)),
            }
        };
        self.seen = self.seen.saturating_add(n as u64);
        if self.seen > self.max {
            let what = if self.compressed { "decompressed file" } else { "file" };
            return Err(Error::Field(umcf_foam_field::Error::TooLarge { what, limit: self.max }));
        }
        buf.try_reserve(n).map_err(|_| Error::TooLarge { what: "file (out of memory)", limit: buf.len() + n })?;
        buf.extend_from_slice(self.chunk.get(..n).unwrap_or_default());
        Ok(n > 0)
    }

    /// The header, and the list's `N (`: the header, the buffer read so far, the index just
    /// after the `(`, and `N`. `None` when the file is binary, its class isn't one of `classes`
    /// (a file without a class is taken), or the list doesn't start within the first few
    /// megabytes.
    fn list(&mut self, classes: &[&str], limits: &Limits) -> Result<Option<ListStart>> {
        let mut buf = Vec::new();
        let (header, end) = loop {
            let more = self.fill(&mut buf)?;
            if let Ok((h, Some(end))) = umcf_foam_field::read_header_end(&buf) {
                break (h, end);
            }
            if !more || buf.len() > PREAMBLE {
                return Ok(None);
            }
        };
        let class_ok = header.class.as_deref().is_none_or(|c| classes.contains(&c));
        if header.format != Format::Ascii || !class_ok {
            return Ok(None);
        }
        loop {
            match list_start(buf.get(end..).unwrap_or_default(), limits)? {
                Start::Found(n, at) => return Ok(Some((header, buf, end + at, n))),
                Start::NotHere => return Ok(None),
                Start::NeedMore => {
                    if buf.len() > PREAMBLE || !self.fill(&mut buf)? {
                        return Ok(None);
                    }
                }
            }
        }
    }
}

/// The stream found the file malformed. The caller reads it again whole, whose reader names the
/// line: this error itself is never shown.
fn bad(s: &Stream, msg: &str) -> Error {
    Error::Bad { file: s.path.clone(), line: 0, msg: msg.into() }
}

fn io(path: &str, source: std::io::Error) -> Error {
    Error::Fs(umcf_core::FsError::Io { path: path.to_string(), source })
}

/// An ASCII `vectorField` (the mesh's `points`), parsed as it is read. `Ok(None)` when the file
/// is not one (binary: read whole).
pub(crate) fn vectors(s: &mut Stream, limits: &Limits) -> Result<Option<Vec<[f64; 3]>>> {
    let Some((header, mut buf, open, n)) = s.list(&["vectorField"], limits)? else { return Ok(None) };
    let plain = Header { class: Some("vectorField".into()), ..header };
    let mut out: Vec<[f64; 3]> = Vec::new();
    // Not more than a chunk could hold up front (a vector takes 7 bytes of text or more): the
    // count is the file's word.
    out.try_reserve_exact(n.min(CHUNK / 7)).map_err(|_| Error::TooLarge { what: "list (out of memory)", limit: n })?;
    let mut at = open;
    while out.len() < n {
        // The whole vectors read so far: each ends at its `)`, and no more than are still due
        // (the list's own `)` may follow).
        let rest = buf.get(at..).unwrap_or_default();
        let need = n - out.len();
        let (mut k, mut end) = (0, 0);
        for (i, c) in rest.iter().enumerate() {
            if *c == b')' {
                k += 1;
                end = i + 1;
                if k == need {
                    break;
                }
            }
        }
        if k > 0 {
            let seg = rest.get(..end).unwrap_or_default();
            let mut text = format!("{k}\n(").into_bytes();
            text.extend_from_slice(seg);
            text.push(b')');
            let got = Reader::new(&s.path, &text, 0, &plain).vectors(limits)?;
            if out.capacity() - out.len() < got.len() {
                // Doubling, but never past the count: the vector ends as long as the list.
                let more = got.len().max(out.len()).min(n - out.len());
                out.try_reserve_exact(more)
                    .map_err(|_| Error::TooLarge { what: "list (out of memory)", limit: out.len() + more })?;
            }
            out.extend_from_slice(&got);
            at += end;
            continue;
        }
        if rest.len() > ELEMENT {
            return Err(bad(s, "expected a vector `(x y z)`"));
        }
        buf.drain(..at);
        at = 0;
        if !s.fill(&mut buf)? {
            return Err(Error::Truncated { file: s.path.clone() });
        }
    }
    // The list's `)`.
    loop {
        let rest = buf.get(at..).unwrap_or_default();
        match rest.iter().position(|c| !c.is_ascii_whitespace()) {
            Some(i) if rest.get(i) == Some(&b')') => return Ok(Some(out)),
            Some(_) => return Err(bad(s, "expected `)`")),
            None => {
                buf.clear();
                at = 0;
                if !s.fill(&mut buf)? {
                    return Err(Error::Truncated { file: s.path.clone() });
                }
            }
        }
    }
}

/// An ASCII `faceList` from face `skip` on, as `Reader::face_list_from`, parsed as it is read:
/// the text of the faces before `skip` is passed over as it comes and never kept. `Ok(None)`
/// when the file is not an ASCII `faceList`, or its list doesn't start within the first few
/// megabytes (then it is read whole).
pub(crate) fn face_list_from(s: &mut Stream, skip: usize, limits: &Limits) -> Result<Option<FaceTail>> {
    let Some((header, mut buf, open, n)) = s.list(&["faceList"], limits)? else { return Ok(None) };
    // Each face is `k(a b c …)`: the faces before `skip` are passed over as they come, checked
    // for that form (digits, then `(`, no `(` before the `)`); anything else, a comment too, is
    // left to the whole-file reader.
    let skip = skip.min(n);
    let mut passed = 0usize;
    let mut at = open;
    let mut state = Skip::Between;
    let mut size: u64 = 0;
    while passed < skip {
        let rest = buf.get(at..).unwrap_or_default();
        let mut found = None;
        let mut k = 0;
        while let Some(&c) = rest.get(k) {
            if let Skip::Labels = state {
                // The labels up to the face's `)` in one pass, the hot path: what they are isn't
                // checked (nor is it by the whole-file reader), only that no `(` comes first.
                let tail = rest.get(k..).unwrap_or_default();
                let end = tail.iter().position(|c| matches!(c, b'(' | b')')).unwrap_or(tail.len());
                if tail.get(end) == Some(&b'(') {
                    return Err(bad(s, "expected a face `k(a b c …)`"));
                }
                k += end;
                if end == tail.len() {
                    break;
                }
                k += 1;
                passed += 1;
                state = Skip::Between;
                if passed == skip {
                    found = Some(k);
                    break;
                }
                continue;
            }
            state = match (state, c) {
                (Skip::Between, c) if c.is_ascii_whitespace() => Skip::Between,
                (Skip::Between | Skip::Count, c) if c.is_ascii_digit() => {
                    // The face's size, within what the whole-file reader takes.
                    let d = u64::from(c - b'0');
                    let before = if let Skip::Count = state { size } else { 0 };
                    size = before.checked_mul(10).and_then(|v| v.checked_add(d)).unwrap_or(u64::MAX);
                    if size > limits.max_elements as u64 {
                        return Err(bad(s, "a face's size is too large"));
                    }
                    Skip::Count
                }
                (Skip::Count | Skip::Open, c) if c.is_ascii_whitespace() => Skip::Open,
                (Skip::Count | Skip::Open, b'(') => Skip::Labels,
                _ => return Err(bad(s, "expected a face `k(a b c …)`")),
            };
            k += 1;
        }
        if let Some(k) = found {
            at += k;
            continue;
        }
        buf.clear();
        at = 0;
        if !s.fill(&mut buf)? {
            return Err(Error::Truncated { file: s.path.clone() });
        }
    }
    // The rest as a list of its own, `n - skip (` and the faces from `skip` on, in one buffer.
    let mut text = format!("{}\n(", n - skip).into_bytes();
    text.extend_from_slice(buf.get(at..).unwrap_or_default());
    drop(buf);
    while s.fill(&mut text)? {}
    let plain = Header { class: Some("faceList".into()), ..header };
    let mut r = Reader::new(&s.path, &text, 0, &plain);
    let (_, offsets, points) = r.face_list_from(limits, 0)?;
    Ok(Some((n, offsets, points)))
}

/// Where the skip over the internal faces is in a face `k(a b c …)`.
#[derive(Clone, Copy)]
enum Skip {
    /// Between faces.
    Between,
    /// In `k`.
    Count,
    /// After `k`, before `(`.
    Open,
    /// In the labels.
    Labels,
}

/// Where a list starts: its count `N` and the index just after its `(`.
enum Start {
    Found(usize, usize),
    /// Something else is there: not a list this reader takes.
    NotHere,
    /// The text ends before it is known.
    NeedMore,
}

/// `N (` at the start of `text`, after whitespace and comments.
fn list_start(text: &[u8], limits: &Limits) -> Result<Start> {
    let mut k = 0;
    loop {
        while text.get(k).is_some_and(u8::is_ascii_whitespace) {
            k += 1;
        }
        if text.get(k..k + 2) == Some(b"//") {
            match text.get(k..).and_then(|r| r.iter().position(|c| *c == b'\n')) {
                Some(e) => k += e,
                None => return Ok(Start::NeedMore),
            }
        } else if text.get(k..k + 2) == Some(b"/*") {
            match text.get(k + 2..).and_then(|r| r.windows(2).position(|w| w == b"*/")) {
                Some(e) => k += e + 4,
                None => return Ok(Start::NeedMore),
            }
        } else if text.get(k) == Some(&b'/') && text.get(k + 1).is_none() {
            // A comment's start, split from the rest.
            return Ok(Start::NeedMore);
        } else {
            break;
        }
    }
    let digits = text.get(k..).unwrap_or_default().iter().take_while(|c| c.is_ascii_digit()).count();
    let mut j = k + digits;
    while text.get(j).is_some_and(u8::is_ascii_whitespace) {
        j += 1;
    }
    match text.get(j) {
        None => Ok(Start::NeedMore),
        Some(b'(') if digits > 0 => {
            let count = std::str::from_utf8(text.get(k..k + digits).unwrap_or_default()).unwrap_or("");
            let n: usize = count.parse().map_err(|_| Error::TooLarge { what: "list", limit: limits.max_elements })?;
            if n > limits.max_elements {
                return Err(Error::TooLarge { what: "list", limit: limits.max_elements });
            }
            Ok(Start::Found(n, j + 1))
        }
        Some(_) => Ok(Start::NotHere),
    }
}
