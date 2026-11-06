//! A field file's outline, read as a stream: everything but its lists' numbers. The file
//! (compressed or not) is decompressed and scanned as it comes, in chunks, so neither it nor its
//! numbers are ever held whole; a list keeps its kind and length ([`ListData::has_values`] is
//! false), its numbers checked as [`Field::parse`] checks them but not kept. The rest (header,
//! `dimensions`, `boundaryField`) is the same [`Document`] [`Field::parse`] makes.
//!
//! The scan is [`scan`]'s, byte for byte: comments, strings and code blocks are text, and lists
//! start at `nonuniform` or at a bare `List<T>`. A bare `List<T>` that doesn't read as data
//! stays text, as there: it is read ahead and taken only once it reads. What is kept (the text,
//! and 64 bytes for each list), and what bare lists read ahead all together, are each bounded by
//! `max_text`; a file over either is `TooLarge`. Errors name the same line.
//!
//! Differences, none of them in a file OpenFOAM writes:
//! - a list before the `FoamFile` header is read as ASCII, where [`Field::parse`] reads it in
//!   the format the header gives further on (a binary file that has one fails here); a bare
//!   list inside the header, before its `}`, isn't read as one, and a `nonuniform` one there
//!   fails the file;
//! - a list's header, its whitespace, one number or an `N{value}` element over 64 KB is an error
//!   (or text, in a bare list), and a few malformed lists fail on another line;
//! - the placeholders' tag is chosen from the text alone, so list data that spells
//!   `__umcfList…` may change the tag (not the lists they name).

use std::io::Read;

use umcf_core::{CaseFs, FsError};
use umcf_foam_dict::Document;

use crate::scan::{self, Tag};
use crate::{Error, Field, Format, GZIP_MAGIC, Header, Kind, Limits, ListData, Result};

/// How much is read from the file at once.
const CHUNK: usize = 1 << 20;
/// How far a list's header (`List<T> N (`) and a `N{value}` element may reach.
const HEAD: usize = 64 << 10;

impl Field {
    /// The outline of `rel` (or of `rel.gz` when only the compressed file exists), read as a
    /// stream: what [`Field::read`] gives, with each list's kind and length but not its values
    /// (`N{value}` lists keep their one value). `limits.max_bytes` bounds the file as it
    /// decompresses, `limits.max_values` a list's length, and `max_text` what is kept (the file
    /// without its lists) and what bare lists may read ahead.
    pub fn outline(fs: &dyn CaseFs, rel: &str, limits: &Limits, max_text: u64) -> Result<Option<Field>> {
        for path in [rel.to_string(), format!("{rel}.gz")] {
            let Some(mut r) = fs.reader(&path, limits.max_bytes)? else { continue };
            let mut magic = Vec::with_capacity(2);
            (&mut r).take(2).read_to_end(&mut magic).map_err(|e| io_error(&path, false, e))?;
            let compressed = magic == GZIP_MAGIC;
            let head = std::io::Cursor::new(magic).chain(r);
            let r: Box<dyn Read + Send> =
                if compressed { Box::new(flate2::read::MultiGzDecoder::new(head)) } else { Box::new(head) };
            let mut src = Src {
                r,
                path,
                compressed,
                chunk: vec![0; CHUNK],
                buf: Vec::new(),
                pos: 0,
                eof: false,
                seen: 0,
                max: limits.max_bytes,
                line: 1,
                text_max: max_text,
                last: None,
                lists: 0,
                held_left: max_text,
            };
            return outline(&mut src, limits).map(Some);
        }
        Ok(None)
    }
}

/// A read error of `path`: a gzip error when it is compressed, else the file system's.
fn io_error(path: &str, compressed: bool, e: std::io::Error) -> Error {
    if compressed {
        Error::Gzip(format!("{path}: {e}"))
    } else {
        Error::Fs(FsError::Io { path: path.to_string(), source: e })
    }
}

/// The file's bytes, as they come: `buf[pos..]` is read and not yet taken.
struct Src {
    r: Box<dyn Read + Send>,
    path: String,
    compressed: bool,
    /// What is read into, `CHUNK` bytes.
    chunk: Vec<u8>,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
    /// Every byte that came out, against `max`.
    seen: u64,
    max: u64,
    /// The line `pos` is on, for errors.
    line: usize,
    /// The bound on what is kept as text.
    text_max: u64,
    /// The last byte taken: whether the next starts a token.
    last: Option<u8>,
    /// How many lists are kept, for the text bound.
    lists: usize,
    /// What held scans may still read ahead, all of them together: failed attempts at bare
    /// lists can't read the same bytes over and over without bound.
    held_left: u64,
}

impl Src {
    /// Reads the next chunk; `false` at the end of the file. What was taken is dropped first.
    fn fill(&mut self) -> Result<bool> {
        if self.eof {
            return Ok(false);
        }
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let n = loop {
            match self.r.read(&mut self.chunk) {
                Ok(n) => break n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io_error(&self.path, self.compressed, e)),
            }
        };
        if n == 0 {
            self.eof = true;
            return Ok(false);
        }
        self.buf.extend_from_slice(self.chunk.get(..n).unwrap_or_default());
        self.seen = self.seen.saturating_add(n as u64);
        if self.seen > self.max {
            return Err(Error::TooLarge { what: "file", limit: self.max });
        }
        Ok(true)
    }

    /// The byte `k` past the next one to take; `None` past the end of the file.
    fn peek(&mut self, k: usize) -> Result<Option<u8>> {
        while self.pos + k >= self.buf.len() {
            if !self.fill()? {
                return Ok(None);
            }
        }
        Ok(self.buf.get(self.pos + k).copied())
    }

    /// Takes `k` bytes (at most what is read).
    fn take(&mut self, k: usize) {
        let end = (self.pos + k).min(self.buf.len());
        if end > self.pos {
            self.last = self.buf.get(end - 1).copied();
        }
        self.line += self.buf.get(self.pos..end).unwrap_or_default().iter().filter(|b| **b == b'\n').count();
        self.pos = end;
    }

    /// Takes `k` bytes into `text`, within the text bound.
    fn copy(&mut self, text: &mut Vec<u8>, k: usize) -> Result<()> {
        let end = (self.pos + k).min(self.buf.len());
        text.extend_from_slice(self.buf.get(self.pos..end).unwrap_or_default());
        self.take(k);
        self.check_kept(text.len())
    }

    /// Whether what is kept, `text` bytes and the lists, is within the text bound.
    fn check_kept(&self, text: usize) -> Result<()> {
        let kept = text.saturating_add(self.lists.saturating_mul(LIST_COST));
        if kept as u64 > self.text_max {
            return Err(Error::TooLarge { what: "field file without its lists", limit: self.text_max });
        }
        Ok(())
    }

    /// The line of the byte `k` past the next one (read already).
    fn line_at(&self, k: usize) -> usize {
        let end = (self.pos + k).min(self.buf.len());
        self.line + self.buf.get(self.pos..end).unwrap_or_default().iter().filter(|b| **b == b'\n').count()
    }

    /// Copies up to `needle` (and it, with `through`), or to the end of the file.
    fn copy_until(&mut self, text: &mut Vec<u8>, needle: &[u8], through: bool) -> Result<()> {
        loop {
            let mut at = true;
            for (j, c) in needle.iter().enumerate() {
                match self.peek(j)? {
                    Some(b) if b == *c => {}
                    Some(_) => at = false,
                    None => return self.copy(text, needle.len()),
                }
            }
            if at {
                if through {
                    self.copy(text, needle.len())?;
                }
                return Ok(());
            }
            self.copy(text, 1)?;
        }
    }

    /// Skips whitespace from `k` past the next byte, [`HEAD`] bytes of it at most; the offset
    /// after it.
    fn skip_ws(&mut self, mut k: usize) -> Result<usize> {
        let start = k;
        while k - start < HEAD && self.peek(k)?.is_some_and(|b| b.is_ascii_whitespace()) {
            k += 1;
        }
        Ok(k)
    }

    /// Charges `n` bytes read ahead by a held scan.
    fn charge(&mut self, n: usize) -> Result<()> {
        self.held_left = self
            .held_left
            .checked_sub(n as u64)
            .ok_or(Error::TooLarge { what: "bare lists read ahead", limit: self.text_max })?;
        Ok(())
    }

    /// A list error on the line of the next byte.
    fn bad(&self, msg: impl Into<String>) -> Error {
        Error::BadList { msg: msg.into(), line: self.line }
    }

    /// An error about the byte `k` past the next one, on its line.
    fn bad_at(&self, k: usize, msg: impl Into<String>) -> Error {
        Error::BadList { msg: msg.into(), line: self.line_at(k) }
    }

    /// Runs `step` on each byte from `from` past the next one until it says the list ended;
    /// the offset just past that byte. Held, nothing is taken (the bytes stay read, within
    /// `held_left`); otherwise the bytes are taken as they are scanned (`from` is then 0,
    /// and so is the offset).
    fn scan(
        &mut self,
        hold: bool,
        from: usize,
        mut step: impl FnMut(u8) -> Step,
    ) -> Result<std::result::Result<usize, Ended>> {
        let mut k = from;
        loop {
            let rest = self.buf.get(self.pos + k..).unwrap_or_default();
            let mut used = 0;
            let mut lines = 0;
            let mut end = None;
            for b in rest {
                used += 1;
                // A line is counted once the byte is taken: an error names the line it is on.
                match step(*b) {
                    Step::More => lines += usize::from(*b == b'\n'),
                    Step::End => {
                        end = Some(Ok(()));
                        break;
                    }
                    Step::Bad(msg) => {
                        end = Some(Err(Ended::Bad(msg)));
                        break;
                    }
                }
            }
            if hold {
                k += used;
                self.charge(used)?;
            } else {
                if used > 0 {
                    self.last = self.buf.get(self.pos + used - 1).copied();
                }
                self.pos += used;
                self.line += lines;
            }
            match end {
                Some(Ok(())) => return Ok(Ok(k)),
                Some(Err(e)) => return Ok(Err(e)),
                None if !self.fill()? => return Ok(Err(Ended::Eof)),
                None => {}
            }
        }
    }
}

/// Why a list's scan stopped short.
enum Ended {
    Bad(String),
    /// The file ended first.
    Eof,
}

enum Step {
    More,
    End,
    Bad(String),
}

/// Whether `b` ends a token, as `scan` tells it.
fn is_delim(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';' | b'"')
}

/// Scans `src` as `scan::scan` scans a whole file, keeping the text and each list's kind and
/// length, then parses the text with placeholders as [`Field::parse`] does.
fn outline(src: &mut Src, limits: &Limits) -> Result<Field> {
    let mut text = Vec::new();
    let mut lists = Vec::new();
    // Where each list's placeholder goes in `text` (the tag is chosen once `text` is whole).
    let mut marks = Vec::new();
    let mut header: Option<Header> = None;
    let mut found = HeaderScan::default();
    while let Some(b) = src.peek(0)? {
        // As in the file: after a list, the byte that ended it (a binary list of size 0 written
        // as its size alone ends on a digit).
        let at_token = src.last.is_none_or(is_delim);
        match (b, src.peek(1)?) {
            (b'/', Some(b'/')) => src.copy_until(&mut text, b"\n", false)?,
            (b'/', Some(b'*')) => {
                src.copy(&mut text, 2)?;
                src.copy_until(&mut text, b"*/", true)?;
            }
            (b'#', Some(b'{')) => {
                src.copy(&mut text, 2)?;
                src.copy_until(&mut text, b"#}", true)?;
            }
            (b'"', _) => {
                src.copy(&mut text, 1)?;
                while let Some(c) = src.peek(0)? {
                    match c {
                        b'\\' => {
                            // The escaped byte may not be read yet.
                            src.peek(1)?;
                            src.copy(&mut text, 2)?;
                        }
                        b'"' => {
                            src.copy(&mut text, 1)?;
                            break;
                        }
                        _ => src.copy(&mut text, 1)?,
                    }
                }
            }
            // A bare `List<T>` (zone files): one that doesn't read as a list's start stays text.
            // The header counts as read only once a list is taken.
            (b'L', _) if at_token && typed_list(src)? => {
                let h = match &header {
                    Some(h) => Ok(h.clone()),
                    None => found.header(&text),
                };
                // A header that doesn't end before it: no list here (as text, `Field::parse`
                // reads the header wherever its `}` is).
                let Ok(h) = h else {
                    src.copy(&mut text, 1)?;
                    continue;
                };
                let format = (h.format, h.arch);
                match list_head(src, 0, format.0 == Format::Binary)
                    .and_then(|head| list(src, head, format, limits, true))
                {
                    Ok(list) => {
                        header = Some(h);
                        keep(src, &mut text, &mut marks, &mut lists, list)?;
                    }
                    Err(Error::BadList { .. }) => src.copy(&mut text, 1)?,
                    Err(e) => return Err(e),
                }
            }
            (b'n', _) if at_token && keyword(src, b"nonuniform")? => {
                if header.is_none() {
                    header = Some(found.header(&text)?);
                }
                let (format, arch) =
                    header.as_ref().map_or((Format::Ascii, Default::default()), |h| (h.format, h.arch));
                src.take(b"nonuniform".len());
                text.extend_from_slice(b"nonuniform ");
                let head = list_head(src, 0, format == Format::Binary)?;
                let list = list(src, head, (format, arch), limits, false)?;
                keep(src, &mut text, &mut marks, &mut lists, list)?;
            }
            _ => src.copy(&mut text, 1)?,
        }
    }
    // The header as `Field::parse` reads it, from the whole text: the lists were read in the
    // format of the one before the first list (or none), and must have been read in its.
    let whole = crate::read_header(&text)?;
    if header.is_some_and(|h| (h.format, h.arch) != (whole.format, whole.arch)) {
        return Err(Error::Unsupported("a list before the FoamFile header that sets its format".into()));
    }
    let header = whole;
    // The placeholders, chosen as for the whole file: no word of the text spells one.
    let tag = Tag::for_input(&text);
    let mut out = Vec::with_capacity(text.len() + marks.len() * 16);
    let mut from = 0;
    for (k, at) in marks.iter().enumerate() {
        out.extend_from_slice(text.get(from..*at).unwrap_or_default());
        out.extend_from_slice(tag.word(k).as_bytes());
        from = *at;
    }
    out.extend_from_slice(text.get(from..).unwrap_or_default());
    let doc = Document::parse_bytes(&out)?;
    tag.check(&doc, lists.len())?;
    Ok(Field { header, doc, lists, tag, size: src.seen })
}

/// What a list kept costs against the text bound, besides its placeholder's space: its
/// [`ListData`], its mark and its placeholder word.
const LIST_COST: usize = 64;

/// Keeps `list`: its placeholder's place in `text`, within the text bound (a file of a million
/// empty lists costs what they are kept in).
fn keep(
    src: &mut Src,
    text: &mut Vec<u8>,
    marks: &mut Vec<usize>,
    lists: &mut Vec<ListData>,
    list: ListData,
) -> Result<()> {
    marks.push(text.len());
    text.push(b' ');
    lists.push(list);
    src.lists = lists.len();
    src.check_kept(text.len())
}

/// Where the header is in the text, found as the text grows: each byte is looked at once
/// (bare lists that fail may ask again and again).
#[derive(Default)]
struct HeaderScan {
    /// How far the text was searched.
    searched: usize,
    /// Where `FoamFile` starts.
    foam: Option<usize>,
    /// The header, once its `}` is in the text; why it can't be read, when it can't.
    read: Option<std::result::Result<Header, String>>,
}

impl HeaderScan {
    /// The header in the text so far, as `read_header_end` reads it from the whole text: none
    /// before a `FoamFile`; one that doesn't end yet would take in the list's numbers as
    /// [`Field::parse`] reads it: an error.
    fn header(&mut self, text: &[u8]) -> Result<Header> {
        match &self.read {
            Some(Ok(h)) => return Ok(h.clone()),
            Some(Err(e)) => return Err(Error::Unsupported(e.clone())),
            None => {}
        }
        let foam = match self.foam {
            Some(f) => f,
            None => {
                let from = self.searched.saturating_sub(7);
                match text.get(from..).and_then(|t| t.windows(8).position(|w| w == b"FoamFile")) {
                    Some(p) => {
                        self.foam = Some(from + p);
                        self.searched = from + p;
                        from + p
                    }
                    None => {
                        self.searched = text.len();
                        return crate::read_header(b"");
                    }
                }
            }
        };
        let from = self.searched.max(foam);
        match text.get(from..).and_then(|t| t.iter().position(|b| *b == b'}')) {
            Some(p) => {
                let h = crate::read_header(text.get(..from + p + 1).unwrap_or(text));
                self.read = Some(h.as_ref().map(Header::clone).map_err(|e| format!("the FoamFile header: {e}")));
                h
            }
            None => {
                self.searched = text.len();
                Err(Error::Unsupported("the FoamFile header doesn't end before the data".into()))
            }
        }
    }
}

/// `word` here, followed by whitespace (the token start is checked by the caller).
fn keyword(src: &mut Src, word: &[u8]) -> Result<bool> {
    Ok(prefix_at(src, 0, word)? && src.peek(word.len())?.is_some_and(|b| b.is_ascii_whitespace()))
}

/// `List<T>` here, with a value type the scanner reads.
fn typed_list(src: &mut Src) -> Result<bool> {
    if !prefix_at(src, 0, b"List<")? {
        return Ok(false);
    }
    let mut name = Vec::new();
    for j in 5..5 + 32 {
        match src.peek(j)? {
            Some(b'>') => return Ok(std::str::from_utf8(&name).ok().and_then(Kind::from_name).is_some()),
            Some(b) => name.push(b),
            None => return Ok(false),
        }
    }
    Ok(false)
}

/// A list's start, read ahead and not yet taken.
struct Head {
    kind: Option<Kind>,
    n: usize,
    /// Its `(` or `{` (`None`: a binary list of size 0 written as its size alone).
    open: Option<u8>,
    /// The offset just past the `(`, `{` or (no bracket) the size.
    end: usize,
}

/// `List<T> N (`, `List<T> N{`, or the untyped `N (`, from `k` past the next byte.
fn list_head(src: &mut Src, k: usize, binary: bool) -> Result<Head> {
    let mut k = src.skip_ws(k)?;
    let mut kind = None;
    if prefix_at(src, k, b"List<")? {
        let mut name = Vec::new();
        let mut j = k + 5;
        loop {
            match src.peek(j)? {
                Some(b'>') => break,
                Some(b) if j < k + 5 + HEAD => name.push(b),
                _ => return Err(src.bad_at(k, "unclosed `List<`")),
            }
            j += 1;
        }
        let name = String::from_utf8_lossy(&name).into_owned();
        kind = Some(Kind::from_name(&name).ok_or_else(|| Error::Unsupported(format!("List<{name}>")))?);
        k = src.skip_ws(j + 1)?;
    }
    // The digits, leading zeros apart (`0002` is 2, as the whole read parses it).
    let mut digits = String::new();
    let mut len = 0;
    while let Some(b) = src.peek(k + len)?.filter(u8::is_ascii_digit) {
        len += 1;
        if b != b'0' || !digits.is_empty() {
            digits.push(char::from(b));
        }
        if digits.len() > 20 || len > HEAD {
            return Err(src.bad_at(k, "list length out of range"));
        }
    }
    if len == 0 {
        return Err(src.bad_at(k, "expected the list length"));
    }
    let n: usize =
        if digits.is_empty() { 0 } else { digits.parse().map_err(|_| src.bad_at(k, "list length out of range"))? };
    let after = k + len;
    let k = src.skip_ws(after)?;
    match src.peek(k)? {
        Some(c @ (b'(' | b'{')) => Ok(Head { kind, n, open: Some(c), end: k + 1 }),
        // OpenFOAM writes an empty binary list as its size alone (see `scan::read_list`).
        _ if binary && n == 0 => Ok(Head { kind, n, open: None, end: after }),
        _ => Err(src.bad_at(k, "expected `(` or `{` after the list length")),
    }
}

/// `word` at `k` past the next byte.
fn prefix_at(src: &mut Src, k: usize, word: &[u8]) -> Result<bool> {
    for (j, c) in word.iter().enumerate() {
        if src.peek(k + j)? != Some(*c) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The list `head` starts, taken from the stream: its kind and length, its elements counted.
/// A held list (a bare `List<T>`) is read ahead first and taken only once it reads, so that one
/// that doesn't (a `BadList` error) is still there to be text.
fn list(
    src: &mut Src,
    head: Head,
    (format, arch): (Format, crate::Arch),
    limits: &Limits,
    hold: bool,
) -> Result<ListData> {
    let Head { kind, n, open, end } = head;
    // Offsets from here on are from the next byte; a list not held takes its start now.
    let base = if hold {
        end
    } else {
        src.take(end);
        0
    };
    let binary = format == Format::Binary;
    let (list, at) = match open {
        None => (ListData::flat(kind.unwrap_or(Kind::Scalar), 0, Vec::new()), base),
        Some(b'{') => {
            // `N{value}`: the one value is small; it is read as `Field::parse` reads it, its
            // errors first, then the `}` after it.
            let mut elem = Vec::new();
            let mut j = base;
            let closed = loop {
                match src.peek(j)? {
                    Some(b'}') => break true,
                    Some(b) if elem.len() < HEAD => elem.push(b),
                    _ => break false,
                }
                j += 1;
            };
            if hold {
                src.charge(j - base)?;
            }
            let start = src.line_at(base);
            let (value, kind, used) = scan::read_ascii_elements(&elem, 0, 1, kind, limits).map_err(|e| match e {
                // Its offset in the element, as a line.
                Error::BadList { msg, line: at } => {
                    Error::BadList { msg, line: start + scan::line_of(&elem, at).saturating_sub(1) }
                }
                e => e,
            })?;
            let rest =
                used + elem.get(used..).unwrap_or_default().iter().take_while(|b| b.is_ascii_whitespace()).count();
            if !closed || rest < elem.len() {
                return Err(src.bad_at(base + rest, "expected `}` after the uniform list value"));
            }
            scan::check_count(n, kind.components(), limits)?;
            (ListData::repeated(kind, n, value), j + 1)
        }
        _ if n == 0 => {
            let k = src.skip_ws(base)?;
            if src.peek(k)? != Some(b')') {
                return Err(src.bad_at(k, "expected `)` closing an empty list"));
            }
            (ListData::flat(kind.unwrap_or(Kind::Scalar), 0, Vec::new()), k + 1)
        }
        _ if binary => {
            scan::check_count(n, kind.unwrap_or(Kind::Scalar).components(), limits)?;
            let kind = kind.ok_or_else(|| Error::Unsupported("binary list without a List<T> type".into()))?;
            let width = if kind == Kind::Label { arch.label_bytes } else { arch.scalar_bytes };
            let mut left = n
                .checked_mul(kind.components())
                .and_then(|v| v.checked_mul(width))
                .ok_or(Error::TooLarge { what: "list", limit: usize::MAX as u64 })?;
            let line = src.line;
            let scanned = src.scan(hold, base, |b| match left {
                0 if b == b')' => Step::End,
                0 => Step::Bad(format!("expected `)` after {n} binary elements")),
                _ => {
                    left -= 1;
                    Step::More
                }
            })?;
            match scanned {
                Ok(at) => (ListData::counted(kind, n), at),
                Err(Ended::Bad(msg)) => return Err(src.bad(msg)),
                // The data is all there, its `)` isn't (as `scan::read_binary` tells them apart).
                Err(Ended::Eof) if left == 0 => {
                    return Err(src.bad(format!("expected `)` after {n} binary elements")));
                }
                Err(Ended::Eof) => return Err(Error::Truncated { line }),
            }
        }
        _ => {
            scan::check_count(n, kind.unwrap_or(Kind::Scalar).components(), limits)?;
            let kind = match kind {
                Some(k) => k,
                None => untyped_kind(src, base)?,
            };
            let at = count_ascii(src, n, kind, hold, base)?;
            (ListData::counted(kind, n), at)
        }
    };
    src.take(at);
    Ok(list)
}

/// An untyped list's kind, from its first element at `k` (see `scan::read_ascii_elements`).
fn untyped_kind(src: &mut Src, k: usize) -> Result<Kind> {
    let k = src.skip_ws(k)?;
    if src.peek(k)? != Some(b'(') {
        return Ok(Kind::Scalar);
    }
    let mut inner = Vec::new();
    let mut j = k + 1;
    loop {
        match src.peek(j)? {
            Some(b')') => break,
            Some(b) if inner.len() < HEAD => inner.push(b),
            _ => return Err(src.bad_at(k, "unclosed element")),
        }
        j += 1;
    }
    let comps = inner.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty()).count();
    if comps == 1 {
        return Ok(Kind::SphericalTensor);
    }
    Kind::from_components(comps).ok_or_else(|| src.bad_at(k, format!("{comps}-component element")))
}

/// Scans `n` ASCII elements and the `)` after them from `from`, as `scan::read_ascii_elements`
/// reads them: numbers for scalars and labels, `(…)` groups of the kind's component count for
/// the others. Each number is checked as that reader checks it, and not kept. The offset past
/// the `)` (see [`Src::scan`]).
fn count_ascii(src: &mut Src, n: usize, kind: Kind, hold: bool, from: usize) -> Result<usize> {
    let scalar = matches!(kind, Kind::Scalar | Kind::Label);
    let comps = kind.components();
    let name = kind.name();
    let mut word = Vec::new();
    let mut count = 0usize;
    let mut inside = false;
    let mut in_group = 0usize;
    let scanned = src.scan(hold, from, |b| {
        let delim = b.is_ascii_whitespace() || matches!(b, b'(' | b')' | b'{' | b'}');
        if !delim {
            if word.len() >= HEAD {
                return Step::Bad("expected a number, found a word too long for one".to_string());
            }
            word.push(b);
            return Step::More;
        }
        if !word.is_empty() {
            if !is_number(&word) {
                return Step::Bad(format!("expected a number, found `{}`", String::from_utf8_lossy(&word)));
            }
            word.clear();
            if scalar {
                count += 1;
                if count > n {
                    return Step::Bad(format!("expected `)` after {n} elements"));
                }
            } else if !inside {
                return Step::Bad(format!("expected `(` starting a {name}"));
            } else {
                in_group += 1;
                if in_group > comps {
                    return Step::Bad(format!("expected `)` closing a {name}"));
                }
            }
        }
        match b {
            _ if b.is_ascii_whitespace() => Step::More,
            b'(' if !scalar && !inside => {
                if count == n {
                    return Step::Bad(format!("expected `)` after {n} elements"));
                }
                inside = true;
                in_group = 0;
                count += 1;
                Step::More
            }
            b')' if inside => {
                if in_group != comps {
                    return Step::Bad(format!("expected a number in a {name}"));
                }
                inside = false;
                Step::More
            }
            b')' if count == n => Step::End,
            b')' => Step::Bad(format!("{count} elements where the list says {n}")),
            _ if inside => Step::Bad(format!("expected `)` closing a {name}")),
            _ if scalar => Step::Bad(format!("expected a number, found `{}`", char::from(b))),
            _ => Step::Bad(format!("expected `(` starting a {name}")),
        }
    })?;
    match scanned {
        Ok(at) => Ok(at),
        Err(Ended::Bad(msg)) => Err(src.bad(msg)),
        Err(Ended::Eof) => Err(src.bad("the file ends inside a list")),
    }
}

/// Whether `word` reads as a number, as `scan`'s number reader takes one: whole by the fast
/// parser, or (`nan`, `-inf`) by the standard one when at most 64 bytes long.
fn is_number(word: &[u8]) -> bool {
    fast_float2::parse::<f64, _>(word).is_ok()
        || (word.len() <= 64 && std::str::from_utf8(word).ok().and_then(|s| s.parse::<f64>().ok()).is_some())
}
