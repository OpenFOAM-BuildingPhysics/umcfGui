//! OpenFOAM `polyMesh`: points, faces, owner, neighbour, boundary patches and cell zones.
//!
//! [`PolyMesh::read`] reads a mesh directory (`constant/air/polyMesh`, or one of a
//! `processorN/`), ASCII or binary, gzip-compressed or not, and checks it before anything uses
//! it: every index in range, list lengths consistent, the boundary patches covering exactly the
//! boundary faces, and `nCells` matching the `owner` header. A mesh that fails a check is an
//! error, never a panic later.
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
    pub fn faces(&self) -> std::ops::Range<usize> {
        self.start_face..self.start_face + self.n_faces
    }

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
            let threads = std::thread::Builder::new().spawn_scoped(s, || ()).is_ok_and(|h| h.join().is_ok());
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

    pub fn n_points(&self) -> usize {
        self.points.len()
    }

    pub fn n_faces(&self) -> usize {
        self.face_offsets.len().saturating_sub(1)
    }

    pub fn n_internal_faces(&self) -> usize {
        self.neighbour.len()
    }

    pub fn n_cells(&self) -> usize {
        self.n_cells
    }

    /// The point labels of face `i`.
    pub fn face(&self, i: usize) -> Option<&[u32]> {
        let start = *self.face_offsets.get(i)? as usize;
        let end = *self.face_offsets.get(i + 1)? as usize;
        self.face_points.get(start..end)
    }

    pub fn patch(&self, name: &str) -> Option<&Patch> {
        self.boundary.iter().find(|p| p.name == name)
    }

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
