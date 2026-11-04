//! STL files: reading ASCII and binary ones without trusting the header, and writing them back.
//!
//! Which kind a file is follows from its size, not its first word: a binary STL is an 80-byte
//! header, a triangle count and 50 bytes per triangle, and its header may well start with
//! `solid` too. A file of exactly that size is binary unless all of it that is looked at is
//! text; otherwise a file starting with `solid` is ASCII.
//!
//! ASCII files may hold several `solid`s, which OpenFOAM reads as the surface's regions (named
//! patches after `snappyHexMesh`). Binary files have no names; OpenFOAM reads the attribute
//! word of each triangle as its region number, and so does [`read`].

use crate::Vec3;

/// The most characters kept of a solid's name.
pub const MAX_NAME: usize = 256;

/// How much of a file [`read`] takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most triangles read (the file is an error beyond that).
    pub max_triangles: usize,
    /// The most `solid`s in an ASCII file.
    pub max_solids: usize,
    /// The most bytes a gzip-compressed file may decompress to.
    pub max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_triangles: 10_000_000, max_solids: 100_000, max_bytes: 1 << 30 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ascii,
    Binary,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Ascii => "ASCII",
            Format::Binary => "binary",
        }
    }
}

/// One triangle as the file has it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triangle {
    pub vertices: [[f32; 3]; 3],
    /// The facet normal written in the file. OpenFOAM ignores it and uses the winding
    /// (counter-clockwise seen from outside).
    pub normal: [f32; 3],
    /// ASCII: the index of its solid in [`Stl::solids`]. Binary: the attribute word.
    pub solid: u32,
}

impl Triangle {
    pub fn points(&self) -> [Vec3; 3] {
        self.vertices.map(Vec3::from_f32)
    }

    /// Twice the area, along the normal the winding gives.
    pub fn cross(&self) -> Vec3 {
        let [a, b, c] = self.points();
        (b - a).cross(c - a)
    }

    pub fn is_finite(&self) -> bool {
        self.vertices.iter().flatten().all(|v| v.is_finite())
    }

    /// Reverses the winding (and the stored normal).
    pub fn flip(&mut self) {
        self.vertices.swap(1, 2);
        self.normal = self.normal.map(|n| -n);
    }
}

/// An STL surface.
#[derive(Debug, Clone, PartialEq)]
pub struct Stl {
    pub format: Format,
    /// ASCII: the solids' names in file order (a triangle's `solid` indexes this; names may
    /// repeat). Binary: empty.
    pub solids: Vec<String>,
    pub triangles: Vec<Triangle>,
    /// What was odd about the file but did not stop it from being read (facets skipped, bytes
    /// after the last triangle).
    pub notes: Vec<String>,
    /// Why OpenFOAM would not read the file as it is; `None` when it reads what [`read`] did.
    /// Writing the surface again ([`Stl::to_bytes`]) gives a file it reads.
    pub openfoam: Option<String>,
    /// A binary file whose header starts with `solid`: OpenFOAM takes a `.stl` file like that
    /// for ASCII and can't read it (a `.stlb` file it reads as binary regardless).
    pub header_solid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StlError {
    #[error("not an STL file: it neither starts with `solid` nor has the size of a binary STL")]
    NotStl,
    #[error("binary STL cut short: its header announces {announced} triangles, the file holds {held}")]
    Truncated { announced: u64, held: u64 },
    #[error("more than {0} triangles")]
    TooManyTriangles(usize),
    #[error("more than {0} solids")]
    TooManySolids(usize),
    #[error("gzip data that doesn't decompress: {0}")]
    Gzip(String),
    #[error("decompresses to more than {0} bytes")]
    TooLarge(u64),
}

const HEADER: usize = 80;
const RECORD: usize = 50;
/// How much of a file is looked at to tell a text file from a binary one.
const SNIFF: usize = 4096;

/// Reads an STL file's bytes, gzip-compressed or not (the bytes decide, not the name).
pub fn read(bytes: &[u8], limits: &Limits) -> Result<Stl, StlError> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes)
            .take(limits.max_bytes.saturating_add(1))
            .read_to_end(&mut out)
            .map_err(|e| StlError::Gzip(e.to_string()))?;
        if out.len() as u64 > limits.max_bytes {
            return Err(StlError::TooLarge(limits.max_bytes));
        }
        return read_plain(&out, limits, true);
    }
    read_plain(bytes, limits, false)
}

/// Compresses a file's bytes with gzip (for a surface whose name ends in `.gz`, which
/// OpenFOAM reads compressed).
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing to a Vec doesn't fail.
    let _ = z.write_all(bytes);
    z.finish().unwrap_or_default()
}

/// The name OpenFOAM gives a solid: its name without the characters a word can't have
/// (`Building 1` is `Building1`), `solid` when that leaves nothing.
pub fn foam_name(name: &str) -> String {
    let w: String =
        name.chars().filter(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '/' | '$' | ';' | '{' | '}')).collect();
    if w.is_empty() { "solid".to_string() } else { w }
}

fn read_plain(bytes: &[u8], limits: &Limits, compressed: bool) -> Result<Stl, StlError> {
    let count = bytes.get(HEADER..HEADER + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes);
    let len = bytes.len() as u64;
    let solid = starts_with_solid(bytes);
    let Some(n) = count else {
        return if solid { read_ascii(bytes, limits) } else { Err(StlError::NotStl) };
    };
    let expected = (HEADER as u64 + 4) + RECORD as u64 * u64::from(n);
    let body = len - (HEADER as u64 + 4);
    // What OpenFOAM does (triSurface::readSTL): a file whose first word starts with `solid` is
    // ASCII, any other binary, when the count fits the size (readSTLBINARY.C; not checked for a
    // compressed file).
    let foam = |n: u64| -> Option<String> {
        let data = len - HEADER as u64;
        if !compressed && (n < data / RECORD as u64 || n > data / (RECORD as u64 / 2)) {
            Some(format!("its header announces {n} triangles, which OpenFOAM doesn't accept for a file of {len} bytes"))
        } else {
            None
        }
    };
    let text = solid && is_text(bytes);
    if expected == len && !text {
        return read_binary(bytes, u64::from(n), limits, Vec::new(), foam(u64::from(n)), solid);
    }
    if text {
        return read_ascii(bytes, limits);
    }
    // A binary file that doesn't fit its count (its header may start with `solid` too).
    if n == 0 && body > 0 && body.is_multiple_of(RECORD as u64) {
        let held = body / RECORD as u64;
        let note = format!("the header announces no triangles, but the file holds {held}: they were read");
        return read_binary(bytes, held, limits, vec![note], foam(0), solid);
    }
    if expected < len && n > 0 {
        let note = format!("{} bytes after the last triangle were ignored", len - expected);
        return read_binary(bytes, u64::from(n), limits, vec![note], foam(u64::from(n)), solid);
    }
    // An ASCII file with a byte that isn't text near its start has text where a count would be:
    // its last byte makes it at least 150 million, beyond any count read. A binary one cut
    // short has a real count there.
    let plausible = usize::try_from(n).is_ok_and(|n| n <= limits.max_triangles);
    if solid && !plausible {
        return read_ascii(bytes, limits);
    }
    if expected > len && n > 0 {
        return Err(StlError::Truncated { announced: u64::from(n), held: body / RECORD as u64 });
    }
    Err(StlError::NotStl)
}

fn starts_with_solid(bytes: &[u8]) -> bool {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    bytes.get(start..start + 5).is_some_and(|w| w.eq_ignore_ascii_case(b"solid"))
}

/// Whether the start of the file is all text (a binary header padded with zeros, or binary
/// numbers after it, are not).
fn is_text(bytes: &[u8]) -> bool {
    bytes.iter().take(SNIFF).all(|b| matches!(b, b'\t' | b'\n' | b'\r' | 0x0b | 0x0c | 0x20..=0x7e))
}

fn read_binary(
    bytes: &[u8],
    n: u64,
    limits: &Limits,
    notes: Vec<String>,
    openfoam: Option<String>,
    header_solid: bool,
) -> Result<Stl, StlError> {
    let n = usize::try_from(n).map_err(|_| StlError::TooManyTriangles(limits.max_triangles))?;
    if n > limits.max_triangles {
        return Err(StlError::TooManyTriangles(limits.max_triangles));
    }
    let start = HEADER + 4;
    let end = n
        .checked_mul(RECORD)
        .and_then(|b| b.checked_add(start))
        .ok_or(StlError::TooManyTriangles(limits.max_triangles))?;
    let records = bytes.get(start..end).unwrap_or_default();
    let mut triangles = Vec::with_capacity(n);
    for r in records.as_chunks::<RECORD>().0 {
        let f = |k: usize| {
            r.get(4 * k..4 * k + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map_or(f32::NAN, f32::from_le_bytes)
        };
        let attribute = r.get(48..50).and_then(|b| <[u8; 2]>::try_from(b).ok()).map_or(0, u16::from_le_bytes);
        triangles.push(Triangle {
            normal: [f(0), f(1), f(2)],
            vertices: [[f(3), f(4), f(5)], [f(6), f(7), f(8)], [f(9), f(10), f(11)]],
            solid: u32::from(attribute),
        });
    }
    Ok(Stl { format: Format::Binary, solids: Vec::new(), triangles, notes, openfoam, header_solid })
}

/// Counts of what an ASCII file had wrong.
#[derive(Default)]
struct Odd {
    /// Facets without three vertices, or with a number that doesn't read.
    malformed: usize,
    /// Lines starting with a word that isn't STL.
    unknown: usize,
    /// Lines read here that OpenFOAM's reader rejects.
    rejected: usize,
    clipped: usize,
}

fn clip(s: &str) -> (String, bool) {
    match s.char_indices().nth(MAX_NAME) {
        Some((at, _)) => (s.get(..at).unwrap_or_default().to_string(), true),
        None => (s.to_string(), false),
    }
}

/// Three numbers, as OpenFOAM's STL reader takes them: digits, a sign, a point and an exponent
/// (not `inf` or `nan`).
fn floats(words: &mut std::str::SplitAsciiWhitespace<'_>) -> Option<[f32; 3]> {
    let mut v = [0.0f32; 3];
    for x in &mut v {
        let w = words.next()?;
        if !w.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')) {
            return None;
        }
        *x = w.parse().ok()?;
    }
    Some(v)
}

fn read_ascii(bytes: &[u8], limits: &Limits) -> Result<Stl, StlError> {
    let text = String::from_utf8_lossy(bytes);
    let mut solids: Vec<String> = Vec::new();
    let mut triangles = Vec::new();
    let mut odd = Odd::default();
    let mut current: Option<u32> = None;
    // The facet being read: its normal, vertices, and whether something in it didn't read.
    let mut facet: Option<([f32; 3], Vec<[f32; 3]>, bool)> = None;
    let new_solid = |name: &str, solids: &mut Vec<String>, odd: &mut Odd| -> Result<u32, StlError> {
        if solids.len() >= limits.max_solids {
            return Err(StlError::TooManySolids(limits.max_solids));
        }
        let (name, clipped) = clip(name);
        odd.clipped += usize::from(clipped);
        solids.push(name);
        u32::try_from(solids.len() - 1).map_err(|_| StlError::TooManySolids(limits.max_solids))
    };
    for line in text.lines() {
        let line = line.trim();
        let mut words = line.split_ascii_whitespace();
        let Some(word) = words.next() else { continue };
        let is = |k: &str| word.eq_ignore_ascii_case(k);
        // OpenFOAM's lexer takes keywords in lower or upper case, not mixed.
        if word.bytes().any(|b| b.is_ascii_lowercase()) && word.bytes().any(|b| b.is_ascii_uppercase()) {
            odd.rejected += 1;
        }
        if is("solid") {
            if facet.take().is_some() {
                odd.malformed += 1;
            }
            let name = line.get(word.len()..).unwrap_or_default().trim();
            if !name.is_ascii() {
                odd.rejected += 1;
            }
            current = Some(new_solid(name, &mut solids, &mut odd)?);
        } else if is("facet") {
            if facet.is_some() {
                odd.malformed += 1;
            }
            let normal = match words.next() {
                Some(w) if w.eq_ignore_ascii_case("normal") => match floats(&mut words) {
                    Some(n) => n,
                    // `nan nan nan` from some exporters: OpenFOAM stops there.
                    None => {
                        odd.rejected += 1;
                        [0.0; 3]
                    }
                },
                _ => [0.0; 3],
            };
            facet = Some((normal, Vec::with_capacity(3), false));
        } else if is("vertex") {
            let f = facet.get_or_insert_with(|| ([0.0; 3], Vec::with_capacity(3), false));
            match floats(&mut words) {
                Some(v) if f.1.len() < 3 => f.1.push(v),
                _ => f.2 = true,
            }
            if words.next().is_some() {
                odd.rejected += 1;
            }
        } else if is("endfacet") {
            if words.next().is_some() {
                odd.rejected += 1;
            }
            match facet.take() {
                Some((normal, v, false)) if v.len() == 3 => {
                    if triangles.len() >= limits.max_triangles {
                        return Err(StlError::TooManyTriangles(limits.max_triangles));
                    }
                    let solid = match current {
                        Some(s) => s,
                        None => *current.insert(new_solid("", &mut solids, &mut odd)?),
                    };
                    let [a, b, c] = [v.first(), v.get(1), v.get(2)].map(|p| p.copied().unwrap_or([0.0; 3]));
                    triangles.push(Triangle { vertices: [a, b, c], normal, solid });
                }
                _ => odd.malformed += 1,
            }
        } else if is("endsolid") {
            if facet.take().is_some() {
                odd.malformed += 1;
            }
            current = None;
        } else if !is("outer") && !is("endloop") && !is("color") {
            odd.unknown += 1;
        }
    }
    if facet.is_some() {
        odd.malformed += 1;
    }
    let mut notes = Vec::new();
    if odd.malformed > 0 {
        notes.push(format!(
            "{} facet(s) skipped: not three vertices, a number that doesn't read, or the file ends inside one",
            odd.malformed
        ));
    }
    if odd.unknown > 0 {
        notes.push(format!("{} line(s) that are not STL were skipped", odd.unknown));
    }
    if odd.rejected > 0 {
        notes.push(format!(
            "{} line(s) read here that OpenFOAM rejects: a facet normal that isn't three numbers (`nan`), keywords in mixed case, words after a vertex or `endfacet`, or a solid name that isn't ASCII",
            odd.rejected
        ));
    }
    if odd.clipped > 0 {
        notes.push(format!("{} solid name(s) longer than {MAX_NAME} characters were cut short", odd.clipped));
    }
    let openfoam = (odd.malformed + odd.unknown + odd.rejected > 0)
        .then(|| "OpenFOAM stops with an error at the first facet or line it can't read".to_string());
    Ok(Stl { format: Format::Ascii, solids, triangles, notes, openfoam, header_solid: false })
}

/// A number as STL text: plain where that is short, in exponent form otherwise. Either reads
/// back to the same `f32`.
fn number(v: f32) -> String {
    let a = v.abs();
    if v == 0.0 {
        "0".to_string()
    } else if (1e-4..1e7).contains(&a) {
        format!("{v}")
    } else {
        format!("{v:e}")
    }
}

/// The unit normal the winding gives (zero for a degenerate triangle).
fn unit_normal(t: &Triangle) -> [f32; 3] {
    let n = t.cross();
    let l = n.length();
    if l > 0.0 && l.is_finite() {
        let n = n * (1.0 / l);
        [n.x as f32, n.y as f32, n.z as f32]
    } else {
        [0.0; 3]
    }
}

impl Stl {
    /// Writes the surface in its own format, in a form OpenFOAM reads. The facet normals are
    /// recomputed from the winding. ASCII solids keep their order, and their names as OpenFOAM
    /// reads them ([`foam_name`], in ASCII); the triangles of a solid that was split in the
    /// file are written together, and solids without triangles are written empty. Triangles
    /// with a coordinate that is not a finite number are left out (no reader takes them).
    pub fn to_bytes(&self) -> Vec<u8> {
        match self.format {
            Format::Ascii => self.to_ascii(),
            Format::Binary => self.to_binary(),
        }
    }

    fn to_ascii(&self) -> Vec<u8> {
        use std::fmt::Write;
        let mut order: Vec<usize> = (0..self.triangles.len()).collect();
        order.sort_by_key(|&k| self.triangles.get(k).map_or(0, |t| t.solid));
        let mut out = String::with_capacity(self.triangles.len().saturating_mul(260));
        let mut open: Option<u32> = None;
        // The name OpenFOAM reads it as, in characters its reader takes.
        let name = |s: u32| {
            let raw = self.solids.get(s as usize).map_or("", String::as_str);
            let n: String = foam_name(raw).chars().filter(char::is_ascii_graphic).collect();
            if n.is_empty() { "solid".to_string() } else { n }
        };
        let mut used = vec![false; self.solids.len()];
        for t in order.iter().filter_map(|&k| self.triangles.get(k)).filter(|t| t.is_finite()) {
            if let Some(u) = used.get_mut(t.solid as usize) {
                *u = true;
            }
            if open != Some(t.solid) {
                if let Some(s) = open {
                    let _ = writeln!(out, "endsolid {}", name(s));
                }
                let _ = writeln!(out, "solid {}", name(t.solid));
                open = Some(t.solid);
            }
            let [nx, ny, nz] = unit_normal(t).map(number);
            let _ = writeln!(out, "  facet normal {nx} {ny} {nz}\n    outer loop");
            for v in t.vertices {
                let [x, y, z] = v.map(number);
                let _ = writeln!(out, "      vertex {x} {y} {z}");
            }
            out.push_str("    endloop\n  endfacet\n");
        }
        if let Some(s) = open {
            let _ = writeln!(out, "endsolid {}", name(s));
        }
        // Solids without triangles are regions too (empty patches): written empty.
        for (k, _) in used.iter().enumerate().filter(|(_, u)| !**u) {
            let n = name(u32::try_from(k).unwrap_or(u32::MAX));
            let _ = writeln!(out, "solid {n}\nendsolid {n}");
        }
        if out.is_empty() {
            out.push_str("solid solid\nendsolid solid\n");
        }
        out.into_bytes()
    }

    fn to_binary(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + 4 + RECORD * self.triangles.len());
        let mut header = b"binary STL written by umcfGui".to_vec();
        header.resize(HEADER, b' ');
        out.extend_from_slice(&header);
        let finite = self.triangles.iter().filter(|t| t.is_finite());
        let n = u32::try_from(finite.clone().count()).unwrap_or(u32::MAX);
        out.extend_from_slice(&n.to_le_bytes());
        for t in finite.take(n as usize) {
            for x in unit_normal(t).iter().chain(t.vertices.iter().flatten()) {
                out.extend_from_slice(&x.to_le_bytes());
            }
            out.extend_from_slice(&u16::try_from(t.solid).unwrap_or(u16::MAX).to_le_bytes());
        }
        out
    }

    /// The bounding box of the finite vertices.
    pub fn bbox(&self) -> Option<crate::BBox> {
        crate::BBox::of(self.triangles.iter().flat_map(|t| t.points()))
    }
}
