//! OpenFOAM field files: `vol*Field`, `surface*Field`, `point*Field` and the like.
//!
//! [`Field::parse`] reads a field in ASCII or binary format, gzip-compressed or not. Its big
//! `nonuniform List<T>` data is read straight into numbers ([`ListData`]); everything else
//! (header, `dimensions`, the patches of `boundaryField`) is a [`Document`], so the small parts
//! keep their full structure. [`times`] finds time and processor directories.
//!
//! Reading only: rewriting big fields is not this crate's job yet. Small field files can be edited
//! losslessly with `umcf-foam-dict` directly.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod scan;
pub mod times;

use std::io::{Read, Write};

use umcf_foam_dict::{Document, Entry, Node};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Dict(#[from] umcf_foam_dict::Error),
    #[error("line {line}: {msg}")]
    BadList { msg: String, line: usize },
    #[error("binary data ends early (list starting at line {line})")]
    Truncated { line: usize },
    #[error("{what} exceeds the limit of {limit}")]
    TooLarge { what: &'static str, limit: u64 },
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("gzip: {0}")]
    Gzip(String),
    #[error(transparent)]
    Fs(#[from] umcf_core::FsError),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Bounds on hostile input: the list lengths come from the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Decompressed size of a `.gz` file, and the size of a file read from a case.
    pub max_bytes: u64,
    /// Numbers in one list (elements × components).
    pub max_values: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_bytes: 8 << 30, max_values: 1 << 31 }
    }
}

/// The value type of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Scalar,
    Vector,
    SphericalTensor,
    SymmTensor,
    Tensor,
    Label,
}

impl Kind {
    pub fn components(self) -> usize {
        match self {
            Kind::Scalar | Kind::SphericalTensor | Kind::Label => 1,
            Kind::Vector => 3,
            Kind::SymmTensor => 6,
            Kind::Tensor => 9,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Scalar => "scalar",
            Kind::Vector => "vector",
            Kind::SphericalTensor => "sphericalTensor",
            Kind::SymmTensor => "symmTensor",
            Kind::Tensor => "tensor",
            Kind::Label => "label",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Some(match name {
            "scalar" => Kind::Scalar,
            "vector" => Kind::Vector,
            "sphericalTensor" => Kind::SphericalTensor,
            "symmTensor" => Kind::SymmTensor,
            "tensor" => Kind::Tensor,
            "label" => Kind::Label,
            _ => return None,
        })
    }

    fn from_components(n: usize) -> Option<Kind> {
        Some(match n {
            1 => Kind::Scalar,
            3 => Kind::Vector,
            6 => Kind::SymmTensor,
            9 => Kind::Tensor,
            _ => return None,
        })
    }
}

/// A list of values, stored as flat components (`x y z x y z …` for vectors).
#[derive(Debug, Clone, PartialEq)]
pub struct ListData {
    pub kind: Kind,
    len: usize,
    values: Vec<f64>,
    /// `N{value}`: one element stands for all `len`.
    repeated: bool,
}

impl ListData {
    fn flat(kind: Kind, len: usize, values: Vec<f64>) -> Self {
        ListData { kind, len, values, repeated: false }
    }

    fn repeated(kind: Kind, len: usize, element: Vec<f64>) -> Self {
        ListData { kind, len, values: element, repeated: true }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Written as `N{value}`: one stored element stands for all of them.
    pub fn is_repeated(&self) -> bool {
        self.repeated
    }

    /// Element `i` as its components.
    pub fn get(&self, i: usize) -> Option<&[f64]> {
        if i >= self.len {
            return None;
        }
        let c = self.kind.components();
        let start = if self.repeated { 0 } else { i.checked_mul(c)? };
        self.values.get(start..start.checked_add(c)?)
    }

    pub fn iter(&self) -> impl Iterator<Item = &[f64]> {
        (0..self.len).filter_map(|i| self.get(i))
    }

    /// The flat components, when every element is stored (not for `N{value}` lists).
    pub fn as_flat(&self) -> Option<&[f64]> {
        (!self.repeated).then_some(self.values.as_slice())
    }

    /// Smallest and largest value of each component (`None` for an empty list).
    pub fn component_range(&self) -> Option<Vec<(f64, f64)>> {
        let c = self.kind.components();
        let mut out = vec![(f64::INFINITY, f64::NEG_INFINITY); c];
        let mut any = false;
        for e in self.iter() {
            any = true;
            for (r, v) in out.iter_mut().zip(e) {
                r.0 = r.0.min(*v);
                r.1 = r.1.max(*v);
            }
        }
        any.then_some(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ascii,
    Binary,
}

/// `arch "LSB;label=32;scalar=64"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arch {
    pub big_endian: bool,
    pub label_bytes: usize,
    pub scalar_bytes: usize,
}

impl Default for Arch {
    fn default() -> Self {
        Arch { big_endian: false, label_bytes: 4, scalar_bytes: 8 }
    }
}

impl Arch {
    pub fn parse(s: &str) -> Result<Arch> {
        let mut a = Arch::default();
        for part in s.split(';').map(str::trim) {
            match part {
                "LSB" => a.big_endian = false,
                "MSB" => a.big_endian = true,
                "label=32" => a.label_bytes = 4,
                "label=64" => a.label_bytes = 8,
                "scalar=32" => a.scalar_bytes = 4,
                "scalar=64" => a.scalar_bytes = 8,
                "" => {}
                other => return Err(Error::Unsupported(format!("arch `{other}`"))),
            }
        }
        Ok(a)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub format: Format,
    pub arch: Arch,
    pub class: Option<String>,
    pub object: Option<String>,
    pub location: Option<String>,
    /// Free text; polyMesh files put their sizes here (`nPoints:… nCells:…`).
    pub note: Option<String>,
}

impl Header {
    /// A `name:value` count in the note, e.g. `count("nCells")`. blockMesh writes
    /// `nCells:400`; topoSet and changeDictionary write the mesh again as `nCells: 400`.
    pub fn count(&self, name: &str) -> Option<u64> {
        let note = self.note.as_deref()?;
        let mut words = note.split_whitespace();
        while let Some(w) = words.next() {
            let Some(rest) = w.strip_prefix(name).and_then(|r| r.strip_prefix(':')) else { continue };
            let value = if rest.is_empty() { words.next() } else { Some(rest) };
            if let Some(n) = value.and_then(|v| v.parse().ok()) {
                return Some(n);
            }
        }
        None
    }
}

/// A value read from a field.
#[derive(Debug, Clone, PartialEq)]
pub enum Value<'a> {
    /// `uniform 0`, `uniform (1 0 0)`: one element's components.
    Uniform(Vec<f64>),
    List(&'a ListData),
    /// Anything else, as compact text (`$internalField`, `#calc …`).
    Other(String),
}

#[derive(Debug, Clone)]
pub struct Field {
    pub header: Header,
    /// The file with its lists replaced by placeholders.
    pub doc: Document,
    lists: Vec<ListData>,
    tag: scan::Tag,
}

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Decompresses gzip data (several members too), refusing output above `limit` bytes.
pub fn gunzip(bytes: &[u8], limit: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(bytes)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| Error::Gzip(e.to_string()))?;
    if out.len() as u64 > limit {
        return Err(Error::TooLarge { what: "decompressed file", limit });
    }
    Ok(out)
}

/// Reads the `FoamFile { … }` header from the start of a file.
pub fn read_header(bytes: &[u8]) -> Result<Header> {
    read_header_end(bytes).map(|(h, _)| h)
}

/// The header and the offset just past its closing `}` (`None` when there is no header), where
/// the data of a list file (`points`, `owner`, `faces`) starts.
pub fn read_header_end(bytes: &[u8]) -> Result<(Header, Option<usize>)> {
    let mut h =
        Header { format: Format::Ascii, arch: Arch::default(), class: None, object: None, location: None, note: None };
    // The header usually follows a short banner, but a long comment block may push it further:
    // search the whole input. No header at all means an ASCII data file.
    let head = bytes;
    let Some(start) = head.windows(8).position(|w| w == b"FoamFile") else { return Ok((h, None)) };
    let Some(close) = head.get(start..).and_then(|s| s.iter().position(|b| *b == b'}')) else { return Ok((h, None)) };
    let text = String::from_utf8_lossy(head.get(start..start + close + 1).unwrap_or(&[]));
    let doc = Document::parse(&text)?;
    if doc.header("format").as_deref() == Some("binary") {
        h.format = Format::Binary;
    }
    if let Some(a) = doc.header("arch") {
        h.arch = Arch::parse(&a)?;
    }
    h.class = doc.header("class");
    h.object = doc.header("object");
    h.location = doc.header("location");
    h.note = doc.header("note");
    Ok((h, Some(start + close + 1)))
}

impl Field {
    pub fn parse(bytes: &[u8]) -> Result<Field> {
        Field::parse_with(bytes, &Limits::default())
    }

    /// Like [`Field::parse`] with explicit limits. Gzip data is detected and decompressed.
    pub fn parse_with(bytes: &[u8], limits: &Limits) -> Result<Field> {
        if bytes.starts_with(&GZIP_MAGIC) {
            let raw = gunzip(bytes, limits.max_bytes)?;
            return Field::parse_plain(&raw, limits);
        }
        Field::parse_plain(bytes, limits)
    }

    fn parse_plain(bytes: &[u8], limits: &Limits) -> Result<Field> {
        let header = read_header(bytes)?;
        let scanned = scan::scan(bytes, header.format == Format::Binary, header.arch, limits)?;
        let doc = Document::parse_bytes(&scanned.text)?;
        scanned.tag.check(&doc, scanned.lists.len())?;
        Ok(Field { header, doc, lists: scanned.lists, tag: scanned.tag })
    }

    /// Reads `rel` from a case, or `rel.gz` when only the compressed file exists.
    pub fn read(fs: &dyn umcf_core::CaseFs, rel: &str, limits: &Limits) -> Result<Option<Field>> {
        for path in [rel.to_string(), format!("{rel}.gz")] {
            if let Some(bytes) = fs.read(&path, limits.max_bytes)? {
                return Field::parse_with(&bytes, limits).map(Some);
            }
        }
        Ok(None)
    }

    pub fn class(&self) -> Option<&str> {
        self.header.class.as_deref()
    }

    pub fn dimensions(&self) -> Option<String> {
        self.doc.value(&["dimensions"])
    }

    pub fn internal(&self) -> Option<Value<'_>> {
        self.value(&["internalField"])
    }

    /// Patch names of `boundaryField`, in file order (regex keys like `"(wall|ground).*"`
    /// included as written).
    pub fn patches(&self) -> Vec<&str> {
        self.doc.get(&["boundaryField"]).and_then(Entry::dict).map(|d| d.keys().collect()).unwrap_or_default()
    }

    pub fn patch_type(&self, patch: &str) -> Option<String> {
        self.doc.value(&["boundaryField", patch, "type"])
    }

    /// A patch entry such as `value`, `refValue` or `gradient`.
    pub fn patch_value(&self, patch: &str, key: &str) -> Option<Value<'_>> {
        self.value(&["boundaryField", patch, key])
    }

    /// Any entry, read as a field value.
    pub fn value(&self, path: &[&str]) -> Option<Value<'_>> {
        let e = self.doc.get(path)?;
        Some(self.interpret(e))
    }

    fn interpret(&self, e: &Entry) -> Value<'_> {
        match e.value.as_slice() {
            [Node::Atom(t), v] if t.text == "uniform" => match numbers(v) {
                Some(n) => Value::Uniform(n),
                None => Value::Other(e.value_text().unwrap_or_default()),
            },
            [Node::Atom(t), Node::Atom(p)] if t.text == "nonuniform" => match self.placeholder_list(&p.text) {
                Some(l) => Value::List(l),
                None => Value::Other(e.value_text().unwrap_or_default()),
            },
            // A bare typed list, as in zone files: `cellLabels List<label> 3(…);`.
            [Node::Atom(p)] => match self.placeholder_list(&p.text) {
                Some(l) => Value::List(l),
                None => Value::Other(e.value_text().unwrap_or_default()),
            },
            _ => Value::Other(e.value_text().unwrap_or_default()),
        }
    }

    /// The list a placeholder word in [`Field::doc`] stands for (for lists the dictionary paths of
    /// [`Field::value`] can't reach, such as those inside the zone list of `cellZones`).
    pub fn placeholder_list(&self, word: &str) -> Option<&ListData> {
        self.tag.index(word).and_then(|k| self.lists.get(k))
    }

    /// Every list in the file, in file order.
    pub fn lists(&self) -> &[ListData] {
        &self.lists
    }
}

/// The text every placeholder of a list starts with while a file is edited: an edit must not
/// write it (see [`rewrite`]).
pub const PLACEHOLDER: &str = "__umcfList";

/// Rewrites a field file with `edit` applied to its dictionary (a boundary condition changed,
/// say), keeping every byte the edit doesn't touch: the lists go back exactly as they were
/// (ASCII or binary). A file whose text wouldn't come back byte for byte unedited is refused,
/// so an edit can never change it anywhere else. A gzip file is compared and edited
/// decompressed and compressed again: its text is kept byte for byte, its compressed bytes
/// generally not.
pub fn rewrite(bytes: &[u8], limits: &Limits, edit: impl FnOnce(&mut Document) -> Result<()>) -> Result<Vec<u8>> {
    if bytes.starts_with(&GZIP_MAGIC) {
        let raw = gunzip(bytes, limits.max_bytes)?;
        let out = rewrite_plain(&raw, limits, edit)?;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&out).map_err(|e| Error::Gzip(e.to_string()))?;
        return gz.finish().map_err(|e| Error::Gzip(e.to_string()));
    }
    rewrite_plain(bytes, limits, edit)
}

fn rewrite_plain(bytes: &[u8], limits: &Limits, edit: impl FnOnce(&mut Document) -> Result<()>) -> Result<Vec<u8>> {
    let header = read_header(bytes)?;
    let scanned = scan::scan(bytes, header.format == Format::Binary, header.arch, limits)?;
    let mut doc = Document::parse_bytes(&scanned.text)?;
    scanned.tag.check(&doc, scanned.lists.len())?;
    if splice(bytes, &doc.to_string(), &scanned)? != bytes {
        return Err(Error::Unsupported("a file that can't be written back unchanged, so it isn't edited".to_string()));
    }
    edit(&mut doc)?;
    splice(bytes, &doc.to_string(), &scanned)
}

/// The dictionary text with each placeholder replaced by its list's bytes from `input`.
fn splice(input: &[u8], text: &str, scanned: &scan::Scanned) -> Result<Vec<u8>> {
    let text = text.as_bytes();
    let prefix = scanned.tag.prefix().as_bytes();
    let mut out = Vec::with_capacity(input.len());
    let mut used = vec![false; scanned.spans.len()];
    let mut i = 0;
    while let Some(at) = scan::find(text, i, prefix) {
        let digits = text.get(at + prefix.len()..).map_or(0, |r| r.iter().take_while(|b| b.is_ascii_digit()).count());
        let word = text.get(at..at + prefix.len() + digits).and_then(|w| std::str::from_utf8(w).ok()).unwrap_or("");
        let bad = || Error::Unsupported(format!("`{word}` in a value: the placeholder of a list"));
        let k = scanned.tag.index(word).ok_or_else(bad)?;
        let span = scanned.spans.get(k).copied().ok_or_else(bad)?;
        match used.get_mut(k) {
            Some(u) if !*u => *u = true,
            _ => return Err(bad()),
        }
        let before = text.get(i..at).unwrap_or(&[]);
        // After `nonuniform` the scanner wrote one space for the whitespace that was there.
        let (before, from) = match before.strip_suffix(b" ") {
            Some(b) if span.lead < span.start => (b, span.lead),
            _ => (before, span.start),
        };
        out.extend_from_slice(before);
        out.extend_from_slice(input.get(from..span.end).ok_or_else(bad)?);
        // And a space after the list.
        i = at + word.len();
        if text.get(i) == Some(&b' ') {
            i += 1;
        }
    }
    out.extend_from_slice(text.get(i..).unwrap_or(&[]));
    Ok(out)
}

fn numbers(n: &Node) -> Option<Vec<f64>> {
    match n {
        Node::Atom(t) => t.text.parse().ok().map(|v| vec![v]),
        Node::List { items, .. } => items
            .iter()
            .map(|i| match i {
                Node::Atom(t) => t.text.parse().ok(),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
