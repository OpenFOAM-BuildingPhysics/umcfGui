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
//!
//! ASCII `points` (both readers) and `faces` ([`BoundaryMesh::read`]) are parsed as they are read
//! through [`CaseFs::reader`], compressed or not, so the file is never held, nor the text of the
//! points or the internal faces. A file the stream doesn't take (no header, a comment among the
//! internal faces, a uniform list), or one it finds malformed, is read again whole, and that
//! reader makes the error.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod lists;
mod stream;

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

/// The cell zones of the mesh in `dir` (`cellZones`, or `cellZones.gz`), without the rest of
/// the mesh; `None` when it has none.
pub fn read_cell_zones(fs: &dyn CaseFs, dir: &str, limits: &Limits) -> Result<Option<Vec<Zone>>> {
    match read_file(fs, dir, "cellZones", limits)? {
        Some((path, bytes)) => Ok(Some(parse_zones(&path, &bytes, limits)?)),
        None => Ok(None),
    }
}

/// The bytes of `dir/name` or `dir/name.gz`, decompressed; an error when neither exists.
fn require(fs: &dyn CaseFs, dir: &str, name: &str, limits: &Limits) -> Result<(String, Vec<u8>)> {
    read_file(fs, dir, name, limits)?.ok_or_else(|| Error::Missing { file: format!("{dir}/{name}") })
}

/// `dir/name` read as a stream by `read` (see `stream`). `None` when it isn't a file the stream
/// takes, and also when the stream finds it malformed: the whole-file reader then reads it again
/// and makes the error, so a file reads the same either way.
fn streamed<T>(
    fs: &dyn CaseFs,
    dir: &str,
    name: &str,
    limits: &Limits,
    read: impl FnOnce(&mut stream::Stream) -> Result<Option<T>>,
) -> Result<Option<T>> {
    let mut s =
        stream::Stream::open(fs, dir, name, limits)?.ok_or_else(|| Error::Missing { file: format!("{dir}/{name}") })?;
    match read(&mut s) {
        Ok(found) => Ok(found),
        Err(Error::Bad { .. } | Error::Truncated { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The mesh's points: an ASCII file parsed as it is read, so that neither the file nor its text
/// is held whole; a binary one read whole.
fn read_points(fs: &dyn CaseFs, dir: &str, limits: &Limits) -> Result<Vec<[f64; 3]>> {
    if let Some(points) = streamed(fs, dir, "points", limits, |s| stream::vectors(s, limits))? {
        return Ok(points);
    }
    let (path, bytes) = require(fs, dir, "points", limits)?;
    open(&path, &bytes)?.1.vectors(limits)
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
        let points = || read_points(fs, dir, limits);
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

/// Whether threads start here: a probe thread, as a failed spawn would drop its job (wasm has
/// no threads).
fn threads<'s>(s: &'s std::thread::Scope<'s, '_>) -> bool {
    std::thread::Builder::new().spawn_scoped(s, || ()).is_ok_and(|h| h.join().is_ok())
}

/// What a mesh's `owner` header notes of its size, as OpenFOAM writes it
/// (`nPoints:… nCells:… nFaces:… nInternalFaces:…`); `None` for each it doesn't say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub points: Option<u64>,
    pub cells: Option<u64>,
    pub faces: Option<u64>,
    pub internal_faces: Option<u64>,
    /// Whether `owner` is written in binary (OpenFOAM writes a mesh's files in one format, its
    /// `writeFormat`); `false` without a header.
    pub binary: bool,
}

/// The counts the `owner` header of the mesh in `dir` notes (all `None` for no `owner`, or one
/// without a header). Only the file's first 64 KB are read (on disk: only those bytes),
/// compressed or not: the header is in the first few hundred. A header that can't be read, or
/// a compressed start that can't be decompressed, is an error.
pub fn counts(fs: &dyn CaseFs, dir: &str, limits: &Limits) -> Result<Counts> {
    use std::io::Read;
    const HEAD: usize = 64 << 10;
    let (path, bytes) = match fs.read_prefix(&format!("{dir}/owner"), HEAD + 1, limits.max_bytes)? {
        Some(b) => (format!("{dir}/owner"), b),
        None => match fs.read_prefix(&format!("{dir}/owner.gz"), HEAD + 1, limits.max_bytes)? {
            Some(b) => (format!("{dir}/owner.gz"), b),
            None => return Ok(Counts::default()),
        },
    };
    let head = if bytes.starts_with(&[0x1f, 0x8b]) {
        // Decompressed as far as these bytes go. A file longer than them ends early here, and
        // that alone is fine (what came out is the file's start); any other error, or an early
        // end in a file that was read whole, is an error (deflate has no checksum of its own,
        // so bytes before a failure may not be the file's).
        // One byte more than is decompressed: a file of up to 64 KB was read whole.
        let whole = bytes.len() <= HEAD;
        let mut head = Vec::new();
        let read = flate2::read::MultiGzDecoder::new(bytes.as_slice()).take(HEAD as u64).read_to_end(&mut head);
        match read {
            Ok(_) => {}
            Err(e) if !whole && e.kind() == std::io::ErrorKind::UnexpectedEof => {}
            Err(e) => return Err(Error::Field(umcf_foam_field::Error::Gzip(format!("{path}: {e}")))),
        }
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
    Ok(Counts {
        points: h.count("nPoints"),
        cells: h.count("nCells"),
        faces: h.count("nFaces"),
        internal_faces: h.count("nInternalFaces"),
        binary: end.is_some() && h.format == umcf_foam_field::Format::Binary,
    })
}

/// What drawing or picking a mesh's boundary needs: points, a face's points by the face's label
/// in the whole mesh, and the patches.
pub trait Boundary {
    /// The points the faces use, by the labels [`Boundary::face`] gives (the mesh's own labels
    /// for a [`PolyMesh`], labels into the kept points for a [`BoundaryMesh`]).
    fn points(&self) -> &[[f64; 3]];
    /// The points of face `i` (a label in the whole mesh), as labels into
    /// [`Boundary::points`]; `None` for a face this mesh doesn't hold.
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
///
/// Only the points the boundary faces use are kept, numbered in the order the faces first use
/// them: [`BoundaryMesh::face`] gives labels into these points, not the mesh's own point labels
/// (a 16-million-cell region has about ten times as many points as its boundary uses).
#[derive(Debug, Clone)]
pub struct BoundaryMesh {
    /// The points the boundary faces use, by the labels [`BoundaryMesh::face`] gives.
    pub points: Vec<[f64; 3]>,
    /// How many points the whole mesh has.
    mesh_points: usize,
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
        let counts = counts(fs, dir, limits)?;
        let points = || read_points(fs, dir, limits);
        let faces = || -> Result<FaceTail> {
            let skip = first.unwrap_or(usize::MAX);
            // An ASCII faceList is read as a stream: the internal faces are counted past and
            // never held in memory, nor is the file. Others are read whole.
            if let Some(tail) = streamed(fs, dir, "faces", limits, |s| stream::face_list_from(s, skip, limits))? {
                return Ok(tail);
            }
            let (path, bytes) = require(fs, dir, "faces", limits)?;
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
        let mesh_points = points.len();
        let mut mesh =
            BoundaryMesh { points, mesh_points, first: first.unwrap_or(n_faces), face_offsets, face_points, boundary };
        mesh.check(n_faces, counts)?;
        mesh.keep_used_points()?;
        Ok(mesh)
    }

    /// Keeps only the points the boundary faces use (checked: every label is in range), and
    /// numbers the faces' points into them.
    fn keep_used_points(&mut self) -> Result<()> {
        let mut map: Vec<u32> = vec![u32::MAX; self.points.len()];
        let mut kept: Vec<[f64; 3]> = Vec::new();
        for p in &mut self.face_points {
            let (Some(slot), Some(point)) = (map.get_mut(*p as usize), self.points.get(*p as usize)) else {
                return Err(Error::Inconsistent(format!(
                    "a face uses point {p}, the mesh has {} points",
                    self.mesh_points
                )));
            };
            if *slot == u32::MAX {
                *slot = u32::try_from(kept.len())
                    .map_err(|_| Error::Unsupported("more boundary points than 32-bit labels hold".into()))?;
                kept.push(*point);
            }
            *p = *slot;
        }
        kept.shrink_to_fit();
        self.points = kept;
        Ok(())
    }

    /// How many points the whole mesh has (more than the boundary keeps: see the type).
    pub fn n_points(&self) -> usize {
        self.mesh_points
    }

    /// The faces of the whole mesh, internal ones too.
    pub fn n_faces(&self) -> usize {
        self.first + self.face_offsets.len().saturating_sub(1)
    }

    /// The label of the first boundary face: the number of internal faces.
    pub fn first_boundary_face(&self) -> usize {
        self.first
    }

    /// The points of boundary face `i` (its label in the whole mesh), as labels into
    /// [`BoundaryMesh::points`]; `None` for an internal face.
    pub fn face(&self, i: usize) -> Option<&[u32]> {
        let k = i.checked_sub(self.first)?;
        let start = *self.face_offsets.get(k)? as usize;
        let end = *self.face_offsets.get(k + 1)? as usize;
        self.face_points.get(start..end)
    }

    /// The checks the boundary allows (see the type): `n_faces` is the faces file's count, and the
    /// counts the `owner` header notes, when it does.
    fn check(&self, n_faces: usize, counts: Counts) -> Result<()> {
        let (noted_faces, noted_internal) = (counts.faces, counts.internal_faces);
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
        let n_points = self.points.len();
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
    /// The points the boundary faces use (see [`BoundaryMesh`]).
    fn points(&self) -> &[[f64; 3]] {
        &self.points
    }

    /// The points of boundary face `i`, as labels into the kept points.
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
