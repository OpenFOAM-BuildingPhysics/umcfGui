//! OpenFOAM `polyMesh`: points, faces, owner, neighbour, boundary patches and cell zones.
//!
//! [`PolyMesh::read`] reads a mesh directory (`constant/air/polyMesh`, or one of a
//! `processorN/`), ASCII or binary, gzip-compressed or not, and checks it before anything uses
//! it: every index in range, list lengths consistent, the boundary patches covering exactly the
//! boundary faces, and `nCells` matching the `owner` header. A mesh that fails a check is an
//! error, never a panic later.
//!
//! [`BoundaryMesh::read`] reads only what the boundary needs (points, the boundary faces and
//! the patches) for drawing and picking: no `owner`, no `neighbour`, and the internal faces are
//! passed over, not kept. Both are [`Boundary`]s.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod lists;

use umcf_core::CaseFs;
use umcf_foam_dict::{Document, Node};
use umcf_foam_field::Field;

use lists::Reader;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Fs(#[from] umcf_core::FsError),
    #[error(transparent)]
    Field(#[from] umcf_foam_field::Error),
    #[error(transparent)]
    Dict(#[from] umcf_foam_dict::Error),
    #[error("{file}: no such file (also not as .gz)")]
    Missing { file: String },
    #[error("{file}, line {line}: {msg}")]
    Bad { file: String, line: usize, msg: String },
    #[error("{file}: binary data ends early")]
    Truncated { file: String },
    #[error("{what} exceeds the limit of {limit} elements")]
    TooLarge { what: &'static str, limit: usize },
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("inconsistent mesh: {0}")]
    Inconsistent(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Size of one file, and of a decompressed `.gz`.
    pub max_bytes: u64,
    /// Elements in one list.
    pub max_elements: usize,
    /// Elements of an `N{value}` list: a few bytes can ask for any size, and no real mesh file
    /// needs a big one.
    pub max_uniform: usize,
}

impl Default for Limits {
    /// Limits for real meshes: 16 GB a file, as many elements as a label holds.
    fn default() -> Self {
        Limits { max_bytes: 16 << 30, max_elements: u32::MAX as usize, max_uniform: 1 << 20 }
    }
}

/// A boundary patch from `boundary`.
#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    pub name: String,
    /// `patch`, `wall`, `empty`, `mappedWall`, `processor`, …
    pub kind: String,
    pub n_faces: usize,
    pub start_face: usize,
    pub groups: Vec<String>,
    /// Every other entry (`neighbourRegion`, `samplePatch`, `offset`, …) as compact text.
    pub params: Vec<(String, String)>,
}

impl Patch {
    /// The labels of the patch's faces.
    pub fn faces(&self) -> std::ops::Range<usize> {
        self.start_face..self.start_face + self.n_faces
    }

    /// The entry `key` of the patch (`neighbourRegion`, `samplePatch`, …), as compact text.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub name: String,
    pub cells: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct PolyMesh {
    pub points: Vec<[f64; 3]>,
    face_offsets: Vec<u32>,
    face_points: Vec<u32>,
    pub owner: Vec<u32>,
    pub neighbour: Vec<u32>,
    pub boundary: Vec<Patch>,
    pub cell_zones: Vec<Zone>,
    n_cells: usize,
}

/// What a set file holds: the class it says (`faceSet`, `cellSet`, `pointSet`) and its labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Set {
    pub class: String,
    pub labels: Vec<u32>,
}

/// The names of the sets in `<mesh dir>/sets` (`.gz` dropped), sorted.
pub fn list_sets(fs: &dyn CaseFs, dir: &str) -> Result<Vec<String>> {
    let mut names: Vec<String> = fs
        .list(&format!("{dir}/sets"))?
        .into_iter()
        .map(|n| n.strip_suffix(".gz").map(str::to_string).unwrap_or(n))
        .filter(|n| !n.starts_with('.') && !n.ends_with('~'))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// The set `name` of the mesh in `dir` (`<dir>/sets/<name>`, plain or `.gz`); `None` when it
/// isn't there. Labels are not checked against a mesh: a set can be older than the mesh.
///
/// OpenFOAM writes a set's labels as text even in a `format binary` file (a set is a hash set,
/// written label by label), so they are always read as text.
pub fn read_set(fs: &dyn CaseFs, dir: &str, name: &str, limits: &Limits) -> Result<Option<Set>> {
    let Some((path, bytes)) = read_file(fs, &format!("{dir}/sets"), name, limits)? else { return Ok(None) };
    parse_set_text(&path, &bytes, limits).map(Some)
}

/// A set file's bytes (plain or gzip-compressed), as [`read_set`] reads them; `path` names it in
/// errors.
pub fn parse_set(path: &str, bytes: &[u8], limits: &Limits) -> Result<Set> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return parse_set_text(path, &umcf_foam_field::gunzip(bytes, limits.max_bytes)?, limits);
    }
    parse_set_text(path, bytes, limits)
}

/// A set file's text: its class and its labels.
fn parse_set_text(path: &str, bytes: &[u8], limits: &Limits) -> Result<Set> {
    let (mut header, end) = umcf_foam_field::read_header_end(bytes)?;
    header.format = umcf_foam_field::Format::Ascii;
    let mut r = Reader::new(path, bytes, end.unwrap_or(0), &header);
    r.skip_trivia();
    let class = header.class.clone().unwrap_or_default();
    Ok(Set { class, labels: r.labels(limits)? })
}

/// The mesh directory of a region (`""` for a single-region case).
pub fn mesh_dir(region: &str) -> String {
    if region.is_empty() { "constant/polyMesh".to_string() } else { format!("constant/{region}/polyMesh") }
}

/// Only the patches of a mesh (`<dir>/boundary`): what a case editor needs, without reading the
/// mesh itself. `Ok(None)` when there is no boundary file.
pub fn read_boundary(fs: &dyn CaseFs, dir: &str, limits: &Limits) -> Result<Option<Vec<Patch>>> {
    match read_file(fs, dir, "boundary", limits)? {
        Some((_, bytes)) => parse_boundary(&bytes).map(Some),
        None => Ok(None),
    }
}

/// The bytes of `dir/name` or `dir/name.gz`, decompressed; `None` if neither exists.
fn read_file(fs: &dyn CaseFs, dir: &str, name: &str, limits: &Limits) -> Result<Option<(String, Vec<u8>)>> {
    for path in [format!("{dir}/{name}"), format!("{dir}/{name}.gz")] {
        if let Some(bytes) = fs.read(&path, limits.max_bytes)? {
            let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
                umcf_foam_field::gunzip(&bytes, limits.max_bytes)?
            } else {
                bytes
            };
            return Ok(Some((path, bytes)));
        }
    }
    Ok(None)
}

/// The bytes of `dir/name` or `dir/name.gz`, decompressed; an error when neither exists.
fn require(fs: &dyn CaseFs, dir: &str, name: &str, limits: &Limits) -> Result<(String, Vec<u8>)> {
    read_file(fs, dir, name, limits)?.ok_or_else(|| Error::Missing { file: format!("{dir}/{name}") })
}

/// A list file: its header and a reader positioned at the data.
fn open<'a>(path: &'a str, bytes: &'a [u8]) -> Result<(umcf_foam_field::Header, Reader<'a>)> {
    let (header, end) = umcf_foam_field::read_header_end(bytes)?;
    let mut r = Reader::new(path, bytes, end.unwrap_or(0), &header);
    r.skip_trivia();
    Ok((header, r))
}

impl PolyMesh {
    /// Reads and checks the mesh in `dir` (`constant/air/polyMesh`): every file, the internal faces and cells too.
    pub fn read(fs: &(dyn CaseFs + Sync), dir: &str, limits: &Limits) -> Result<PolyMesh> {
        let points = || -> Result<Vec<[f64; 3]>> {
            let (path, bytes) = require(fs, dir, "points", limits)?;
            open(&path, &bytes)?.1.vectors(limits)
        };
        let faces = || -> Result<(Vec<u32>, Vec<u32>)> {
            let (path, bytes) = require(fs, dir, "faces", limits)?;
            let (header, mut r) = open(&path, &bytes)?;
            match header.class.as_deref() {
                Some("faceCompactList") => r.face_compact_list(limits),
                _ => r.face_list(limits),
            }
        };
        let owner = || -> Result<(Option<u64>, Vec<u32>)> {
            let (path, bytes) = require(fs, dir, "owner", limits)?;
            let (header, mut r) = open(&path, &bytes)?;
            Ok((header.count("nCells"), r.labels(limits)?))
        };
        let neighbour = || -> Result<Vec<u32>> {
            let (path, bytes) = require(fs, dir, "neighbour", limits)?;
            open(&path, &bytes)?.1.labels(limits)
        };
        // The four big files are independent: read them in parallel where threads exist, one
        // after the other where they don't (wasm). A probe thread tells which: a failed spawn
        // drops its job, so the real jobs only go to threads once one has run.
        let (points, faces, owner, neighbour) = std::thread::scope(|s| -> Result<_> {
            let threads = threads(s);
            if !threads {
                return Ok((points()?, faces()?, owner()?, neighbour()?));
            }
            let started = |e: std::io::Error| Error::Unsupported(format!("could not start a reader thread: {e}"));
            let joined = || Error::Inconsistent("a mesh reader thread failed".into());
            let p = std::thread::Builder::new().spawn_scoped(s, points).map_err(started)?;
            let o = std::thread::Builder::new().spawn_scoped(s, owner).map_err(started)?;
            let n = std::thread::Builder::new().spawn_scoped(s, neighbour).map_err(started)?;
            let f = faces();
            let p = p.join().map_err(|_| joined())?;
            let o = o.join().map_err(|_| joined())?;
            let n = n.join().map_err(|_| joined())?;
            Ok((p?, f?, o?, n?))
        })?;
        let (n_cells_note, owner) = owner;

        let (_, bytes) = require(fs, dir, "boundary", limits)?;
        let boundary = parse_boundary(&bytes)?;

        let cell_zones = match read_file(fs, dir, "cellZones", limits)? {
            Some((path, bytes)) => parse_zones(&path, &bytes, limits)?,
            None => Vec::new(),
        };

        let (face_offsets, face_points) = faces;
        let mut mesh =
            PolyMesh { points, face_offsets, face_points, owner, neighbour, boundary, cell_zones, n_cells: 0 };
        mesh.n_cells = mesh.check(n_cells_note)?;
        Ok(mesh)
    }

    /// How many points the mesh has.
    pub fn n_points(&self) -> usize {
        self.points.len()
    }

    /// How many faces the mesh has.
    pub fn n_faces(&self) -> usize {
        self.face_offsets.len().saturating_sub(1)
    }

    /// How many faces lie between two cells.
    pub fn n_internal_faces(&self) -> usize {
        self.neighbour.len()
    }

    /// How many cells the mesh has.
    pub fn n_cells(&self) -> usize {
        self.n_cells
    }

    /// The point labels of face `i`.
    pub fn face(&self, i: usize) -> Option<&[u32]> {
        let start = *self.face_offsets.get(i)? as usize;
        let end = *self.face_offsets.get(i + 1)? as usize;
        self.face_points.get(start..end)
    }

    /// The patch called `name`.
    pub fn patch(&self, name: &str) -> Option<&Patch> {
        self.boundary.iter().find(|p| p.name == name)
    }

    /// The cell zone called `name`.
    pub fn cell_zone(&self, name: &str) -> Option<&Zone> {
        self.cell_zones.iter().find(|z| z.name == name)
    }

    /// Smallest and largest coordinates.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let first = *self.points.first()?;
        Some(self.points.iter().fold((first, first), |(mut lo, mut hi), p| {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            (lo, hi)
        }))
    }

    /// Every consistency check; returns the cell count.
    fn check(&self, n_cells_note: Option<u64>) -> Result<usize> {
        let bad = |m: String| Err(Error::Inconsistent(m));
        let (n_faces, n_points) = (self.n_faces(), self.n_points());
        if self.owner.len() != n_faces {
            return bad(format!("{} faces but {} owners", n_faces, self.owner.len()));
        }
        if self.neighbour.len() > n_faces {
            return bad(format!("{} neighbours for {} faces", self.neighbour.len(), n_faces));
        }
        if let Some(p) = self.face_points.iter().find(|p| **p as usize >= n_points) {
            return bad(format!("a face uses point {p}, the mesh has {n_points} points"));
        }
        if let Some(i) = (0..n_faces).find(|i| self.face(*i).is_none_or(|f| f.len() < 3)) {
            return bad(format!("face {i} has fewer than 3 points"));
        }
        // Checked: a label of u32::MAX would overflow, and wrap to 0 cells in release builds.
        let max_cell = match self.owner.iter().chain(&self.neighbour).max() {
            None => 0,
            Some(m) => match m.checked_add(1) {
                Some(n) => n as usize,
                None => return bad(format!("cell label {m} is out of range")),
            },
        };
        // Every cell has a face: more cells than face sides means unused labels (a hostile
        // `4294967294` would ask callers for billions of cells).
        if max_cell > self.owner.len() + self.neighbour.len() {
            return bad(format!(
                "the faces reference cell {} but have only {} sides: cells without faces",
                max_cell - 1,
                self.owner.len() + self.neighbour.len()
            ));
        }
        if let Some(n) = n_cells_note
            && n != max_cell as u64
        {
            return bad(format!("the owner header says {n} cells, the faces reference {max_cell}"));
        }
        let mut next = self.n_internal_faces();
        let mut patches: Vec<&Patch> = self.boundary.iter().collect();
        patches.sort_by_key(|p| p.start_face);
        for p in patches {
            if p.start_face != next {
                return bad(format!("patch {} starts at face {}, expected {next}", p.name, p.start_face));
            }
            next = match p.start_face.checked_add(p.n_faces) {
                Some(n) if n <= n_faces => n,
                _ => return bad(format!("patch {} runs past the last face", p.name)),
            };
        }
        if next != n_faces {
            return bad(format!("the patches cover faces up to {next}, the mesh has {n_faces}"));
        }
        for z in &self.cell_zones {
            if let Some(c) = z.cells.iter().find(|c| **c as usize >= max_cell) {
                return bad(format!("cell zone {} uses cell {c}, the mesh has {max_cell} cells", z.name));
            }
        }
        Ok(max_cell)
    }
}

/// The faces of a mesh from one face on: the number of all faces, then compact offsets and
/// point labels of the faces kept.
type FaceTail = (usize, Vec<u32>, Vec<u32>);

/// A gzip-compressed ASCII `faceList` from face `skip` on, as `Reader::face_list_from`,
/// decompressed as a stream: the text of the faces before `skip` is passed over as it comes and
/// never kept. `Ok(None)` when the file is not an ASCII `faceList`, or its list doesn't start
/// within the first few megabytes (then it is read whole).
fn gz_face_list_from(path: &str, gz: &[u8], skip: usize, limits: &Limits) -> Result<Option<FaceTail>> {
    use std::io::Read;
    /// How far into the file the header and the list's `N (` may be.
    const PREAMBLE: usize = 4 << 20;
    let mut decoder = flate2::read::MultiGzDecoder::new(gz);
    let mut chunk = vec![0u8; 1 << 20];
    // Every decompressed byte is counted here, against the limit.
    let mut seen: u64 = 0;
    let mut fill = |buf: &mut Vec<u8>| -> Result<bool> {
        let n = decoder.read(&mut chunk).map_err(|e| Error::Field(umcf_foam_field::Error::Gzip(e.to_string())))?;
        seen = seen.saturating_add(n as u64);
        if seen > limits.max_bytes {
            return Err(Error::Field(umcf_foam_field::Error::TooLarge {
                what: "decompressed file",
                limit: limits.max_bytes,
            }));
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
        Ok(n > 0)
    };
    // The header, then `N (`.
    let mut buf = Vec::new();
    let (header, end) = loop {
        let more = fill(&mut buf)?;
        if let Ok((h, Some(end))) = umcf_foam_field::read_header_end(&buf) {
            break (h, end);
        }
        if !more || buf.len() > PREAMBLE {
            return Ok(None);
        }
    };
    if header.format != umcf_foam_field::Format::Ascii || !matches!(header.class.as_deref(), Some("faceList") | None) {
        return Ok(None);
    }
    let (n, open) = loop {
        match list_start(buf.get(end..).unwrap_or_default(), limits)? {
            Start::Found(n, at) => break (n, end + at),
            Start::NotHere => return Ok(None),
            Start::NeedMore => {
                if buf.len() > PREAMBLE || !fill(&mut buf)? {
                    return Ok(None);
                }
            }
        }
    };
    // Each face is `k(a b c …)`: the faces before `skip` end at the first `skip` `)`s. The lines
    // passed over are counted, so that an error names the file's own line.
    let skip = skip.min(n);
    // The newlines before the list's `(`, then those passed with the faces.
    let mut lines = buf.get(..open).map_or(0, |h| h.iter().filter(|c| **c == b'\n').count());
    let mut passed = 0usize;
    let mut at = open;
    loop {
        if passed == skip {
            break;
        }
        let rest = buf.get(at..).unwrap_or_default();
        let mut found = None;
        for (k, c) in rest.iter().enumerate() {
            match c {
                b'\n' => lines += 1,
                b')' => {
                    passed += 1;
                    if passed == skip {
                        found = Some(k + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(k) = found {
            at += k;
            continue;
        }
        buf.clear();
        at = 0;
        if !fill(&mut buf)? {
            return Err(Error::Truncated { file: path.to_string() });
        }
    }
    // The rest as a list of its own, `n - skip (` and the faces from `skip` on, in one buffer.
    let mut text = format!("{}\n(", n - skip).into_bytes();
    text.extend_from_slice(buf.get(at..).unwrap_or_default());
    drop(buf);
    while fill(&mut text)? {}
    let plain = umcf_foam_field::Header { class: Some("faceList".into()), ..header };
    let mut r = Reader::new(path, &text, 0, &plain);
    match r.face_list_from(limits, 0) {
        Ok((_, offsets, points)) => Ok(Some((n, offsets, points))),
        // The re-made list's line 2 is the file's line `lines + 1`.
        Err(Error::Bad { file, line, msg }) => Err(Error::Bad { file, line: (line + lines).saturating_sub(1), msg }),
        Err(e) => Err(e),
    }
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

/// Whether threads start here: a probe thread, as a failed spawn would drop its job (wasm has
/// no threads).
fn threads<'s>(s: &'s std::thread::Scope<'s, '_>) -> bool {
    std::thread::Builder::new().spawn_scoped(s, || ()).is_ok_and(|h| h.join().is_ok())
}

/// The face counts a mesh's `owner` header notes (`nFaces`, `nInternalFaces`), as OpenFOAM writes
/// them; `None` for each it doesn't (or no `owner`, or one without a header). A compressed file is
/// decompressed only as far as its header (64 KB); an error decompressing that far, or a header
/// that can't be read, is an error. An uncompressed one is read whole.
fn owner_counts(fs: &dyn CaseFs, dir: &str, limits: &Limits) -> Result<(Option<u64>, Option<u64>)> {
    use std::io::Read;
    let (path, bytes) = match fs.read(&format!("{dir}/owner"), limits.max_bytes)? {
        Some(b) => (format!("{dir}/owner"), b),
        None => match fs.read(&format!("{dir}/owner.gz"), limits.max_bytes)? {
            Some(b) => (format!("{dir}/owner.gz"), b),
            None => return Ok((None, None)),
        },
    };
    let head = if bytes.starts_with(&[0x1f, 0x8b]) {
        // The header is in the first kilobytes. Any error decompressing them is an error: deflate
        // has no checksum of its own, so bytes before a failed one may not be the file's.
        let mut head = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes.as_slice())
            .take(64 << 10)
            .read_to_end(&mut head)
            .map_err(|e| Error::Field(umcf_foam_field::Error::Gzip(format!("{path}: {e}"))))?;
        head
    } else {
        bytes
    };
    // A header that can't be read is an error, as the whole mesh's reader makes it (a file with
    // no header at all is plain data: no counts).
    let (h, end) = umcf_foam_field::read_header_end(&head)
        .map_err(|e| Error::Inconsistent(format!("{path}: its header can't be read: {e}")))?;
    // `FoamFile {` without its `}` (the file cut short) reads as no header: here it is an error.
    if end.is_none() && head.windows(8).any(|w| w == b"FoamFile") {
        return Err(Error::Inconsistent(format!("{path}: its header can't be read: it ends before its `}}`")));
    }
    Ok((h.count("nFaces"), h.count("nInternalFaces")))
}

/// What drawing or picking a mesh's boundary needs: its points, a face's point labels by its
/// label in the whole mesh, and the patches.
pub trait Boundary {
    /// The mesh's points, by label.
    fn points(&self) -> &[[f64; 3]];
    /// The point labels of face `i` (a label in the whole mesh); `None` for a face this mesh
    /// doesn't hold.
    fn face(&self, i: usize) -> Option<&[u32]>;
    /// The boundary patches, as `boundary` lists them.
    fn patches(&self) -> &[Patch];
}

impl Boundary for PolyMesh {
    /// The mesh's points, by label.
    fn points(&self) -> &[[f64; 3]] {
        &self.points
    }

    /// The point labels of face `i`.
    fn face(&self, i: usize) -> Option<&[u32]> {
        PolyMesh::face(self, i)
    }

    /// The boundary patches.
    fn patches(&self) -> &[Patch] {
        &self.boundary
    }
}

/// A mesh's boundary: its points, its boundary faces and its patches, read without `neighbour`,
/// the internal faces or `owner` (only its header). It is checked as far as it goes: the patches
/// cover the faces from the first boundary face to the last, where the `owner` header says the
/// internal faces end, every face has 3 points or more, every point label is in range.
#[derive(Debug, Clone)]
pub struct BoundaryMesh {
    pub points: Vec<[f64; 3]>,
    /// The label of the first boundary face (the mesh's internal faces come before it).
    first: usize,
    /// Offsets and point labels of faces `first..`.
    face_offsets: Vec<u32>,
    face_points: Vec<u32>,
    pub boundary: Vec<Patch>,
}

impl BoundaryMesh {
    /// Reads and checks the boundary of the mesh in `dir`: its patches, points and boundary faces.
    pub fn read(fs: &(dyn CaseFs + Sync), dir: &str, limits: &Limits) -> Result<BoundaryMesh> {
        let (_, bytes) = require(fs, dir, "boundary", limits)?;
        let boundary = parse_boundary(&bytes)?;
        // The patches start where the internal faces end; none: no boundary faces.
        let first = boundary.iter().map(|p| p.start_face).min();
        // What the mesh says of its faces, to check the boundary against.
        let counts = owner_counts(fs, dir, limits)?;
        let points = || -> Result<Vec<[f64; 3]>> {
            let (path, bytes) = require(fs, dir, "points", limits)?;
            open(&path, &bytes)?.1.vectors(limits)
        };
        let faces = || -> Result<FaceTail> {
            let skip = first.unwrap_or(usize::MAX);
            // As `read_file` picks the file: `faces`, else `faces.gz`. A compressed ASCII
            // faceList is decompressed as a stream: the internal faces are counted past and
            // never held, decompressed, in memory.
            let (path, bytes) = match fs.read(&format!("{dir}/faces"), limits.max_bytes)? {
                Some(bytes) => (format!("{dir}/faces"), bytes),
                None => {
                    let gz = format!("{dir}/faces.gz");
                    let bytes = fs
                        .read(&gz, limits.max_bytes)?
                        .ok_or_else(|| Error::Missing { file: format!("{dir}/faces") })?;
                    if bytes.starts_with(&[0x1f, 0x8b])
                        && let Some(tail) = gz_face_list_from(&gz, &bytes, skip, limits)?
                    {
                        return Ok(tail);
                    }
                    (gz, bytes)
                }
            };
            let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
                umcf_foam_field::gunzip(&bytes, limits.max_bytes)?
            } else {
                bytes
            };
            let (header, mut r) = open(&path, &bytes)?;
            match header.class.as_deref() {
                Some("faceCompactList") => r.face_compact_list_from(limits, skip),
                _ => r.face_list_from(limits, skip),
            }
        };
        // Points and faces in parallel where threads exist (see PolyMesh::read).
        let (points, faces) = std::thread::scope(|s| -> Result<_> {
            let threads = threads(s);
            if !threads {
                return Ok((points()?, faces()?));
            }
            let started = |e: std::io::Error| Error::Unsupported(format!("could not start a reader thread: {e}"));
            let p = std::thread::Builder::new().spawn_scoped(s, points).map_err(started)?;
            let f = faces();
            let p = p.join().map_err(|_| Error::Inconsistent("a mesh reader thread failed".into()))?;
            Ok((p?, f?))
        })?;
        let (n_faces, face_offsets, face_points) = faces;
        let mesh = BoundaryMesh { points, first: first.unwrap_or(n_faces), face_offsets, face_points, boundary };
        mesh.check(n_faces, counts)?;
        Ok(mesh)
    }

    /// How many points the mesh has.
    pub fn n_points(&self) -> usize {
        self.points.len()
    }

    /// The faces of the whole mesh, internal ones too.
    pub fn n_faces(&self) -> usize {
        self.first + self.face_offsets.len().saturating_sub(1)
    }

    /// The label of the first boundary face: the number of internal faces.
    pub fn first_boundary_face(&self) -> usize {
        self.first
    }

    /// The point labels of boundary face `i` (its label in the whole mesh); `None` for an
    /// internal face.
    pub fn face(&self, i: usize) -> Option<&[u32]> {
        let k = i.checked_sub(self.first)?;
        let start = *self.face_offsets.get(k)? as usize;
        let end = *self.face_offsets.get(k + 1)? as usize;
        self.face_points.get(start..end)
    }

    /// The checks the boundary allows (see the type): `n_faces` is the faces file's count, and the
    /// counts the `owner` header notes, when it does.
    fn check(&self, n_faces: usize, (noted_faces, noted_internal): (Option<u64>, Option<u64>)) -> Result<()> {
        let bad = |m: String| Err(Error::Inconsistent(m));
        // A `boundary` left from another mesh: its patches don't start where this mesh's internal
        // faces end, or the faces aren't as many as the mesh says.
        if let Some(n) = noted_faces
            && n != n_faces as u64
        {
            return bad(format!("the owner header says {n} faces, the faces file has {n_faces}"));
        }
        // Without patches the first boundary face is the last face: the owner must say there
        // are no boundary faces either.
        if let Some(n) = noted_internal
            && n != self.first as u64
        {
            return bad(format!(
                "the patches start at face {}, but the owner header says the mesh has {n} internal faces: is `boundary` from another mesh?",
                self.first
            ));
        }
        if self.first > n_faces {
            return bad(format!("the patches start at face {}, the mesh has {n_faces}", self.first));
        }
        let n_points = self.n_points();
        if let Some(p) = self.face_points.iter().find(|p| **p as usize >= n_points) {
            return bad(format!("a face uses point {p}, the mesh has {n_points} points"));
        }
        if let Some(i) = (self.first..n_faces).find(|i| self.face(*i).is_none_or(|f| f.len() < 3)) {
            return bad(format!("face {i} has fewer than 3 points"));
        }
        let mut next = self.first;
        let mut patches: Vec<&Patch> = self.boundary.iter().collect();
        patches.sort_by_key(|p| p.start_face);
        for p in patches {
            if p.start_face != next {
                return bad(format!("patch {} starts at face {}, expected {next}", p.name, p.start_face));
            }
            next = match p.start_face.checked_add(p.n_faces) {
                Some(n) if n <= n_faces => n,
                _ => return bad(format!("patch {} runs past the last face", p.name)),
            };
        }
        if next != n_faces {
            return bad(format!("the patches cover faces up to {next}, the mesh has {n_faces}"));
        }
        Ok(())
    }
}

impl Boundary for BoundaryMesh {
    /// The mesh's points, by label.
    fn points(&self) -> &[[f64; 3]] {
        &self.points
    }

    /// The point labels of face `i`.
    fn face(&self, i: usize) -> Option<&[u32]> {
        BoundaryMesh::face(self, i)
    }

    /// The boundary patches.
    fn patches(&self) -> &[Patch] {
        &self.boundary
    }
}

/// `N ( name { … } … )`: the list, whether or not the count parses as a keyword.
fn entry_list(doc: &Document) -> Vec<&Node> {
    doc.root
        .values()
        .chain(doc.root.entries().filter_map(|e| match e.value.as_slice() {
            [list @ Node::List { .. }] => Some(list),
            _ => None,
        }))
        .collect()
}

/// The name token and the dictionary of patch `name` in a `boundary` file's document, to edit.
pub fn boundary_patch_mut<'a>(
    doc: &'a mut Document,
    name: &str,
) -> Option<(&'a mut umcf_foam_dict::Tok, &'a mut umcf_foam_dict::Dict)> {
    use umcf_foam_dict::Item;
    for item in &mut doc.root.items {
        let list = match item {
            Item::Value(n @ Node::List { .. }) => n,
            Item::Entry(e) => match e.value.as_mut_slice() {
                [n @ Node::List { .. }] => n,
                _ => continue,
            },
            _ => continue,
        };
        let Node::List { items, .. } = list else { continue };
        let at = items.windows(2).position(|w| matches!(w, [Node::Atom(t), Node::Dict { .. }] if t.text == name));
        if let Some(i) = at
            && let [Node::Atom(t), Node::Dict { body, .. }, ..] = items.get_mut(i..).unwrap_or_default()
        {
            return Some((t, body));
        }
    }
    None
}

/// The patches of a `boundary` file's bytes, as [`read_boundary`] reads them; a `.gz` may
/// decompress to `limits.max_bytes`.
pub fn parse_boundary_bytes(bytes: &[u8], limits: &Limits) -> Result<Vec<Patch>> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return parse_boundary(&umcf_foam_field::gunzip(bytes, limits.max_bytes)?);
    }
    parse_boundary(bytes)
}

fn named_dicts(doc: &Document) -> Vec<(&str, &umcf_foam_dict::Dict)> {
    let mut out = Vec::new();
    for list in entry_list(doc) {
        let Node::List { items, .. } = list else { continue };
        let mut it = items.iter();
        while let (Some(Node::Atom(name)), Some(Node::Dict { body, .. })) = (it.next(), it.next()) {
            out.push((name.text.as_str(), body));
        }
    }
    out
}

fn parse_boundary(bytes: &[u8]) -> Result<Vec<Patch>> {
    let doc = Document::parse_bytes(bytes)?;
    let mut out = Vec::new();
    for (name, body) in named_dicts(&doc) {
        let num = |k: &str| -> Result<usize> {
            body.get(k)
                .and_then(|e| e.words())
                .and_then(|w| w.first().and_then(|s| s.parse().ok()))
                .ok_or_else(|| Error::Inconsistent(format!("boundary: patch {name} has no valid {k}")))
        };
        let groups = body
            .get("inGroups")
            .map(|e| {
                e.value
                    .iter()
                    .flat_map(|n| match n {
                        Node::List { items, .. } => items
                            .iter()
                            .filter_map(|i| match i {
                                Node::Atom(t) => Some(t.text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>(),
                        _ => Vec::new(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let params = body
            .entries()
            .filter(|e| !matches!(e.key(), "type" | "nFaces" | "startFace" | "inGroups"))
            .map(|e| (e.key().to_string(), e.value_text().unwrap_or_default()))
            .collect();
        out.push(Patch {
            name: name.to_string(),
            kind: body.get("type").and_then(|e| e.value_text()).unwrap_or_default(),
            n_faces: num("nFaces")?,
            start_face: num("startFace")?,
            groups,
            params,
        });
    }
    Ok(out)
}

fn parse_zones(path: &str, bytes: &[u8], limits: &Limits) -> Result<Vec<Zone>> {
    // The field reader splices `cellLabels List<label> N (…)` out into numbers, under the mesh's
    // limits rather than the field defaults.
    let field_limits = umcf_foam_field::Limits { max_bytes: limits.max_bytes, max_values: limits.max_elements };
    let f = Field::parse_with(bytes, &field_limits)?;
    let bad = |msg: String| Error::Bad { file: path.to_string(), line: 0, msg };
    let mut out = Vec::new();
    // Labels expanded from `N{v}` across every zone: many small repeated lists add up.
    let mut repeated_total = 0usize;
    for (name, body) in named_dicts(&f.doc) {
        let list = body
            .get("cellLabels")
            .and_then(|e| match e.value.as_slice() {
                [Node::Atom(p)] => f.placeholder_list(&p.text),
                _ => None,
            })
            .ok_or_else(|| bad(format!("zone {name} has no cellLabels list")))?;
        // `N{v}` costs a few bytes of input for any N: cap it like the owner and neighbour lists,
        // summed over the file's zones. Written-out lists are bounded by the file size.
        if list.is_repeated() {
            repeated_total = repeated_total.saturating_add(list.len());
            if repeated_total > limits.max_uniform {
                return Err(Error::TooLarge { what: "uniform list", limit: limits.max_uniform });
            }
        }
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(list.len())
            .map_err(|_| Error::TooLarge { what: "list (out of memory)", limit: list.len() })?;
        for e in list.iter() {
            let v = e.first().copied().unwrap_or(-1.0);
            if !(0.0..=f64::from(u32::MAX)).contains(&v) || v.fract() != 0.0 {
                return Err(bad(format!("zone {name}: cell label {v} is not a label")));
            }
            cells.push(v as u32);
        }
        out.push(Zone { name: name.to_string(), cells });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
