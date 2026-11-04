//! The typed urbanMicroclimateFoam case.
//!
//! [`Case::open`] reads a case into what an editor works with:
//! - the layout (OpenFOAM version, case kind, regions; from `umcf-schema`);
//! - the run controls of `system/controlDict`;
//! - the initial time directory;
//! - per region, the mesh patches and the boundary conditions of every field there.
//!
//! The dictionaries of `system/`, `constant/` and each region's `system/<r>/` and
//! `constant/<r>/` are kept as lossless `foam-dict` documents. [`Case::set`] edits one entry and
//! leaves the rest of the file as it was, comments and `#include`s included. [`Case::save`]
//! writes only the files that changed, atomically, after keeping a backup of each. It first
//! checks every one of them: if any changed on disk since it was read, nothing is written.
//!
//! Fields are read, not edited, here: their boundary conditions are listed per patch.
//!
//! Files that are not dictionaries (the surfaces in `constant/triSurface`) are held as bytes
//! once a command writes them ([`Case::put_file`]); [`Case::save`] writes them with the same
//! checks and backups.
//!
//! A `Case` is cheap to clone (documents are shared until edited), so an editor can keep
//! snapshots for undo. What is on disk is shared by all clones of an opened case: restoring a
//! snapshot taken before a save shows the snapshot's text as a change, to be saved again.
//! Clones are versions of one editing session, not independent editors: whichever saves last
//! wins. Two cases opened separately do protect each other (the second save is a conflict).
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use umcf_core::{CaseFs, CaseWrite, FsError};
use umcf_foam_dict::Document;
use umcf_foam_field::times::{self, TimeDir};
use umcf_foam_field::{Field, Value};
use umcf_foam_mesh::Patch;
use umcf_schema::{CaseKind, Layout, Note, Severity};

mod controls;
pub mod script;
pub mod workflow;

pub use controls::Controls;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Layout(#[from] umcf_schema::Error),
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error("{path}: {source}")]
    Dict { path: String, source: umcf_foam_dict::Error },
    #[error("{path} is not one of the case's dictionaries")]
    NotLoaded { path: String },
    #[error("{path} changed on disk since the case was opened: nothing was saved")]
    Conflict { path: String },
    #[error("backups go under `.umcfgui/`, not to `{0}`")]
    BadBackup(String),
    #[error("`{0}` is not a file path inside the case")]
    BadFile(String),
    #[error("{0} is one of the case's dictionaries: it is edited entry by entry, not replaced")]
    IsDict(String),
    #[error("{0} already has 1000 backups")]
    BackupsFull(String),
    #[error("{path}: {source}")]
    Field { path: String, source: umcf_foam_field::Error },
    #[error("{0}")]
    Bc(String),
    #[error("saving stopped after {saved:?}: {source}")]
    Partial { saved: Vec<String>, source: Box<Error> },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Size limits for [`Case::open`]. Dictionaries are small; field files can be large, and only
/// their boundary conditions are needed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    /// A file bigger than this is not loaded as a dictionary.
    pub dict_bytes: u64,
    /// A field file bigger than this (after decompression) is listed without its boundary
    /// conditions.
    pub field_bytes: u64,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions { dict_bytes: 4 << 20, field_bytes: 256 << 20 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Fluid,
    Solid,
    Vegetation,
    /// The one region of a single-region case (a meshing step); its name is `""`.
    Single,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    /// As listed in the case (`air`, `buildings`, …); `""` for a single-region case.
    pub name: String,
    pub kind: RegionKind,
    /// The patches of `constant/<region>/polyMesh/boundary`; `None` when there is no mesh yet.
    pub patches: Option<Vec<Patch>>,
    /// The fields of the initial time directory, by name.
    pub fields: Vec<FieldInfo>,
}

impl Region {
    pub fn field(&self, name: &str) -> Option<&FieldInfo> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// One field file of the initial time: what it is and its boundary conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    pub name: String,
    /// Case-relative path (`0/air/U`, or `0/air/U.gz`).
    pub path: String,
    /// `volScalarField`, `volVectorField`, …
    pub class: Option<String>,
    pub dimensions: Option<String>,
    /// `internalField` in short: `uniform 300`, `nonuniform List<scalar> 2400`, `$T0`.
    pub internal: Option<String>,
    /// `boundaryField` entries in file order. `None` when the file was too big to read here.
    pub bcs: Option<Vec<Bc>>,
    /// `boundaryField` also takes entries from elsewhere (`#include`, `#includeEtc`,
    /// `$macro`): the list above may not be all of them.
    pub indirect: bool,
    /// The file has a `boundaryField` dictionary of its own.
    pub boundary_field: bool,
    /// `object` of the FoamFile header: the field the file holds. A copy such as `T~` or
    /// `T.bak` still says `T`.
    pub object: Option<String>,
}

/// One `boundaryField` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bc {
    /// The key without quotes: a patch name, a group, or a pattern such as `(wall|roof).*`.
    pub patch: String,
    /// The key was quoted, so OpenFOAM matches it as a regular expression.
    pub pattern: bool,
    /// `type`; `None` for an entry without one (a `$macro` or `#include` in its place).
    pub kind: Option<String>,
    /// `value` in short, when there is one.
    pub value: Option<String>,
    /// `patchType`: the patch type a condition is meant for (`fixedJump` on a `cyclic` patch),
    /// which lifts OpenFOAM's check that constraint patches use their own type.
    pub patch_type: Option<String>,
}

/// What is on disk for one dictionary, as read or last saved. Shared by every clone of a
/// case, so an undo snapshot can't make the model forget what was saved.
#[derive(Debug)]
struct OnDisk {
    bytes: Arc<Vec<u8>>,
    /// The document those bytes parsed to. A file whose document is still this one is
    /// unchanged, without comparing text.
    doc: Arc<Document>,
}

/// A file's length and hash: enough to tell whether a file on disk is still the one read,
/// without keeping a copy of a surface of hundreds of megabytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    len: usize,
    hash: u64,
}

fn fingerprint(bytes: &[u8]) -> Fingerprint {
    use std::hash::Hasher;
    // `DefaultHasher::new` has fixed keys: the same bytes hash the same within a session.
    let mut h = std::hash::DefaultHasher::new();
    h.write(bytes);
    Fingerprint { len: bytes.len(), hash: h.finish() }
}

/// The largest file that is not a dictionary [`Case::file`] and [`Case::put_file`] handle.
pub const MAX_FILE_BYTES: u64 = 1 << 30;

#[derive(Debug, Default)]
struct DiskState {
    files: BTreeMap<String, OnDisk>,
    /// Files that are not dictionaries, written through [`Case::put_file`]: as this session
    /// last saved them.
    raw: BTreeMap<String, Option<Fingerprint>>,
    /// Files that are not dictionaries, as they were on disk before this session first saved a
    /// command's version of them (`None`: there was none). A case from before that command (an
    /// undo snapshot) has them so, even after the save.
    originals: BTreeMap<String, Option<Arc<Vec<u8>>>>,
    /// `(backup dir, file)` pairs already backed up: a file is backed up once per session.
    backed_up: std::collections::BTreeSet<(String, String)>,
}

type Disk = Arc<std::sync::Mutex<DiskState>>;

fn lock(disk: &Disk) -> std::sync::MutexGuard<'_, DiskState> {
    // A panic elsewhere while holding the lock leaves the state itself intact.
    disk.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A file that is not a dictionary, as a command wrote it.
#[derive(Debug, Clone)]
struct RawFile {
    bytes: Arc<Vec<u8>>,
    fingerprint: Fingerprint,
    /// The file on disk this one was made from (`None`: there was none). Saving over that file,
    /// or over what this session last saved there, is not a conflict.
    base: Option<Fingerprint>,
}

/// What [`Case::save`] writes for one file.
enum Pending {
    Dict(Arc<Document>),
    Raw(Arc<Vec<u8>>, Fingerprint),
}

/// An opened case. See the crate documentation.
#[derive(Debug, Clone)]
pub struct Case {
    pub layout: Layout,
    pub controls: Controls,
    /// The time directory the run starts from (`startFrom` resolved); `None` when it isn't there.
    pub start: Option<TimeDir>,
    pub regions: Vec<Region>,
    /// What didn't read (files skipped, a missing start time): the case still opens.
    pub notes: Vec<Note>,
    /// `processorN` directories: the case is decomposed, and a parallel run reads its times
    /// and meshes from there.
    pub processors: usize,
    /// The case's scripts (`Allrun`, `Allprepare`, job scripts), read into steps.
    pub scripts: Vec<script::Script>,
    /// The paths in the case when it was opened (see [`Case::exists`]).
    paths: std::collections::BTreeSet<String>,
    files: BTreeMap<String, Arc<Document>>,
    /// Files that are not dictionaries, as commands wrote them.
    raw: BTreeMap<String, RawFile>,
    /// Paths that are the same file as a loaded one (a symlinked dictionary): path → that one.
    aliases: BTreeMap<String, String>,
    disk: Disk,
    options: OpenOptions,
}

pub const CONTROL_DICT: &str = "system/controlDict";

fn note(severity: Severity, path: impl Into<String>, message: impl Into<String>) -> Note {
    Note { severity, path: path.into(), message: message.into() }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// The scripts in the case directory, read into steps.
fn read_scripts(fs: &dyn CaseFs, options: OpenOptions, notes: &mut Vec<Note>) -> Result<Vec<script::Script>> {
    let mut out = Vec::new();
    for name in fs.list("")? {
        if !script::is_script_name(&name) || !fs.is_file(&name) {
            continue;
        }
        match fs.read(&name, options.dict_bytes) {
            Ok(Some(bytes)) => {
                let s = script::parse(&name, &String::from_utf8_lossy(&bytes));
                if s.truncated {
                    notes.push(note(
                        Severity::Warning,
                        &name,
                        "script read only in part (too deeply nested, or its loops run too many commands): it is checked only that far",
                    ));
                }
                out.push(s);
            }
            Ok(None) => {}
            Err(e) => notes.push(note(Severity::Info, &name, format!("script not read: {e}"))),
        }
    }
    Ok(out)
}

/// The most entries [`snapshot`] lists.
const SNAPSHOT_LIMIT: usize = 50_000;

/// The paths of the case that scripts copy, move and read: everything to a depth of five,
/// except inside `processor*`, time directories other than `0`, and meshes (a `polyMesh` lists
/// its files and `sets/`, no deeper). A directory that wasn't listed is marked
/// `<dir>/\u{1}`: what is below it is not known.
fn snapshot(fs: &dyn CaseFs) -> Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    let mut stack: Vec<(String, usize)> = vec![(String::new(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let names = match fs.list(&dir) {
            Ok(n) => n,
            // A directory that can't be listed: what is below it is not known.
            Err(_) => {
                out.insert(format!("{dir}/\u{1}"));
                continue;
            }
        };
        for n in names {
            let rel = join(&dir, &n);
            if out.len() >= SNAPSHOT_LIMIT {
                // Not everything was listed: a path not in the set is not known to be absent.
                out.insert("\u{1}".to_string());
                return Ok(out);
            }
            let is_dir = fs.is_dir(&rel);
            out.insert(rel.clone());
            if !is_dir {
                continue;
            }
            let time = n != "0" && n.bytes().next().is_some_and(|b| b.is_ascii_digit()) && n.parse::<f64>().is_ok();
            let mesh_inner = dir.ends_with("polyMesh") && n != "sets";
            if depth >= 4
                || time
                || n.starts_with("processor")
                || mesh_inner
                || n == "dynamicCode"
                || n == "postProcessing"
            {
                out.insert(format!("{rel}/\u{1}"));
            } else {
                stack.push((rel, depth + 1));
            }
        }
    }
    Ok(out)
}

/// Files that are not dictionaries of the case: hidden files, editor backups, compressed data.
fn skipped_name(name: &str) -> bool {
    name.starts_with('.') || name.ends_with('~') || name.ends_with(".gz")
}

impl Case {
    pub fn open(fs: &dyn CaseFs) -> Result<Case> {
        Case::open_with(fs, OpenOptions::default())
    }

    pub fn open_with(fs: &dyn CaseFs, options: OpenOptions) -> Result<Case> {
        let layout = umcf_schema::detect(fs)?;
        let mut notes = Vec::new();

        let mut regions: Vec<(String, RegionKind)> = Vec::new();
        regions.extend(layout.regions.fluid.iter().map(|r| (r.clone(), RegionKind::Fluid)));
        regions.extend(layout.regions.solid.iter().map(|r| (r.clone(), RegionKind::Solid)));
        regions.extend(layout.regions.vegetation.iter().map(|r| (r.clone(), RegionKind::Vegetation)));
        if regions.is_empty() && (layout.kind == CaseKind::Meshing || fs.is_dir("constant/polyMesh")) {
            regions.push((String::new(), RegionKind::Single));
        }

        // The dictionaries: every file directly in system/ and constant/, and in each region's
        // system/<r>/ and constant/<r>/.
        let mut dirs = vec!["system".to_string(), "constant".to_string()];
        for (r, _) in regions.iter().filter(|(r, _)| !r.is_empty()) {
            dirs.push(format!("system/{r}"));
            dirs.push(format!("constant/{r}"));
        }
        let mut files = BTreeMap::new();
        let mut disk = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        let mut identities: BTreeMap<std::path::PathBuf, String> = BTreeMap::new();
        for dir in &dirs {
            let mut names = fs.list(dir)?;
            // controlDict first: if another file links to it (or it to another), it is the one
            // loaded and the other is its alias.
            if dir == "system"
                && let Some(i) = names.iter().position(|n| n == "controlDict")
            {
                let c = names.remove(i);
                names.insert(0, c);
            }
            for name in names {
                let rel = join(dir, &name);
                if skipped_name(&name) || !fs.is_file(&rel) {
                    continue;
                }
                // Two paths, one file: edits through either go to one document, so a save
                // can't write the file twice with different texts.
                if let Some(id) = fs.identity(&rel) {
                    if let Some(first) = identities.get(&id) {
                        notes.push(note(Severity::Info, &rel, format!("the same file as {first}")));
                        aliases.insert(rel, first.clone());
                        continue;
                    }
                    identities.insert(id, rel.clone());
                }
                if fs.hard_links(&rel) > 1 {
                    notes.push(note(
                        Severity::Info,
                        &rel,
                        "hard-linked: saving replaces this path only, other links keep the old text",
                    ));
                }
                // controlDict is required and always loaded (layout has already read it).
                let limit = if rel == CONTROL_DICT { options.dict_bytes.max(16 << 20) } else { options.dict_bytes };
                let bytes = match fs.read(&rel, limit) {
                    Ok(Some(b)) => b,
                    Ok(None) => continue,
                    Err(FsError::TooLarge { .. }) => {
                        notes.push(note(Severity::Info, &rel, "too large for a dictionary: not loaded"));
                        continue;
                    }
                    // Only controlDict is required (and layout has read it already): any other
                    // file that can't be read (permissions, I/O) is a note.
                    Err(e) if rel != CONTROL_DICT => {
                        notes.push(note(Severity::Warning, &rel, format!("can't be read, not loaded: {e}")));
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                };
                match Document::parse_bytes(&bytes) {
                    Ok(doc) => {
                        let doc = Arc::new(doc);
                        disk.insert(rel.clone(), OnDisk { bytes: Arc::new(bytes), doc: Arc::clone(&doc) });
                        files.insert(rel, doc);
                    }
                    Err(e) => notes.push(note(Severity::Warning, &rel, format!("not a dictionary, not editable: {e}"))),
                }
            }
        }

        let controls = files.get(CONTROL_DICT).map(|d| Controls::read(d)).unwrap_or_default();
        let time_dirs = times::time_dirs(fs, "")?;
        let processors = times::processor_dirs(fs)?.len();
        let start = controls.start_dir(&time_dirs);
        if start.is_none() && layout.kind == CaseKind::Solver {
            notes.push(note(
                Severity::Warning,
                CONTROL_DICT,
                format!("the start time ({}) has no time directory", controls.describe_start()),
            ));
        }

        let mut out = Vec::new();
        for (name, kind) in regions {
            let mesh = umcf_foam_mesh::mesh_dir(&name);
            // `boundary` is a small dictionary: the dictionary cap, not the mesh one, also for
            // what a `boundary.gz` decompresses to.
            let limits = umcf_foam_mesh::Limits { max_bytes: options.dict_bytes, ..Default::default() };
            let patches = match umcf_foam_mesh::read_boundary(fs, &mesh, &limits) {
                Ok(p) => p,
                Err(e) => {
                    notes.push(note(Severity::Warning, format!("{mesh}/boundary"), e.to_string()));
                    None
                }
            };
            let fields = match &start {
                Some(t) => read_fields(
                    fs,
                    &if name.is_empty() { t.name.clone() } else { join(&t.name, &name) },
                    options,
                    &mut notes,
                )?,
                None => Vec::new(),
            };
            out.push(Region { name, kind, patches, fields });
        }

        let scripts = read_scripts(fs, options, &mut notes)?;
        let paths = snapshot(fs)?;
        let disk = Arc::new(std::sync::Mutex::new(DiskState {
            files: disk,
            raw: BTreeMap::new(),
            originals: BTreeMap::new(),
            backed_up: Default::default(),
        }));
        Ok(Case {
            layout,
            controls,
            start,
            regions: out,
            notes,
            processors,
            scripts,
            paths,
            files,
            raw: BTreeMap::new(),
            aliases,
            disk,
            options,
        })
    }

    /// Whether `rel` (a file or directory) was in the case when it was opened. Covers the case
    /// directory, `system/`, `constant/` (not mesh internals beyond `polyMesh/` itself and its
    /// `sets/`), `0/`-like directories and other non-time directories, to a depth of five;
    /// not processor or later time directories. `None` when `rel` is outside what was listed.
    pub fn exists(&self, rel: &str) -> Option<bool> {
        let rel = rel.trim_end_matches('/');
        if self.paths.contains(rel) {
            return Some(true);
        }
        // Below a listed mesh, processor or time directory: not listed, so not known.
        let mut dir = rel;
        while let Some((parent, _)) = dir.rsplit_once('/') {
            if self.paths.contains(&format!("{parent}/\u{1}")) {
                return None;
            }
            dir = parent;
        }
        let top = rel.split('/').next().unwrap_or(rel);
        if top.starts_with("processor") || self.paths.contains(&format!("{top}/\u{1}")) || self.paths.contains("\u{1}")
        {
            return None;
        }
        Some(false)
    }

    /// Whether `rel` was a directory with entries when the case was opened.
    pub fn is_dir(&self, rel: &str) -> bool {
        let prefix = format!("{}/", rel.trim_end_matches('/'));
        self.paths.range(prefix.clone()..).next().is_some_and(|p| p.starts_with(&prefix))
    }

    /// The case's workflow, read from its scripts (see [`workflow::Workflow::import`]).
    pub fn workflow(&self) -> workflow::Workflow {
        let regions: Vec<String> = self.regions.iter().map(|r| r.name.clone()).filter(|n| !n.is_empty()).collect();
        let mut wf = workflow::Workflow::import(&regions, self.controls.application.as_deref(), &self.scripts);
        for r in &regions {
            let dir = format!("constant/{r}/");
            let files: Vec<String> = self
                .files
                .keys()
                .filter_map(|p| p.strip_prefix(&dir))
                .filter(|f| !f.contains('/') && !workflow::RADIATION_OUTPUTS.contains(f) && *f != "divqrsw")
                .map(String::from)
                .collect();
            wf.constant_files.push((r.clone(), files));
        }
        wf
    }

    pub fn region(&self, name: &str) -> Option<&Region> {
        self.regions.iter().find(|r| r.name == name)
    }

    /// The loaded dictionaries, by case-relative path.
    pub fn dict_paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// The loaded path a path stands for: itself, or the file it is a link to.
    pub fn resolve<'a>(&'a self, rel: &'a str) -> &'a str {
        self.aliases.get(rel).map_or(rel, String::as_str)
    }

    /// Paths that are the same file as a loaded dictionary, with that dictionary's path.
    pub fn aliases(&self) -> impl Iterator<Item = (&str, &str)> {
        self.aliases.iter().map(|(a, p)| (a.as_str(), p.as_str()))
    }

    pub fn dict(&self, rel: &str) -> Option<&Document> {
        self.files.get(self.resolve(rel)).map(Arc::as_ref)
    }

    fn dict_mut(&mut self, rel: &str) -> Result<&mut Document> {
        let key = self.resolve(rel).to_string();
        let doc = self.files.get_mut(&key).ok_or_else(|| Error::NotLoaded { path: rel.to_string() })?;
        Ok(Arc::make_mut(doc))
    }

    fn edited(&mut self, rel: &str) {
        if self.resolve(rel) == CONTROL_DICT
            && let Some(doc) = self.files.get(CONTROL_DICT)
        {
            self.controls = Controls::read(doc);
        }
    }

    /// Sets the entry at `path` of dictionary `rel` to `value` (OpenFOAM syntax: `3600`,
    /// `(0 0 -9.81)`, `{ type fixedValue; }`), adding it if it is missing. Nothing else in the
    /// file changes.
    pub fn set(&mut self, rel: &str, path: &[&str], value: &str) -> Result<()> {
        self.dict_mut(rel)?.set(path, value).map_err(|source| Error::Dict { path: rel.to_string(), source })?;
        self.edited(rel);
        Ok(())
    }

    /// Removes the entry at `path`; `false` if there was none.
    pub fn remove(&mut self, rel: &str, path: &[&str]) -> Result<bool> {
        let removed =
            self.dict_mut(rel)?.remove(path).map_err(|source| Error::Dict { path: rel.to_string(), source })?;
        self.edited(rel);
        Ok(removed)
    }

    /// Dictionaries whose text differs from what is on disk (as read or last saved). Files
    /// never edited are skipped without serialising them.
    pub fn changed(&self) -> Vec<String> {
        let disk = lock(&self.disk);
        let mut out: Vec<&str> = self
            .files
            .iter()
            .filter(|(rel, doc)| match disk.files.get(rel.as_str()) {
                Some(d) if Arc::ptr_eq(doc, &d.doc) => false,
                Some(d) => doc.to_string().as_bytes() != d.bytes.as_slice(),
                None => true,
            })
            .map(|(rel, _)| rel.as_str())
            .collect();
        out.extend(
            self.raw
                .iter()
                .filter(|(rel, f)| {
                    // Unchanged when it is what this session last saved there, or, before any
                    // save, the file it was made from.
                    let on_disk = match disk.raw.get(rel.as_str()) {
                        Some(saved) => *saved,
                        None => f.base,
                    };
                    Some(f.fingerprint) != on_disk
                })
                .map(|(rel, _)| rel.as_str()),
        );
        // Files a command put that this case doesn't have (it is from before that command):
        // changed when a save wrote the command's version over the original.
        out.extend(disk.originals.iter().filter(|(rel, _)| !self.raw.contains_key(rel.as_str())).filter_map(
            |(rel, original)| match (original, disk.raw.get(rel.as_str()).copied().flatten()) {
                (Some(o), Some(saved)) if fingerprint(o) != saved => Some(rel.as_str()),
                _ => None,
            },
        ));
        out.sort_unstable();
        out.into_iter().map(str::to_string).collect()
    }

    /// A file that is not a dictionary (a surface in `constant/triSurface`): its bytes as the
    /// case has them, written by a command and not saved yet, or else read from `fs`. `None`
    /// when there is no such file.
    pub fn file(&self, fs: &dyn CaseFs, rel: &str) -> Result<Option<Arc<Vec<u8>>>> {
        if let Some(f) = self.raw.get(rel) {
            return Ok(Some(Arc::clone(&f.bytes)));
        }
        // From before a command whose version was saved: as it was then.
        if let Some(original) = self.saved_over(rel) {
            return Ok(Some(original));
        }
        Ok(fs.read(rel, MAX_FILE_BYTES)?.map(Arc::new))
    }

    /// The bytes [`Case::put_file`] last put at `rel` (saved or not), without reading the disk;
    /// `None` when no command wrote that file, so it is as on disk. Two calls return the same
    /// `Arc` until the file is put again: a cheap test of whether it changed.
    pub fn put_bytes(&self, rel: &str) -> Option<&Arc<Vec<u8>>> {
        self.raw.get(rel).map(|f| &f.bytes)
    }

    /// The bytes a file had before this session saved a command's version over it, when this
    /// case doesn't hold a version (it is from before that command) and there was a file.
    fn saved_over(&self, rel: &str) -> Option<Arc<Vec<u8>>> {
        if self.raw.contains_key(rel) {
            return None;
        }
        let disk = lock(&self.disk);
        disk.raw.get(rel).copied().flatten()?;
        disk.originals.get(rel).cloned().flatten()
    }

    /// The bytes a command about to put `rel` starts from, when the case holds no version: as
    /// before this session's save of a version, or else as on disk now, read once and kept as
    /// the original.
    fn hold(&self, fs: &dyn CaseFs, rel: &str) -> Result<Option<Arc<Vec<u8>>>> {
        let saved = lock(&self.disk).raw.get(rel).copied().flatten().is_some();
        if saved {
            return Ok(lock(&self.disk).originals.get(rel).cloned().flatten());
        }
        let now = fs.read(rel, MAX_FILE_BYTES)?.map(Arc::new);
        lock(&self.disk).originals.insert(rel.to_string(), now.clone());
        Ok(now)
    }

    /// The files [`Case::put_file`] wrote (saved or not), by path.
    pub fn put_paths(&self) -> impl Iterator<Item = &str> {
        self.raw.keys().map(String::as_str)
    }

    /// Sets the bytes of a file that is not a dictionary; [`Case::save`] writes it. A change to
    /// a file the case doesn't hold yet is based on what is on disk at `rel` now (what
    /// [`Case::file`] returned); the save refuses if that changed in the meantime. At most
    /// [`MAX_FILE_BYTES`].
    pub fn put_file(&mut self, fs: &dyn CaseFs, rel: &str, bytes: Vec<u8>) -> Result<()> {
        let bad = rel.is_empty()
            || rel.starts_with('/')
            || rel.contains('\\')
            || rel.split('/').any(|c| c.is_empty() || c == "." || c == "..")
            || rel.starts_with(".umcfgui/");
        if bad {
            return Err(Error::BadFile(rel.to_string()));
        }
        if self.files.contains_key(self.resolve(rel)) {
            return Err(Error::IsDict(rel.to_string()));
        }
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(
                FsError::TooLarge { path: rel.to_string(), size: bytes.len() as u64, limit: MAX_FILE_BYTES }.into()
            );
        }
        let base = match self.raw.get(rel) {
            Some(f) => f.base,
            None => self.hold(fs, rel)?.as_deref().map(|b| fingerprint(b)),
        };
        let fingerprint = fingerprint(&bytes);
        self.raw.insert(rel.to_string(), RawFile { bytes: Arc::new(bytes), fingerprint, base });
        Ok(())
    }

    /// Sets the boundary condition of `patches` in a field of the start time. A patch whose own
    /// entry already has type `kind` keeps it, with `entries` set in it (its other keys,
    /// comments and `#include`s stay). Otherwise its entry becomes `{ type <kind>; <key>
    /// <value>; … }`, replacing one of exactly that name: a new type starts afresh, and a group
    /// or pattern entry stays for the other patches it covers. Nothing else in the file
    /// changes, byte for byte: not the other entries, the comments, nor the data lists.
    /// [`Case::save`] writes it.
    pub fn set_bc(
        &mut self,
        fs: &dyn CaseFs,
        region: &str,
        field: &str,
        patches: &[&str],
        kind: &str,
        entries: &[(&str, &str)],
    ) -> Result<()> {
        let which = if region.is_empty() { field.to_string() } else { format!("{field} of region {region}") };
        let r = self
            .regions
            .iter()
            .position(|r| r.name == region)
            .ok_or_else(|| Error::Bc(format!("the case has no region `{region}`")))?;
        let info = self
            .regions
            .get(r)
            .and_then(|r| r.field(field))
            .ok_or_else(|| Error::Bc(format!("there is no field {which} at the start time")))?;
        if info.bcs.is_none() {
            return Err(Error::Bc(format!("{}: too large to be read here, so it isn't edited", info.path)));
        }
        if !info.boundary_field {
            return Err(Error::Bc(format!(
                "{}: its boundaryField comes from elsewhere (a macro or an #include): edit that file",
                info.path
            )));
        }
        // The placeholder of a field's data lists can't be written by an edit: the list would be
        // spliced in there.
        let words =
            std::iter::once(kind).chain(patches.iter().copied()).chain(entries.iter().flat_map(|(k, v)| [*k, *v]));
        if let Some(w) = words.into_iter().find(|w| w.contains(umcf_foam_field::PLACEHOLDER)) {
            return Err(Error::Bc(format!("`{w}` can't be written into a field file")));
        }
        let path = info.path.clone();
        let name = info.name.clone();
        let mut body = format!("{{ type {kind};");
        for (k, v) in entries {
            body.push_str(&format!(" {k} {v};"));
        }
        body.push_str(" }");
        let limits = umcf_foam_field::Limits { max_bytes: self.options.field_bytes, ..Default::default() };
        // Read once: the bytes edited are the ones the save will check the disk against.
        let bytes = match self.put_bytes(&path) {
            Some(b) => Some(Arc::clone(b)),
            None => self.hold(fs, &path)?,
        };
        let bytes = bytes.ok_or_else(|| Error::Bc(format!("{path} is not there any more")))?;
        let out = umcf_foam_field::rewrite(&bytes, &limits, |doc| {
            for p in patches {
                let own = doc.get(&["boundaryField", p]).and_then(umcf_foam_dict::Entry::dict);
                let visible = own.and_then(|d| word(d, "type"));
                // An entry whose type comes from an `#include` or a `$macro` in it.
                let from_elsewhere = own.is_some_and(|d| {
                    d.items.iter().any(|i| {
                        matches!(i, umcf_foam_dict::Item::Directive { .. } | umcf_foam_dict::Item::Macro { .. })
                    })
                });
                if visible.as_deref() == Some(kind) {
                    for (k, v) in entries {
                        doc.set(&["boundaryField", p, k], v)?;
                    }
                } else if visible.is_none() && from_elsewhere {
                    // Kept, with the type and entries set after what it brings in (the last
                    // one read wins).
                    doc.set(&["boundaryField", p, "type"], kind)?;
                    for (k, v) in entries {
                        doc.set(&["boundaryField", p, k], v)?;
                    }
                } else {
                    doc.set(&["boundaryField", p], &body)?;
                }
            }
            Ok(())
        })
        .map_err(|source| Error::Field { path: path.clone(), source })?;
        let parsed = Field::parse_with(&out, &limits).map_err(|source| Error::Field { path: path.clone(), source })?;
        let summary = summarise(name, path.clone(), &parsed);
        self.put_file(fs, &path, out)?;
        if let Some(f) = self.regions.get_mut(r).and_then(|r| r.fields.iter_mut().find(|f| f.path == path)) {
            *f = summary;
        }
        Ok(())
    }

    /// Writes the changed dictionaries and returns their paths.
    ///
    /// 1. Every changed file on disk must still be the one that was read (or last saved).
    ///    Otherwise [`Error::Conflict`] and nothing is written.
    /// 2. With `backup` (a directory under `.umcfgui/`, one per session), each file's
    ///    original goes to `<backup>/<path>` the first time the file is saved in the session.
    ///    If a different file is already there, the copy gets the next free `.1`, `.2`, …
    ///    name: a backup is never skipped or overwritten.
    /// 3. The files are written atomically, one by one, each checked once more right before
    ///    it is replaced. If a write fails or a file changed in the meantime,
    ///    [`Error::Partial`] names the files already saved. The check and the rename are not
    ///    one step: a program writing the same file in that instant can still lose its change.
    pub fn save(&mut self, fs: &mut dyn CaseWrite, backup: Option<&str>) -> Result<Vec<String>> {
        if let Some(dir) = backup {
            let dir = dir.trim_end_matches('/');
            if !dir.starts_with(".umcfgui/") || dir.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
                return Err(Error::BadBackup(dir.to_string()));
            }
        }
        let changed = self.changed();
        // 1. Check them all before writing anything.
        let mut plan = Vec::new();
        for rel in changed {
            if let Some(doc) = self.files.get(&rel) {
                let original = lock(&self.disk).files.get(&rel).map(|d| Arc::clone(&d.bytes));
                let limit = self.options.dict_bytes.max(original.as_ref().map_or(0, |b| b.len() as u64));
                // Grown past the limit: changed (it was read within it). Other errors are themselves.
                let on_disk = match fs.read(&rel, limit) {
                    Err(FsError::TooLarge { .. }) => return Err(Error::Conflict { path: rel }),
                    r => r?,
                };
                if on_disk.as_deref() != original.as_deref().map(Vec::as_slice) {
                    return Err(Error::Conflict { path: rel });
                }
                plan.push((rel, Pending::Dict(Arc::clone(doc)), original, limit));
            } else if let Some(f) = self.raw.get(&rel) {
                let saved = lock(&self.disk).raw.get(&rel).copied().flatten();
                let on_disk = match fs.read(&rel, MAX_FILE_BYTES) {
                    Err(FsError::TooLarge { .. }) => return Err(Error::Conflict { path: rel }),
                    r => r?,
                };
                let now = on_disk.as_deref().map(fingerprint);
                if now != f.base && (now.is_none() || now != saved) {
                    return Err(Error::Conflict { path: rel });
                }
                plan.push((
                    rel,
                    Pending::Raw(Arc::clone(&f.bytes), f.fingerprint),
                    on_disk.map(Arc::new),
                    MAX_FILE_BYTES,
                ));
            } else if let Some(original) = self.saved_over(&rel) {
                // An undo past a saved command: the original goes back.
                let saved = lock(&self.disk).raw.get(&rel).copied().flatten();
                let on_disk = match fs.read(&rel, MAX_FILE_BYTES) {
                    Err(FsError::TooLarge { .. }) => return Err(Error::Conflict { path: rel }),
                    r => r?,
                };
                if on_disk.as_deref().map(fingerprint) != saved {
                    return Err(Error::Conflict { path: rel });
                }
                let fp = fingerprint(&original);
                plan.push((rel, Pending::Raw(original, fp), on_disk.map(Arc::new), MAX_FILE_BYTES));
            }
        }
        // 2. Backups: once per file and session (backup directory).
        if let Some(dir) = backup {
            let dir = dir.trim_end_matches('/');
            for (rel, _, original, _) in &plan {
                let Some(original) = original else { continue };
                let key = (dir.to_string(), rel.clone());
                if lock(&self.disk).backed_up.contains(&key) {
                    continue;
                }
                let base = format!("{dir}/{rel}");
                let mut copy = base.clone();
                let mut k = 0;
                loop {
                    // Larger than the original: certainly a different file.
                    let same = match fs.read(&copy, original.len() as u64) {
                        Ok(Some(b)) => Some(b == original.as_slice()),
                        Err(FsError::TooLarge { .. }) => Some(false),
                        Ok(None) => None,
                        Err(e) => return Err(e.into()),
                    };
                    match same {
                        Some(true) => break,
                        Some(false) if k < 1000 => {
                            k += 1;
                            copy = format!("{base}.{k}");
                        }
                        Some(false) => return Err(Error::BackupsFull(base)),
                        None => {
                            fs.write(&copy, original)?;
                            break;
                        }
                    }
                }
                lock(&self.disk).backed_up.insert(key);
            }
        }
        // 3. Write.
        let mut saved = Vec::new();
        for (rel, pending, original, limit) in plan {
            let text = match &pending {
                Pending::Dict(doc) => Arc::new(doc.to_string().into_bytes()),
                Pending::Raw(bytes, _) => Arc::clone(bytes),
            };
            // A read error is reported as itself (it doesn't mean the file changed), except a
            // file grown past the limit, which did change.
            let result = match fs.read(&rel, limit) {
                Ok(on_disk) if on_disk.as_deref() == original.as_deref().map(Vec::as_slice) => {
                    fs.write(&rel, &text).map_err(Error::from)
                }
                Ok(_) | Err(FsError::TooLarge { .. }) => Err(Error::Conflict { path: rel.clone() }),
                Err(e) => Err(e.into()),
            };
            if let Err(e) = result {
                return Err(if saved.is_empty() { e } else { Error::Partial { saved, source: Box::new(e) } });
            }
            match pending {
                Pending::Dict(doc) => {
                    lock(&self.disk).files.insert(rel.clone(), OnDisk { bytes: text, doc });
                }
                Pending::Raw(_, fp) => {
                    lock(&self.disk).raw.insert(rel.clone(), Some(fp));
                }
            }
            saved.push(rel);
        }
        Ok(saved)
    }
}

fn read_fields(fs: &dyn CaseFs, dir: &str, options: OpenOptions, notes: &mut Vec<Note>) -> Result<Vec<FieldInfo>> {
    let limits = umcf_foam_field::Limits { max_bytes: options.field_bytes, ..Default::default() };
    let mut out = Vec::new();
    for name in times::fields(fs, dir)? {
        let plain = join(dir, &name);
        let path = if fs.is_file(&plain) { plain } else { format!("{plain}.gz") };
        match Field::read(fs, &join(dir, &name), &limits) {
            Ok(Some(f)) => out.push(summarise(name, path, &f)),
            Ok(None) => {}
            Err(umcf_foam_field::Error::Fs(FsError::TooLarge { .. }) | umcf_foam_field::Error::TooLarge { .. }) => {
                let why = if options.field_bytes == 0 {
                    "not read (field files skipped): its boundary conditions are not checked"
                } else {
                    "too large to read its boundary conditions here"
                };
                notes.push(note(Severity::Info, &path, why));
                out.push(FieldInfo {
                    name,
                    path,
                    class: None,
                    dimensions: None,
                    internal: None,
                    bcs: None,
                    indirect: false,
                    boundary_field: false,
                    object: None,
                });
            }
            Err(e) => notes.push(note(Severity::Warning, &path, e.to_string())),
        }
    }
    Ok(out)
}

/// A field value in short: lists by their size, not their numbers.
fn short(f: &Field, path: &[&str]) -> Option<String> {
    match f.value(path)? {
        Value::List(l) => Some(format!("nonuniform List<{}> {}", l.kind.name(), l.len())),
        _ => f.doc.value(path),
    }
}

/// An entry's value in short, read from the entry itself (no path lookup: a `boundaryField`
/// with thousands of patches stays linear).
fn short_entry(f: &Field, e: &umcf_foam_dict::Entry) -> Option<String> {
    use umcf_foam_dict::Node;
    if let [Node::Atom(n), Node::Atom(p)] = e.value.as_slice()
        && n.text == "nonuniform"
        && let Some(l) = f.placeholder_list(&p.text)
    {
        return Some(format!("nonuniform List<{}> {}", l.kind.name(), l.len()));
    }
    e.value_text()
}

/// A word entry (`type`, `patchType`): OpenFOAM's word reader takes a quoted string too.
fn word(d: &umcf_foam_dict::Dict, key: &str) -> Option<String> {
    d.get(key).and_then(umcf_foam_dict::Entry::value_text).map(|v| umcf_foam_dict::unquote(v.trim()).to_string())
}

fn summarise(name: String, path: String, f: &Field) -> FieldInfo {
    let entries = f.doc.get(&["boundaryField"]).and_then(umcf_foam_dict::Entry::dict);
    let bcs = entries
        .map(|d| {
            d.entries()
                .map(|e| {
                    let body = e.dict();
                    Bc {
                        patch: e.key().to_string(),
                        pattern: e.key.text.starts_with('"'),
                        kind: body.and_then(|b| word(b, "type")),
                        value: body.and_then(|b| b.get("value")).and_then(|v| short_entry(f, v)),
                        patch_type: body.and_then(|b| word(b, "patchType")),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    // Entry-level directives and `$macro` merges bring in entries this list can't show. Without
    // a `boundaryField` dictionary of its own (`boundaryField $common;`, or one that comes from
    // an `#include` at the top of the file), all of them may come from elsewhere.
    let brings_in = |d: &umcf_foam_dict::Dict| {
        d.items.iter().any(|i| matches!(i, umcf_foam_dict::Item::Directive { .. } | umcf_foam_dict::Item::Macro { .. }))
    };
    let indirect = match entries {
        Some(d) => brings_in(d),
        None => brings_in(&f.doc.root) || f.doc.get(&["boundaryField"]).is_some(),
    };
    FieldInfo {
        name,
        path,
        class: f.class().map(str::to_string),
        dimensions: f.dimensions(),
        internal: short(f, &["internalField"]),
        bcs: Some(bcs),
        indirect,
        boundary_field: entries.is_some(),
        object: f.header.object.clone(),
    }
}

#[cfg(test)]
mod tests;
