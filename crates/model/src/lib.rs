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
use std::fmt::Write as _;
use std::sync::Arc;

use umcf_core::{CaseFs, CaseWrite, FsError};
use umcf_foam_dict::Document;
use umcf_foam_field::times::{self, TimeDir};
use umcf_foam_field::{Field, Value};
pub use umcf_foam_mesh::Patch;
use umcf_schema::{CaseKind, Layout, Note, Severity};

pub mod blocks;
pub mod components;
mod controls;
pub mod description;
pub mod description_edit;
pub mod material_edit;
pub mod mesh_box;
pub mod new_case;
pub mod region_add;
pub mod results;
pub mod script;
pub mod settings;
pub mod tables;
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
    #[error("{0} is a symbolic link: writing it would write the file it points to")]
    IsLink(String),
    #[error("{0} is there already: give replace to write over it")]
    Exists(String),
    #[error("{0} already has 1000 backups")]
    BackupsFull(String),
    #[error("{path}: {source}")]
    Field { path: String, source: umcf_foam_field::Error },
    #[error("{0}")]
    Bc(String),
    /// An edit of the workflow description that can't be made, and why.
    #[error("{0}")]
    Describe(String),
    /// A new case that can't be made, and why.
    #[error("{0}")]
    NewCase(String),
    #[error("saving stopped after {saved:?}: {source}")]
    Partial { saved: Vec<String>, source: Box<Error> },
    #[error(
        "cancelled while the patch names in the results were being changed ({done} files changed so far): save again to finish it, or undo and save to take it back"
    )]
    ResultsCancelled { done: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Size limits for [`Case::open`]. Dictionaries are small; field files can be large, and only
/// their boundary conditions are needed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    /// A file bigger than this is not loaded as a dictionary.
    pub dict_bytes: u64,
    /// A field file bigger than this (after decompression) is not edited. At open, where only a
    /// field's outline is read, it bounds the text outside the file's lists instead (4 MB at
    /// most): a file with more is listed without its boundary conditions. 0: field files are
    /// not read at all.
    pub field_bytes: u64,
    /// The start-time fields are listed but not read: [`Case::read_start_fields`] reads them
    /// later (the desktop app opens a case first, then reads them as a job of their own).
    pub defer_fields: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions { dict_bytes: 4 << 20, field_bytes: 256 << 20, defer_fields: false }
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
    /// The sizes the `owner` header of the mesh the run reads notes (that of the latest time up to
    /// the start time with a mesh of its own, else `constant`'s); all `None` without a mesh, or
    /// a note.
    pub counts: umcf_foam_mesh::Counts,
    /// The fields of the initial time directory, by name.
    pub fields: Vec<FieldInfo>,
    /// A solid region's cell zones (`cellZones` of the mesh the run reads; none without the
    /// file), which its materials are assigned to, read with the start-time fields: `None` for
    /// other regions, without a mesh, until the fields are read, or when the file couldn't be
    /// read (a note says why). Shared, so copies of the case cost nothing.
    pub cell_zones: Option<Arc<Vec<umcf_foam_mesh::Zone>>>,
    /// Where a solid region's zones are read from (the mesh the run reads), `None` for other
    /// regions and without a mesh.
    pub zones_from: Option<String>,
}

/// The cell zones of the mesh in `dir` (none without the file), within the cap on a field file:
/// they are labels per cell, as big as a field (none read with a cap of 0). A file that can't be
/// read is a note.
fn solid_zones(
    fs: &dyn CaseFs,
    dir: &str,
    options: OpenOptions,
    notes: &mut Vec<Note>,
) -> Option<Arc<Vec<umcf_foam_mesh::Zone>>> {
    if options.field_bytes == 0 {
        return None;
    }
    let limits = umcf_foam_mesh::Limits { max_bytes: options.field_bytes, ..Default::default() };
    match umcf_foam_mesh::read_cell_zones(fs, dir, &limits) {
        Ok(z) => Some(Arc::new(z.unwrap_or_default())),
        Err(e) => {
            notes.push(note(
                Severity::Info,
                format!("{dir}/cellZones"),
                format!("{e}: the materials' cell zones are not checked"),
            ));
            None
        }
    }
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
    /// How many values a `nonuniform` `internalField` lists.
    pub internal_size: Option<usize>,
    /// `boundaryField` entries in file order. `None` when the file was too big to read here.
    pub bcs: Option<Vec<Bc>>,
    /// Bigger, decompressed, than commands edit ([`OpenOptions::field_bytes`]): its conditions
    /// are read and checked, not changed.
    pub too_big: bool,
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
    /// The entry holds data lists (`value nonuniform List<scalar> 120(…)`), one value per face
    /// of the patch.
    pub lists: bool,
    /// The condition's other entries that are one word (`qrNbr qr;`, `Tnbr T;`): those in
    /// [`KEPT_WORDS`] always, the others up to [`MAX_WORDS`]: what a check may need to know of
    /// how it is set.
    pub words: Vec<(String, String)>,
}

/// Most one-word entries a [`Bc`] keeps besides [`KEPT_WORDS`].
pub const MAX_WORDS: usize = 32;

/// The one-word entries a [`Bc`] keeps whatever comes before them, the last of a key given twice
/// (as OpenFOAM reads it): those the case check reads.
pub const KEPT_WORDS: [&str; 2] = ["qrNbr", "qsNbr"];

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

/// Refuses `rel` when it is a symbolic link: what is put there is written to the file the link
/// points to. A linked directory on the way is followed, as for any save.
fn unlinked(fs: &dyn CaseFs, rel: &str) -> Result<()> {
    if fs.is_link(rel) { Err(Error::IsLink(rel.to_string())) } else { Ok(()) }
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
    /// Files that are not dictionaries, as they were on disk when this session first saved a
    /// command's version of them. A case from before that command (an undo snapshot) has them
    /// so, even after the save.
    originals: BTreeMap<String, Original>,
    /// `(backup dir, file)` pairs already backed up: a file is backed up once per session.
    backed_up: std::collections::BTreeSet<(String, String)>,
    /// The workflow descriptions put whole ([`Case::put_description`]) so far, which numbers
    /// each one, and the number of the put behind the description last saved (0: none).
    description_puts: u64,
    description_saved: u64,
    /// Files saved as put whole ([`Case::put_file`]): an undo past that save puts the original
    /// back whole, so it is never written through a link either.
    whole: std::collections::BTreeSet<String>,
    /// The patch renames done in the results ([`Case::save_results`]), in order.
    results: Vec<results::ResultRename>,
    /// The renames a cancelled [`Case::save_results`] was doing, in order, how many of them
    /// each result file has had (by what the file is, [`CaseFs::identity`]: a file reached
    /// through a linked directory too; none when not listed), and how many have had their
    /// `boundaryData` directories renamed: the next one gives each only the rest.
    results_unfinished: Vec<results::ResultRename>,
    results_progress: BTreeMap<String, usize>,
    results_dirs_done: usize,
    /// What the cancelled calls renamed and skipped: reported by the one that finishes.
    results_saved: results::ResultsSaved,
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
    /// Put whole ([`Case::put_file`]), not an edit of the file there: never written through a
    /// link, which the save checks again.
    whole: bool,
    /// A script ([`Case::put_script`]): saved executable.
    script: bool,
}

/// The bytes a command edits a file from, and the fingerprint of the file they came from.
type EditBase = (Option<Arc<Vec<u8>>>, Option<Fingerprint>);

/// A mapped patch of another region that names a renamed patch: the region's index, the
/// patch, and the entries to set in it.
type Partner = (usize, String, Vec<(String, String)>);

/// A file as it was before this session's first save over it.
#[derive(Debug, Clone)]
struct Original {
    /// `None`: there was no file.
    bytes: Option<Arc<Vec<u8>>>,
    fingerprint: Option<Fingerprint>,
}

/// A boundary condition for [`Case::set_bc`]: its type, the entries set, and the defaults only a
/// new entry gets.
#[derive(Debug, Clone, Copy)]
pub struct Condition<'a> {
    pub kind: &'a str,
    pub entries: &'a [(&'a str, &'a str)],
    pub defaults: &'a [(&'a str, &'a str)],
}

/// A change to a mesh patch for [`Case::set_patch`]; `None` and empty leave things as they are.
#[derive(Debug, Clone, Copy, Default)]
pub struct PatchChange<'a> {
    /// A new name.
    pub name: Option<&'a str>,
    /// `patch`, `wall`, `mappedWall`, `symmetry`, `empty`, …
    pub kind: Option<&'a str>,
    /// The groups, replacing `inGroups`.
    pub groups: Option<&'a [&'a str]>,
    /// Other entries set in the patch (`neighbourRegion air`, …).
    pub entries: &'a [(&'a str, &'a str)],
    /// A new name followed into the results too, when saved ([`Case::save_results`]).
    pub results: bool,
}

/// What [`Case::set_patch`] changed besides the mesh, and what it couldn't.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchReport {
    /// Field files whose entry for the patch was renamed.
    pub fields: Vec<String>,
    /// Mapped patches of other regions (`region/patch`) that now name the new name.
    pub partners: Vec<String>,
    /// Field files not edited here (too large, or not read): not renamed.
    pub unread: Vec<String>,
    /// Field files with a pattern entry or text from elsewhere, which may name the patch by its
    /// old name: not changed there.
    pub indirect: Vec<String>,
    /// Field files of `0.orig` whose entry for the patch was renamed.
    pub orig: Vec<String>,
    /// Other files whose entry for the patch was renamed (a `viewFactorsDict`).
    pub others: Vec<String>,
    /// Time directories other than the start with the region's fields (results): not read, and
    /// not changed.
    pub times: Vec<String>,
    /// Where the old name is still written, as a word, in the case's dictionaries (`system`,
    /// `constant`, `0.orig`) once the rest is renamed: (file, line), the first [`MAX_MENTIONS`];
    /// line 0 for a directory of that name (`constant/<region>/boundaryData/<patch>`).
    pub mentions: Vec<(String, usize)>,
    /// There are more than [`MAX_MENTIONS`].
    pub more_mentions: bool,
    /// Files that could have been searched for the old name but couldn't be read.
    pub unsearched: Vec<String>,
    /// Files of `0.orig` and view factor dictionaries that may have an entry for the patch but
    /// weren't changed, and why.
    pub not_renamed: Vec<(String, String)>,
}

/// What [`Case::rename_entry`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Renamed {
    Yes,
    /// The file has no such entry, or is not a dictionary.
    No,
    /// Too large to be edited here.
    TooLarge,
    /// It has the entry but it can't be renamed (the new name is there already, the entry is
    /// there twice, the file can't be written back unchanged, …).
    Failed(String),
}

/// The most places [`PatchReport::mentions`] lists.
pub const MAX_MENTIONS: usize = 200;
/// The largest dictionary searched for mentions of a renamed patch.
const MENTION_BYTES: u64 = 4 << 20;

/// A patch [`Case::create_patch`] prepares.
#[derive(Debug, Clone, Copy)]
pub struct NewPatch<'a> {
    pub name: &'a str,
    /// `patch`, `wall`, `mappedWall`, …
    pub kind: &'a str,
    pub groups: &'a [&'a str],
    /// Other entries of the patch (`neighbourRegion air`, …).
    pub entries: &'a [(&'a str, &'a str)],
    /// The boundary faces it takes, by face label of the region's mesh.
    pub faces: &'a [u32],
}

/// What [`Case::create_patch`] wrote, and what is left to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateReport {
    /// The face set (`constant/<region>/polyMesh/sets/<name>`).
    pub set: String,
    /// The dictionary the patch was added to ([`pending_dict`]), and whether this made it.
    pub dict: String,
    pub new_dict: bool,
    /// The patches the faces come from, with how many each gives.
    pub from: Vec<(String, usize)>,
    /// Patches that give all their faces: `createPatch` removes them.
    pub emptied: Vec<String>,
    /// Field files given an entry for the new patch, a copy of the one of the patch its faces
    /// come from.
    pub fields: Vec<String>,
    /// Field files that still need an entry for it before a run: faces from several patches,
    /// or no entry of the patch's own to copy, and no entry for one of its groups (the given
    /// ones, `wall` for a wall type, the type for a constraint type).
    pub unfilled: Vec<String>,
    /// Field files with data lists on a patch that gives faces: their size is wrong once
    /// `createPatch` has run, so they must be made again (from `0.orig`, by `setFields`, …).
    pub resized: Vec<String>,
    /// Field files not edited here (too large, or not read).
    pub unread: Vec<String>,
    /// Field files with a pattern entry or text from elsewhere (`#includeEtc
    /// "caseDicts/setConstraintTypes"`), which may give the new patch a condition.
    pub maybe: Vec<String>,
}

/// Whether trivia `text` (one line) leaves a `/* … */` comment open at its end: read left to
/// right, a `//` ends the line and a `/*` runs to its `*/`.
fn opens_block_comment(text: &str) -> bool {
    let mut rest = text;
    loop {
        match (rest.find("//"), rest.find("/*")) {
            (_, None) => return false,
            (Some(l), Some(b)) if l < b => return false,
            (_, Some(b)) => match rest.get(b + 2..).and_then(|r| r.find("*/").map(|e| (r, e))) {
                Some((r, e)) => rest = r.get(e + 2..).unwrap_or(""),
                None => return true,
            },
        }
    }
}

/// How [`pending_dict`] says the mesh it was prepared on: its number of faces.
const STAMP: &str = "// prepared on a mesh of ";

/// The dictionary [`Case::create_patch`] prepares patches in, for one run of `createPatch
/// [-region <region>] -dict <it> -overwrite`.
pub fn pending_dict(region: &str) -> String {
    if region.is_empty() {
        "system/createPatchDict.umcf".to_string()
    } else {
        format!("system/{region}/createPatchDict.umcf")
    }
}

/// The `createPatch` command that makes the patches prepared in [`pending_dict`]. OpenFOAM-8
/// reads `-dict` below `system/<region>/` already, OpenFOAM-12 from the case; `None` gives the
/// command of each.
pub fn create_patch_command(region: &str, version: Option<umcf_core::OfVersion>) -> String {
    use umcf_core::OfVersion;
    if region.is_empty() {
        return "createPatch -dict system/createPatchDict.umcf -overwrite".to_string();
    }
    let v8 = format!("createPatch -region {region} -dict system/createPatchDict.umcf -overwrite");
    let v12 = format!("createPatch -region {region} -dict system/{region}/createPatchDict.umcf -overwrite");
    match version {
        Some(OfVersion::V8) => v8,
        Some(OfVersion::V12) => v12,
        None => format!("{v12} (OpenFOAM-12; OpenFOAM-8: {v8})"),
    }
}

/// OpenFOAM's wall patch types: a patch of one is in the group `wall`.
pub fn is_wall_type(kind: &str) -> bool {
    matches!(kind, "wall" | "mappedWall" | "mappedExtrudedWall" | "nonConformalMappedWall")
}

/// OpenFOAM's constraint patch types that take a condition of their own type (`empty`, …) and
/// are in the group of their type.
pub fn is_constraint_type(kind: &str) -> bool {
    matches!(kind, "empty" | "symmetry" | "symmetryPlane" | "wedge")
}

/// Whether patch `p` is in group `g`: its `inGroups`, or the group OpenFOAM gives its type
/// (`wall` for the wall types, the type for a constraint type).
fn in_group(p: &Patch, g: &str) -> bool {
    p.groups.iter().any(|x| umcf_foam_dict::unquote(x) == g)
        || (g == "wall" && is_wall_type(&p.kind))
        || (is_constraint_type(&p.kind) && p.kind == g)
}

/// The kind of condition a patch type takes: its own for a constraint type, `wall` for the wall
/// types, else the type. A copied entry is only right between patches of one kind.
fn condition_kind(kind: &str) -> &str {
    match kind {
        k if is_wall_type(k) => "wall",
        "patch" | "mapped" | "mappedPatch" | "mappedExtrudedPatch" => "patch",
        k => k,
    }
}

/// Patch types whose faces are paired with others (by `createPatch`'s ordering, or across
/// processors): faces are not taken from them.
pub const COUPLED: &[&str] = &[
    "processor",
    "processorCyclic",
    "cyclic",
    "cyclicAMI",
    "cyclicACMI",
    "cyclicSlip",
    "cyclicRepeatAMI",
    "nonConformalCyclic",
    "nonConformalProcessorCyclic",
    "nonConformalError",
];

/// What [`Case::set_bc`] couldn't do as asked, by patch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BcReport {
    /// Their replaced entry brought in text from elsewhere (`#include`, `$macro`): it is gone.
    pub replaced: Vec<String>,
    /// Their kept entry brings in text from elsewhere, which isn't read here: the defaults were
    /// not set in it, and it may have no value.
    pub unfilled: Vec<String>,
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
    /// The start-time fields are listed, not read yet ([`OpenOptions::defer_fields`]): their
    /// boundary conditions aren't known until [`Case::read_start_fields`].
    pub fields_pending: bool,
    /// The data tables the solver reads by file name that are there (`tables::TABLES`), with
    /// their first and last times.
    pub tables: Vec<tables::Table>,
    /// Where in [`Case::notes`] the fields' notes go when they are read later: where reading
    /// them at open would have put them.
    field_notes_at: usize,
    /// Where the solids' cell zone notes go among the notes (after the regions'), as for
    /// `field_notes_at`.
    zone_notes_at: usize,
    /// The paths in the case when it was opened (see [`Case::exists`]).
    paths: std::collections::BTreeSet<String>,
    files: BTreeMap<String, Arc<Document>>,
    /// Files that are not dictionaries, as commands wrote them.
    raw: BTreeMap<String, RawFile>,
    /// Paths that are the same file as a loaded one (a symlinked dictionary): path → that one.
    aliases: BTreeMap<String, String>,
    /// The number of the workflow description put whole ([`Case::put_description`], new or
    /// over the one there) this case holds, 0 when none: until the description on disk is this
    /// put's, the save checks it isn't a link. Kept with the shared disk state, so an undo
    /// snapshot from before or after the save agrees.
    description_put: u64,
    /// The patch renames to follow into the results ([`Case::follow_results`]), in order.
    results: Vec<results::ResultRename>,
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
/// `<dir>/\u{1}`, and one whose listing failed `<dir>/\u{2}` (the case root: `\u{2}`): what is
/// below either is not known.
fn snapshot(fs: &dyn CaseFs) -> Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    let mut stack: Vec<(String, usize)> = vec![(String::new(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let names = match fs.list(&dir) {
            Ok(n) => n,
            // A directory that can't be listed: what is below it is not known.
            Err(_) => {
                out.insert(if dir.is_empty() { "\u{2}".to_string() } else { format!("{dir}/\u{2}") });
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
    /// Opens the case in `fs` with the default [`OpenOptions`].
    pub fn open(fs: &(dyn CaseFs + Sync)) -> Result<Case> {
        Case::open_with(fs, OpenOptions::default())
    }

    /// Opens the case in `fs`: its layout, dictionaries, start time, and each region's patches,
    /// mesh counts and start-time fields (read in parallel, as outlines). A file that can't be
    /// read is a note in [`Case::notes`]; a folder that isn't a case, an unreadable
    /// `controlDict` or a directory that can't be listed is an error.
    pub fn open_with(fs: &(dyn CaseFs + Sync), options: OpenOptions) -> Result<Case> {
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

        // The start-time fields of every region, read at once.
        let field_dirs: Vec<String> = match &start {
            Some(t) => regions
                .iter()
                .map(|(name, _)| if name.is_empty() { t.name.clone() } else { join(&t.name, name) })
                .collect(),
            None => Vec::new(),
        };
        let mut fields = read_fields(fs, &field_dirs, options, None, &mut notes)?.into_iter();
        let field_notes_at = notes.len();
        // The zones' notes after every region's (as a deferred read puts them).
        let mut zone_notes = Vec::new();
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
            // The sizes the `owner` header notes, for the fields' lengths (its first bytes only),
            // of the mesh the run reads: the latest time up to the start time with a mesh of
            // its own (meshing without `-overwrite` writes one), else `constant`.
            let mesh_owner = start
                .as_ref()
                .and_then(|t| {
                    time_dirs.iter().filter(|d| d.value <= t.value).rev().find_map(|d| {
                        let dir = if name.is_empty() {
                            format!("{}/polyMesh", d.name)
                        } else {
                            format!("{}/{name}/polyMesh", d.name)
                        };
                        (fs.is_file(&format!("{dir}/owner")) || fs.is_file(&format!("{dir}/owner.gz"))).then_some(dir)
                    })
                })
                .unwrap_or_else(|| mesh.clone());
            // Read whether or not `constant` has a mesh: a time directory may have the only one.
            // The error names the file (`owner` or `owner.gz`).
            let counts = match umcf_foam_mesh::counts(fs, &mesh_owner, &umcf_foam_mesh::Limits::default()) {
                Ok(c) => c,
                Err(e) => {
                    notes.push(note(Severity::Info, mesh_owner.clone(), format!("{e}: field sizes not checked")));
                    Default::default()
                }
            };
            // A solid's materials go by cell zone (solid/updatebuildingMaterials.H): read with
            // the fields, which a deferred open reads later.
            let meshed = patches.is_some() || counts.cells.is_some();
            let zones_from = (kind == RegionKind::Solid && meshed).then(|| mesh_owner.clone());
            let cell_zones = match &zones_from {
                Some(dir) if !options.defer_fields => solid_zones(fs, dir, options, &mut zone_notes),
                _ => None,
            };
            let fields = fields.next().unwrap_or_default();
            out.push(Region { name, kind, patches, counts, fields, cell_zones, zones_from });
        }

        let zone_notes_at = notes.len();
        notes.append(&mut zone_notes);
        let tables = tables::read(fs, options.dict_bytes);
        let scripts = read_scripts(fs, options, &mut notes)?;
        let paths = snapshot(fs)?;
        let disk = Arc::new(std::sync::Mutex::new(DiskState {
            files: disk,
            raw: BTreeMap::new(),
            originals: BTreeMap::new(),
            backed_up: Default::default(),
            description_puts: 0,
            description_saved: 0,
            whole: Default::default(),
            results: Vec::new(),
            results_unfinished: Vec::new(),
            results_progress: BTreeMap::new(),
            results_dirs_done: 0,
            results_saved: Default::default(),
        }));
        let fields_pending = options.defer_fields && start.is_some();
        Ok(Case {
            layout,
            controls,
            start,
            regions: out,
            notes,
            processors,
            scripts,
            fields_pending,
            tables,
            field_notes_at,
            zone_notes_at,
            paths,
            files,
            raw: BTreeMap::new(),
            aliases,
            description_put: 0,
            results: Vec::new(),
            disk,
            options,
        })
    }

    /// Reads the start-time fields a case opened with [`OpenOptions::defer_fields`] listed
    /// only, in parallel as at open, counting each file done in `progress`; their notes join the
    /// case's. Nothing to do (`false`) when they were read already: a field edited since would
    /// otherwise be read back from disk.
    pub fn read_start_fields(
        &mut self,
        fs: &(dyn CaseFs + Sync),
        progress: Option<&umcf_core::Progress>,
    ) -> Result<bool> {
        if !self.fields_pending {
            return Ok(false);
        }
        let Some(t) = &self.start else {
            self.fields_pending = false;
            return Ok(true);
        };
        let dirs: Vec<String> = self
            .regions
            .iter()
            .map(|r| if r.name.is_empty() { t.name.clone() } else { join(&t.name, &r.name) })
            .collect();
        let options = OpenOptions { defer_fields: false, ..self.options };
        let mut notes = Vec::new();
        let read = read_fields(fs, &dirs, options, progress, &mut notes)?;
        let mut zone_notes = Vec::new();
        for (region, fields) in self.regions.iter_mut().zip(read) {
            region.fields = fields;
            if let Some(dir) = &region.zones_from {
                region.cell_zones = solid_zones(fs, dir, options, &mut zone_notes);
            }
        }
        // In the order reading them at open gives: the fields' notes, then (after the regions'
        // notes, moved on by those) the zones'.
        let shift = notes.len();
        let at = self.field_notes_at.min(self.notes.len());
        self.notes.splice(at..at, notes);
        let at = (self.zone_notes_at + shift).min(self.notes.len());
        self.notes.splice(at..at, zone_notes);
        self.fields_pending = false;
        Ok(true)
    }

    /// An error while the start-time fields are listed only: commands that change the field
    /// files with the mesh (renaming or adding a patch) wait for them.
    fn fields_read(&self) -> Result<()> {
        if self.fields_pending {
            return Err(Error::Bc("the field files aren't read yet: Read fields (case.fields) first".to_string()));
        }
        Ok(())
    }

    /// Takes the start-time fields `read` has read since this case (a snapshot of it from before,
    /// in the undo history) was taken with them listed only: nothing can have edited a field
    /// before it was read, so they are the same files. Nothing to do once its fields are read.
    pub fn adopt_fields(&mut self, read: &Case) {
        if !self.fields_pending || read.fields_pending {
            return;
        }
        for region in &mut self.regions {
            if let Some(r) = read.regions.iter().find(|r| r.name == region.name) {
                region.fields = r.fields.clone();
                region.cell_zones = r.cell_zones.clone();
            }
        }
        self.notes = read.notes.clone();
        self.fields_pending = false;
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
            if self.paths.contains(&format!("{parent}/\u{1}")) || self.paths.contains(&format!("{parent}/\u{2}")) {
                return None;
            }
            dir = parent;
        }
        let top = rel.split('/').next().unwrap_or(rel);
        if top.starts_with("processor")
            || self.paths.contains(&format!("{top}/\u{1}"))
            || self.paths.contains(&format!("{top}/\u{2}"))
            || self.paths.contains("\u{1}")
            || self.paths.contains("\u{2}")
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
        wf.case_regions = regions.clone();
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
        // A split's zone as its topoSetDict makes them: a zone the clean-up named is kept only
        // when the dictionary makes it; else the one zone it makes, if one. The zones it makes
        // go to the workflow, which writes no split for a dictionary that makes others too.
        for m in &mut wf.meshes {
            let workflow::MeshBuild::Split(sp) = &mut m.build else { continue };
            match self.dict(&sp.dict).map(workflow::zones_told) {
                Some(Some(made)) => {
                    if !sp.zone.as_ref().is_some_and(|z| made.contains(z)) {
                        sp.zone = match made.as_slice() {
                            [zone] => Some(zone.clone()),
                            _ => None,
                        };
                    }
                    wf.split_zones.push((m.region.clone(), workflow::DictZones::Made(made)));
                }
                // There, but what it makes can't be told.
                Some(None) => wf.split_zones.push((m.region.clone(), workflow::DictZones::Unclear)),
                // Known not to be there: no script (topoSet would stop it after the copy).
                None if self.exists(&sp.dict) == Some(false) => {
                    wf.split_zones.push((m.region.clone(), workflow::DictZones::Missing))
                }
                // Maybe one the case doesn't list (below a mesh or a later time).
                None => wf.split_zones.push((m.region.clone(), workflow::DictZones::Unclear)),
            }
        }
        wf
    }

    /// The size limits the case was opened with.
    pub fn options(&self) -> OpenOptions {
        self.options
    }

    /// The case's workflow description (`system/umcfWorkflow`), read, with its problems; `None`
    /// when the case has none (or it isn't a dictionary: see the open notes).
    pub fn description(&self) -> Option<(description::Description, Vec<description::Problem>)> {
        self.dict(description::PATH).map(description::Description::read)
    }

    /// Puts `text` as the workflow description, a new dictionary or one replacing the case's
    /// (`replace`). Refused when there is one and `replace` is false, and when the file is on
    /// disk but didn't load as a dictionary (whatever `replace`).
    pub fn put_description(&mut self, fs: &dyn CaseFs, text: &str, replace: bool) -> Result<()> {
        let rel = description::PATH;
        // The same file as another dictionary (a link, either way): writing it would change
        // what that one reads.
        if self.resolve(rel) != rel || self.aliases.values().any(|p| p == rel) {
            return Err(Error::IsDict(rel.to_string()));
        }
        // A link to a file that isn't one of the loaded dictionaries (a field, a script), or to
        // nothing: the save would write through it.
        unlinked(fs, rel)?;
        if self.dict(rel).is_none() && (self.raw.contains_key(rel) || fs.is_file(rel) || fs.is_dir(rel)) {
            // On disk but not loaded (not a dictionary): not written over, with or without
            // `replace`; the file is mended or removed by hand.
            return Err(Error::NotLoaded { path: rel.to_string() });
        }
        if self.dict(rel).is_some() && !replace {
            return Err(Error::Exists(rel.to_string()));
        }
        let doc = Document::parse(text).map_err(|source| Error::Dict { path: rel.to_string(), source })?;
        self.files.insert(rel.to_string(), Arc::new(doc));
        let mut disk = lock(&self.disk);
        disk.description_puts += 1;
        self.description_put = disk.description_puts;
        Ok(())
    }

    /// Puts `text` as the whole dictionary `rel` (a generated one, such as a `blockMeshDict`
    /// written from the workflow), new or replacing the one the case has; [`Case::save`]
    /// writes it, keeping a backup of the file it replaces. Refused for a path that is a link
    /// or the same file as another dictionary, a path outside the case, and a file on disk
    /// that didn't load as a dictionary.
    pub fn put_dict(&mut self, fs: &dyn CaseFs, rel: &str, text: &str) -> Result<()> {
        let bad = rel.is_empty()
            || rel.starts_with('/')
            || rel.contains('\\')
            || rel.split('/').any(|c| c.is_empty() || c == "." || c == "..")
            || rel.starts_with(".umcfgui/");
        if bad {
            return Err(Error::BadFile(rel.to_string()));
        }
        if self.resolve(rel) != rel || self.aliases.values().any(|p| p == rel) {
            return Err(Error::IsDict(rel.to_string()));
        }
        unlinked(fs, rel)?;
        if self.dict(rel).is_none() && (self.raw.contains_key(rel) || fs.is_file(rel) || fs.is_dir(rel)) {
            return Err(Error::NotLoaded { path: rel.to_string() });
        }
        let doc = Document::parse(text).map_err(|source| Error::Dict { path: rel.to_string(), source })?;
        self.files.insert(rel.to_string(), Arc::new(doc));
        Ok(())
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

    /// Sets the entry at `path` as [`Case::set`] does, a dictionary value kept on the key's
    /// line as written (`{ role side; }`).
    pub fn set_inline(&mut self, rel: &str, path: &[&str], value: &str) -> Result<()> {
        self.dict_mut(rel)?.set_inline(path, value).map_err(|source| Error::Dict { path: rel.to_string(), source })?;
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
            |(rel, o)| match (&o.bytes, disk.raw.get(rel.as_str()).copied().flatten()) {
                (Some(_), Some(saved)) if o.fingerprint != Some(saved) => Some(rel.as_str()),
                _ => None,
            },
        ));
        out.sort_unstable();
        let mut out: Vec<String> = out.into_iter().map(str::to_string).collect();
        drop(disk);
        // Renames still to be followed into the results (or undone there).
        out.extend(self.results_pending().iter().map(results::ResultRename::label));
        out
    }

    /// A file that is not a dictionary (a surface in `constant/triSurface`): its bytes as the
    /// case has them, written by a command and not saved yet, or else read from `fs`. `None`
    /// when there is no such file.
    pub fn file(&self, fs: &dyn CaseFs, rel: &str) -> Result<Option<Arc<Vec<u8>>>> {
        if let Some(f) = self.raw.get(rel) {
            return Ok(Some(Arc::clone(&f.bytes)));
        }
        // From before a command whose version was saved: as it was then.
        if let Some((original, _)) = self.saved_over(rel) {
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

    /// The file as it was before this session saved a command's version over it, when this
    /// case doesn't hold a version (it is from before that command), there was a file, and the
    /// disk still has the command's version.
    fn saved_over(&self, rel: &str) -> Option<(Arc<Vec<u8>>, Fingerprint)> {
        if self.raw.contains_key(rel) {
            return None;
        }
        let disk = lock(&self.disk);
        let saved = disk.raw.get(rel).copied().flatten()?;
        let o = disk.originals.get(rel)?;
        match (&o.bytes, o.fingerprint) {
            (Some(b), Some(f)) if f != saved => Some((Arc::clone(b), f)),
            _ => None,
        }
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
        self.puttable(rel)?;
        // A whole new file, not an edit of the one there: a link (to a field, another surface,
        // nothing) would carry it to another file.
        unlinked(fs, rel)?;
        let base = match self.raw.get(rel) {
            Some(f) => f.base,
            None => match self.saved_over(rel) {
                Some((_, f)) => Some(f),
                None => fs.read(rel, MAX_FILE_BYTES)?.map(|b| fingerprint(&b)),
            },
        };
        self.put_based(rel, bytes, base)?;
        if let Some(f) = self.raw.get_mut(rel) {
            f.whole = true;
        }
        Ok(())
    }

    /// Puts a script whole, as [`Case::put_file`] does; [`Case::save`] writes it executable
    /// ([`CaseWrite::write_executable`]). A script the case reads (`Allrun`, … at its root) is read
    /// again from the new text, so its workflow and checks are of the script as it now is (the
    /// notes the open gave on the old one stay until the case is opened again).
    pub fn put_script(&mut self, fs: &dyn CaseFs, rel: &str, bytes: Vec<u8>) -> Result<()> {
        let read = !rel.contains('/') && script::is_script_name(rel) && bytes.len() as u64 <= self.options.dict_bytes;
        let parsed = read.then(|| script::parse(rel, &String::from_utf8_lossy(&bytes)));
        self.put_file(fs, rel, bytes)?;
        if let Some(f) = self.raw.get_mut(rel) {
            f.script = true;
        }
        if let Some(s) = parsed {
            match self.scripts.iter_mut().find(|x| x.path == rel) {
                Some(old) => *old = s,
                None => self.scripts.push(s),
            }
        }
        Ok(())
    }

    /// [`Case::put_file`] with the fingerprint of the file the new bytes were made from.
    fn put_based(&mut self, rel: &str, bytes: Vec<u8>, base: Option<Fingerprint>) -> Result<()> {
        self.puttable(rel)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(
                FsError::TooLarge { path: rel.to_string(), size: bytes.len() as u64, limit: MAX_FILE_BYTES }.into()
            );
        }
        let fingerprint = fingerprint(&bytes);
        // An edit of a file put whole is still a file put whole.
        let whole = self.raw.get(rel).is_some_and(|f| f.whole);
        let script = self.raw.get(rel).is_some_and(|f| f.script);
        self.raw.insert(rel.to_string(), RawFile { bytes: Arc::new(bytes), fingerprint, base, whole, script });
        Ok(())
    }

    /// Whether a command may put a file at `rel`: a path inside the case, not a dictionary.
    fn puttable(&self, rel: &str) -> Result<()> {
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
        Ok(())
    }

    /// Sets the boundary condition of `patches` in a field of the start time. A patch whose own
    /// entry already has type `kind` keeps it, with `entries` set in it (its other keys,
    /// comments and `#include`s stay). So does an entry without a type of its own that brings
    /// text in from an `#include` or a `$macro`: the type and `entries` are set after it.
    /// Otherwise its entry becomes `{ type <kind>; <key> <value>; … }`, replacing one of
    /// exactly that name: a new type starts afresh, and a group or pattern entry stays for the
    /// other patches it covers. Nothing else in the file changes, byte for byte: not the other
    /// entries, the comments, nor the data lists. [`Case::save`] writes it. `defaults` go into
    /// new entries only, after `entries` (a key in both is taken from `entries`): a kept entry
    /// keeps its own. Returns which patches lost text from elsewhere and which kept an entry
    /// from elsewhere without the defaults.
    pub fn set_bc(
        &mut self,
        fs: &dyn CaseFs,
        region: &str,
        field: &str,
        patches: &[&str],
        condition: &Condition,
    ) -> Result<BcReport> {
        let Condition { kind, entries, defaults } = *condition;
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
        if self.fields_pending {
            return Err(Error::Bc(format!(
                "{}: the field files aren't read yet: Read fields (case.fields) first",
                info.path
            )));
        }
        if info.bcs.is_none() || info.too_big {
            return Err(Error::Bc(format!("{}: too large to be edited here", info.path)));
        }
        if !info.boundary_field {
            return Err(Error::Bc(format!(
                "{}: its boundaryField comes from elsewhere (a macro or an #include): edit that file",
                info.path
            )));
        }
        // The placeholder of a field's data lists can't be written by an edit: the list would be
        // spliced in there.
        let words = std::iter::once(kind)
            .chain(patches.iter().copied())
            .chain(entries.iter().chain(defaults).flat_map(|(k, v)| [*k, *v]));
        if let Some(w) = words.into_iter().find(|w| w.contains(umcf_foam_field::PLACEHOLDER)) {
            return Err(Error::Bc(format!("`{w}` can't be written into a field file")));
        }
        let path = info.path.clone();
        let name = info.name.clone();
        // A new entry: the entries, then the defaults not given, `value` last as OpenFOAM writes it.
        let given: std::collections::HashSet<&str> = entries.iter().map(|(k, _)| *k).collect();
        let mut fresh: Vec<&(&str, &str)> =
            entries.iter().chain(defaults.iter().filter(|(k, _)| !given.contains(k))).collect();
        fresh.sort_by_key(|(k, _)| *k == "value");
        let mut body = format!("{{ type {kind};");
        for (k, v) in fresh {
            body.push_str(&format!(" {k} {v};"));
        }
        body.push_str(" }");
        let limits = umcf_foam_field::Limits { max_bytes: self.options.field_bytes, ..Default::default() };
        // Read once: the bytes edited are the ones the save will check the disk against.
        let (bytes, base) = self.edit_base(fs, &path)?;
        let bytes = bytes.ok_or_else(|| Error::Bc(format!("{path} is not there any more")))?;
        // Patches whose replaced entry brought in text from elsewhere (`#include`, `$macro`).
        let mut dropped = Vec::new();
        let mut unfilled = Vec::new();
        let out = umcf_foam_field::rewrite(&bytes, &limits, |doc| {
            for p in patches {
                let indirect_value = doc
                    .get(&["boundaryField", p])
                    .filter(|e| e.dict().is_none())
                    .and_then(umcf_foam_dict::Entry::value_text)
                    .is_some_and(|v| v.trim_start().starts_with(['$', '#']));
                let own = doc.get(&["boundaryField", p]).and_then(umcf_foam_dict::Entry::dict);
                let visible = own.and_then(|d| word(d, "type"));
                // An entry whose type comes from an `#include` or a `$macro` in it.
                let from_elsewhere = own.is_some_and(|d| {
                    d.items.iter().any(|i| {
                        matches!(i, umcf_foam_dict::Item::Directive { .. } | umcf_foam_dict::Item::Macro { .. })
                    })
                });
                let missing_defaults = defaults.iter().any(|(k, _)| !entries.iter().any(|(e, _)| e == k));
                if visible.as_deref() == Some(kind) {
                    for (k, v) in entries {
                        doc.set(&["boundaryField", p, k], v)?;
                    }
                    // Its own keys are kept as they are; what it brings in isn't read here.
                    if from_elsewhere && missing_defaults {
                        unfilled.push(p.to_string());
                    }
                } else if visible.is_none() && from_elsewhere {
                    // Kept, with the type and entries set after what it brings in (the last
                    // one read wins). No defaults: what it brings in isn't read here, and a
                    // default could override a value it has.
                    doc.set(&["boundaryField", p, "type"], kind)?;
                    for (k, v) in entries {
                        doc.set(&["boundaryField", p, k], v)?;
                    }
                    if missing_defaults {
                        unfilled.push(p.to_string());
                    }
                } else {
                    if from_elsewhere || indirect_value {
                        dropped.push(p.to_string());
                    }
                    doc.set(&["boundaryField", p], &body)?;
                }
            }
            Ok(())
        })
        .map_err(|source| Error::Field { path: path.clone(), source })?;
        let parsed = Field::parse_with(&out, &limits).map_err(|source| Error::Field { path: path.clone(), source })?;
        let summary = summarise(name, path.clone(), &parsed);
        self.put_based(&path, out, base)?;
        if let Some(f) = self.regions.get_mut(r).and_then(|r| r.fields.iter_mut().find(|f| f.path == path)) {
            *f = summary;
        }
        Ok(BcReport { replaced: dropped, unfilled })
    }

    /// The bytes a command edits a file from, read once, and the fingerprint of the file they
    /// were read from (what [`Case::save`] checks the disk against).
    fn edit_base(&self, fs: &dyn CaseFs, path: &str) -> Result<EditBase> {
        Ok(match (self.raw.get(path), self.saved_over(path)) {
            (Some(f), _) => (Some(Arc::clone(&f.bytes)), f.base),
            (None, Some((b, f))) => (Some(b), Some(f)),
            (None, None) => {
                let b = fs.read(path, MAX_FILE_BYTES)?.map(Arc::new);
                let f = b.as_deref().map(|b| fingerprint(b));
                (b, f)
            }
        })
    }

    /// The size a file a command edits may have, decompressed: [`Case::rewrite_file`]'s cap, so
    /// that what it wrote can be read back.
    fn edit_cap(&self) -> u64 {
        self.options.field_bytes.max(1 << 20)
    }

    /// The patches of `boundary` bytes [`Case::rewrite_file`] returned.
    fn reparse_boundary(&self, path: &str, bytes: &[u8]) -> Result<Vec<Patch>> {
        let limits = umcf_foam_mesh::Limits { max_bytes: self.edit_cap(), ..Default::default() };
        umcf_foam_mesh::parse_boundary_bytes(bytes, &limits).map_err(|e| Error::Bc(format!("{path}: {e}")))
    }

    /// Rewrites one file a command edits (a field, a mesh's `boundary`) with `edit`, keeping
    /// every byte it doesn't touch, and puts it; returns the new bytes.
    fn rewrite_file(
        &mut self,
        fs: &dyn CaseFs,
        path: &str,
        edit: impl FnOnce(&mut umcf_foam_dict::Document) -> umcf_foam_field::Result<()>,
    ) -> Result<Vec<u8>> {
        let limits = umcf_foam_field::Limits { max_bytes: self.edit_cap(), ..Default::default() };
        let (bytes, base) = self.edit_base(fs, path)?;
        let bytes = bytes.ok_or_else(|| Error::Bc(format!("{path} is not there any more")))?;
        let out = umcf_foam_field::rewrite(&bytes, &limits, edit)
            .map_err(|source| Error::Field { path: path.to_string(), source })?;
        self.put_based(path, out.clone(), base)?;
        Ok(out)
    }

    /// Changes a patch of a region's mesh (`constant/<region>/polyMesh/boundary`): its type,
    /// its groups, other entries (`neighbourRegion`, …), or its name. Only those entries
    /// change, byte for byte. A new name goes into the region's field files at the start time
    /// and in `0.orig` (the patch's own `boundaryField` entry), its entry in the region's
    /// `viewFactorsDict`, and the mapped patches of other regions that name it; the report lists
    /// the other time directories and where the old name is still written. [`Case::save`] writes
    /// it all.
    pub fn set_patch(
        &mut self,
        fs: &dyn CaseFs,
        region: &str,
        patch: &str,
        change: &PatchChange,
    ) -> Result<PatchReport> {
        // The field files follow a patch's name: they must be read to be followed.
        self.fields_read()?;
        let r = self
            .regions
            .iter()
            .position(|r| r.name == region)
            .ok_or_else(|| Error::Bc(format!("the case has no region `{region}`")))?;
        let patches = self
            .regions
            .get(r)
            .and_then(|r| r.patches.clone())
            .ok_or_else(|| Error::Bc(format!("region `{region}` has no mesh")))?;
        if !patches.iter().any(|p| p.name == patch) {
            return Err(Error::Bc(format!("region `{region}` has no patch `{patch}`")));
        }
        let words = [change.name, change.kind]
            .into_iter()
            .flatten()
            .chain(change.groups.unwrap_or(&[]).iter().copied())
            .chain(change.entries.iter().flat_map(|(k, v)| [*k, *v]));
        if let Some(w) = words.into_iter().find(|w| w.contains(umcf_foam_field::PLACEHOLDER)) {
            return Err(Error::Bc(format!("`{w}` can't be written into a mesh file")));
        }
        if let Some(to) = change.name
            && to != patch
        {
            self.name_free(r, &patches, to)?;
            let twice = self
                .regions
                .get(r)
                .into_iter()
                .flat_map(|reg| &reg.fields)
                .filter(|f| !f.too_big)
                .find(|f| f.bcs.iter().flatten().filter(|b| b.patch == patch).count() > 1);
            if let Some(f) = twice {
                return Err(Error::Bc(format!("{} has two entries `{patch}`: keep one first", f.path)));
            }
        }
        let mut report = PatchReport::default();
        let dir = umcf_foam_mesh::mesh_dir(region);
        let path = self.mesh_file(fs, &dir, "boundary");
        let out = self.rewrite_file(fs, &path, |doc| {
            let (name, dict) = umcf_foam_mesh::boundary_patch_mut(doc, patch)
                .ok_or_else(|| umcf_foam_field::Error::Unsupported(format!("no patch `{patch}` in the file")))?;
            if let Some(kind) = change.kind {
                dict.set(&["type"], kind)?;
            }
            if let Some(groups) = change.groups {
                dict.set(&["inGroups"], &format!("List<word> {}({})", groups.len(), groups.join(" ")))?;
            }
            for (k, v) in change.entries {
                dict.set(&[k], v)?;
            }
            if let Some(to) = change.name {
                name.text = to.to_string();
            }
            Ok(())
        })?;
        let parsed = self.reparse_boundary(&path, &out)?;
        if let Some(reg) = self.regions.get_mut(r) {
            reg.patches = Some(parsed);
        }
        let Some(to) = change.name.filter(|to| *to != patch) else { return Ok(report) };
        if change.results {
            self.follow_results(region, patch, to);
        }
        // The region's fields: the patch's own entry gets the new name.
        let fields: Vec<(String, String, bool)> = self.regions.get(r).map_or(Vec::new(), |reg| {
            reg.fields
                .iter()
                .map(|f| {
                    let own = f.bcs.as_ref().is_some_and(|b| b.iter().any(|b| b.patch == patch));
                    // Without an entry of its own, a pattern or text from elsewhere may give it
                    // its condition by the old name: not changed there.
                    let maybe = f.indirect || f.bcs.iter().flatten().any(|b| b.pattern);
                    if maybe && !own && f.bcs.is_some() {
                        report.indirect.push(f.path.clone());
                    }
                    (f.name.clone(), f.path.clone(), f.bcs.is_some() && own)
                })
                .collect()
        });
        let limits = umcf_foam_field::Limits { max_bytes: self.options.field_bytes, ..Default::default() };
        for (name, fpath, own) in fields {
            // Not read, or read but too big to edit with an entry of its own: that entry keeps
            // the old name.
            let unread = self
                .regions
                .get(r)
                .and_then(|reg| reg.field(&name))
                .is_some_and(|f| f.bcs.is_none() || (f.too_big && own));
            if unread {
                report.unread.push(fpath);
                continue;
            }
            if !own {
                continue;
            }
            let out = self.rewrite_file(fs, &fpath, |doc| {
                if let Some(bf) = doc.root.get_mut("boundaryField").and_then(umcf_foam_dict::Entry::dict_mut) {
                    bf.rename(patch, to);
                }
                Ok(())
            })?;
            let parsed =
                Field::parse_with(&out, &limits).map_err(|source| Error::Field { path: fpath.clone(), source })?;
            let summary = summarise(name, fpath.clone(), &parsed);
            if let Some(f) = self.regions.get_mut(r).and_then(|reg| reg.fields.iter_mut().find(|f| f.path == fpath)) {
                *f = summary;
            }
            report.fields.push(fpath);
        }
        // Mapped patches of other regions that name it.
        let others: Vec<Partner> = self
            .regions
            .iter()
            .enumerate()
            .flat_map(|(i, reg)| {
                let own = reg.name.clone();
                reg.patches.iter().flatten().filter_map(move |p| {
                    let get =
                        |k: &str| p.params.iter().find(|(x, _)| x == k).map(|(_, v)| v.trim_matches('"').to_string());
                    // Without a region the partner is in the patch's own region (cyclic,
                    // nonConformal, a mapped patch within one region).
                    let nbr_region = get("neighbourRegion").or_else(|| get("sampleRegion")).unwrap_or(own.clone());
                    let mut keys: Vec<(String, String)> = Vec::new();
                    if nbr_region == region {
                        keys.extend(
                            ["neighbourPatch", "samplePatch"]
                                .iter()
                                .filter(|k| get(k).as_deref() == Some(patch))
                                .map(|k| (k.to_string(), to.to_string())),
                        );
                    }
                    // A non-conformal patch's original patch is in its own region.
                    if i == r && get("originalPatch").as_deref() == Some(patch) {
                        keys.push(("originalPatch".to_string(), to.to_string()));
                    }
                    let itself = i == r && p.name == patch;
                    (!keys.is_empty() && !itself).then(|| (i, p.name.clone(), keys))
                })
            })
            .collect();
        for (i, other, keys) in others {
            let Some(oname) = self.regions.get(i).map(|x| x.name.clone()) else { continue };
            let opath = self.mesh_file(fs, &umcf_foam_mesh::mesh_dir(&oname), "boundary");
            let out = self.rewrite_file(fs, &opath, |doc| {
                if let Some((_, dict)) = umcf_foam_mesh::boundary_patch_mut(doc, &other) {
                    for (k, v) in &keys {
                        dict.set(&[k.as_str()], v)?;
                    }
                }
                Ok(())
            })?;
            let parsed = self.reparse_boundary(&opath, &out)?;
            if let Some(reg) = self.regions.get_mut(i) {
                reg.patches = Some(parsed);
            }
            report.partners.push(format!("{oname}/{other}"));
        }
        // 0.orig: the region's field files there, as at the start time (preparing the case
        // copies them to it).
        let orig = if region.is_empty() { "0.orig".to_string() } else { format!("0.orig/{region}") };
        // A directory that is missing lists as empty; one that can't be read is said so.
        let listed = fs.list(&orig).unwrap_or_else(|e| {
            report.not_renamed.push((orig.clone(), format!("it can't be listed ({e})")));
            Vec::new()
        });
        for name in listed {
            let path = format!("{orig}/{name}");
            if !fs.is_file(&path) {
                continue;
            }
            match self.rename_entry(fs, &path, Some("boundaryField"), patch, to) {
                Renamed::Yes => report.orig.push(path),
                Renamed::No => {}
                Renamed::TooLarge => report.not_renamed.push((path, "it is too large to be read here".to_string())),
                Renamed::Failed(why) => report.not_renamed.push((path, why)),
            }
        }
        // The view factor dictionary: an entry per patch.
        let vf = |dir: &str| {
            if region.is_empty() { format!("{dir}/viewFactorsDict") } else { format!("{dir}/{region}/viewFactorsDict") }
        };
        for plain in [vf("system"), vf("constant")] {
            // Plain or compressed, put by a command and not saved yet, or on disk. A directory
            // that can't be listed is said so, not taken as having none.
            let gz = format!("{plain}.gz");
            let dir = plain.rsplit_once('/').map_or("", |(d, _)| d).to_string();
            let listed = match fs.list(&dir) {
                Ok(names) => names,
                Err(e) => {
                    report.not_renamed.push((dir, format!("it can't be listed ({e})")));
                    continue;
                }
            };
            let there = |p: &str| {
                self.raw.contains_key(p)
                    || self.dict(p).is_some()
                    || (p.rsplit_once('/').is_some_and(|(_, n)| listed.iter().any(|l| l == n)) && fs.is_file(p))
            };
            let Some(path) = [plain, gz].into_iter().find(|p| there(p)) else { continue };
            match self.rename_entry(fs, &path, None, patch, to) {
                Renamed::Yes => report.others.push(path),
                Renamed::No => {}
                Renamed::TooLarge => report.not_renamed.push((path, "it is too large to be read here".to_string())),
                Renamed::Failed(why) => report.not_renamed.push((path, why)),
            }
        }
        // Results in other time directories are not read: they can be large, and each edit is
        // kept whole for undo.
        let start = self.start.as_ref().map(|t| t.name.clone());
        for t in umcf_foam_field::times::time_dirs(fs, "").unwrap_or_default() {
            let dir = if region.is_empty() { t.name.clone() } else { format!("{}/{region}", t.name) };
            if Some(&t.name) != start.as_ref() && fs.is_dir(&dir) {
                report.times.push(t.name);
            }
        }
        (report.mentions, report.more_mentions, report.unsearched) = self.mentions(fs, patch);
        Ok(report)
    }

    /// Renames the entry `from` to `to` in the dictionary `inside` of the file at `path` (the
    /// file's top level when `None`, where the entry must be a dictionary), keeping every other
    /// byte, and puts it when it had one. Refused when `to` is there already or `from` twice.
    fn rename_entry(&mut self, fs: &dyn CaseFs, path: &str, inside: Option<&str>, from: &str, to: &str) -> Renamed {
        // Whether the dictionary has `from` once (as a dictionary at the top level) and no `to`.
        let check = |doc: &umcf_foam_dict::Document| -> std::result::Result<bool, String> {
            let dict = match inside {
                Some(k) => match doc.root.get(k).and_then(umcf_foam_dict::Entry::dict) {
                    Some(d) => d,
                    None => return Ok(false),
                },
                None => &doc.root,
            };
            let n = dict.keys().filter(|k| *k == from).count();
            if n == 0 || (inside.is_none() && dict.get(from).and_then(umcf_foam_dict::Entry::dict).is_none()) {
                return Ok(false);
            }
            if n > 1 {
                return Err(format!("it has two entries `{from}`; keep one"));
            }
            if dict.keys().any(|k| k == to) {
                return Err(format!("it has an entry `{to}` already"));
            }
            Ok(true)
        };
        let rename = |doc: &mut umcf_foam_dict::Document| {
            let dict = match inside {
                Some(k) => doc.root.get_mut(k).and_then(umcf_foam_dict::Entry::dict_mut),
                None => Some(&mut doc.root),
            };
            dict.is_some_and(|d| d.rename(from, to))
        };
        // One of the case's dictionaries: edited where it is held.
        if let Some(doc) = self.dict(path) {
            match check(doc) {
                Ok(false) => return Renamed::No,
                Err(why) => return Renamed::Failed(why),
                Ok(true) => {}
            }
            let Ok(doc) = self.dict_mut(path) else { return Renamed::No };
            if !rename(doc) {
                return Renamed::No;
            }
            self.edited(path);
            return Renamed::Yes;
        }
        // Another file: read no more than can be edited.
        let cap = self.edit_cap();
        let (bytes, base) = if self.raw.contains_key(path) || self.saved_over(path).is_some() {
            match self.edit_base(fs, path) {
                Ok((Some(b), base)) => (b, base),
                Ok((None, _)) => return Renamed::No,
                Err(e) => return Renamed::Failed(e.to_string()),
            }
        } else {
            match fs.read(path, cap) {
                Ok(Some(b)) => {
                    let f = fingerprint(&b);
                    (Arc::new(b), Some(f))
                }
                Ok(None) => return Renamed::No,
                Err(FsError::TooLarge { .. }) => return Renamed::TooLarge,
                Err(e) => return Renamed::Failed(e.to_string()),
            }
        };
        if bytes.len() as u64 > cap {
            return Renamed::TooLarge;
        }
        let limits = umcf_foam_field::Limits { max_bytes: cap, ..Default::default() };
        let mut verdict = Ok(false);
        let out = umcf_foam_field::rewrite(&bytes, &limits, |doc| {
            verdict = check(doc);
            if verdict == Ok(true) && !rename(doc) {
                verdict = Ok(false);
            }
            Ok(())
        });
        match (out, verdict) {
            (Ok(out), Ok(true)) => match self.put_based(path, out, base) {
                Ok(()) => Renamed::Yes,
                Err(e) => Renamed::Failed(e.to_string()),
            },
            (_, Err(why)) => Renamed::Failed(why),
            (Err(umcf_foam_field::Error::TooLarge { .. }), _) => Renamed::TooLarge,
            // Not a dictionary this can read (a note, a binary file): it is not a field.
            (Err(_), _) => {
                let text = String::from_utf8_lossy(&bytes);
                if bytes.starts_with(&[0x1f, 0x8b]) {
                    Renamed::Failed("it is compressed and can't be read here".to_string())
                } else if (inside.is_none() || text.contains("boundaryField")) && text.contains(from) {
                    Renamed::Failed("it can't be edited here".to_string())
                } else {
                    Renamed::No
                }
            }
            (Ok(_), Ok(false)) => Renamed::No,
        }
    }

    /// Where `word` is written, as a whole word, in the case's dictionaries under `system`,
    /// `constant` (not the meshes, the surfaces nor mapped boundary data) and `0.orig`: (file,
    /// line), at most [`MAX_MENTIONS`], and whether there are more. The directory of the data
    /// mapped onto a patch of that name (`constant/<region>/boundaryData/<patch>`) is listed
    /// first, with line 0; then the lines, by file. Compressed, binary and files over 4 MB are
    /// passed over; the files that couldn't be read are returned third.
    pub fn mentions(&self, fs: &dyn CaseFs, word: &str) -> (Vec<(String, usize)>, bool, Vec<String>) {
        let mut out = Vec::new();
        let mut unread = Vec::new();
        if word.is_empty() {
            return (out, false, unread);
        }
        // The files there when the case was opened, and those commands made since.
        let skipped =
            |p: &str| p.split('/').any(|c| matches!(c, "polyMesh" | "triSurface" | "geometry" | "boundaryData"));
        let searched =
            |p: &str| ["system", "constant", "0.orig"].iter().any(|d| p == *d || p.starts_with(&format!("{d}/")));
        // Directories under those whose listing failed when the case was opened: not searched.
        // The case root itself: none of them was searched.
        if self.paths.contains("\u{2}") {
            unread.extend(["system", "constant", "0.orig"].map(String::from));
        }
        for p in &self.paths {
            if let Some(dir) = p.strip_suffix("/\u{2}")
                && searched(dir)
                && !skipped(dir)
            {
                unread.push(dir.to_string());
            }
        }
        // The files there when the case was opened, the dictionaries the case holds, and the
        // files commands made since.
        let mut files: Vec<String> = self
            .paths
            .iter()
            .chain(self.files.keys())
            .chain(self.raw.keys())
            .filter(|p| !p.contains('\u{1}') && !p.contains('\u{2}'))
            .filter(|p| ["system/", "constant/", "0.orig/"].iter().any(|d| p.starts_with(d)))
            .cloned()
            .collect();
        files.sort();
        files.dedup();
        let push = |out: &mut Vec<(String, usize)>, at: (String, usize)| {
            if out.len() > MAX_MENTIONS {
                return false;
            }
            out.push(at);
            true
        };
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        // First the data mapped onto the patch, found by its name: `boundaryData/<patch>/…`.
        let mut named_dirs = std::collections::BTreeSet::new();
        for path in &files {
            let parts: Vec<&str> = path.split('/').collect();
            if let Some(k) = parts.windows(2).position(|w| w == ["boundaryData", word]).map(|k| k + 1)
                && k + 1 < parts.len()
            {
                named_dirs.insert(parts.get(..=k).unwrap_or_default().join("/"));
            }
        }
        for d in named_dirs {
            push(&mut out, (d, 0));
        }
        for path in files {
            if out.len() > MAX_MENTIONS {
                break;
            }
            if self.is_dir(&path) || skipped(&path) || path.ends_with(".gz") {
                continue;
            }
            // A dictionary the case holds as it is now; another file as put, or from the disk
            // no larger than is searched.
            let bytes = match self.dict(&path) {
                Some(doc) => Arc::new(doc.to_string().into_bytes()),
                None if self.raw.contains_key(&path) || self.saved_over(&path).is_some() => {
                    match self.file(fs, &path) {
                        Ok(Some(b)) => b,
                        Ok(None) => continue,
                        Err(_) => {
                            unread.push(path);
                            continue;
                        }
                    }
                }
                None => match fs.read(&path, MENTION_BYTES) {
                    Ok(Some(b)) => Arc::new(b),
                    // Gone since the case was opened, or larger than is searched.
                    Ok(None) | Err(FsError::TooLarge { .. }) => continue,
                    Err(_) => {
                        unread.push(path);
                        continue;
                    }
                },
            };
            if bytes.len() as u64 > MENTION_BYTES || bytes.contains(&0) {
                continue;
            }
            for (n, line) in bytes.split(|b| *b == b'\n').enumerate() {
                let found = line.windows(word.len()).enumerate().any(|(i, w)| {
                    w == word.as_bytes()
                        && (i == 0 || !line.get(i - 1).copied().is_some_and(is_word))
                        && !line.get(i + word.len()).copied().is_some_and(is_word)
                });
                if found && !push(&mut out, (path.clone(), n + 1)) {
                    break;
                }
            }
        }
        let more = out.len() > MAX_MENTIONS;
        out.truncate(MAX_MENTIONS);
        (out, more, unread)
    }

    /// Whether `to` can be a new patch name in region `r` (with `patches`): not a patch, a
    /// group, a number, nor a field entry already.
    fn name_free(&self, r: usize, patches: &[Patch], to: &str) -> Result<()> {
        let region = self.regions.get(r).map_or("", |reg| reg.name.as_str());
        if patches.iter().any(|p| p.name == to) {
            return Err(Error::Bc(format!("region `{region}` has a patch `{to}` already")));
        }
        if patches.iter().any(|p| p.groups.iter().any(|g| g == to)) {
            return Err(Error::Bc(format!("`{to}` is a patch group of region `{region}`")));
        }
        if to.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::Bc(format!("`{to}` is a number, not a patch name")));
        }
        // A field entry by that name (a group, a pattern, a leftover) would take the patch's
        // entry over, or merge with it.
        let taken = self
            .regions
            .get(r)
            .into_iter()
            .flat_map(|reg| &reg.fields)
            .find(|f| f.bcs.iter().flatten().any(|b| b.patch == to));
        if let Some(f) = taken {
            return Err(Error::Bc(format!("{} has an entry `{to}` already", f.path)));
        }
        Ok(())
    }

    /// Prepares a new patch of a region's mesh from some of its boundary faces, for OpenFOAM's
    /// `createPatch` to make, once, on this mesh: the faces go into a face set
    /// (`constant/<region>/polyMesh/sets/<name>`), and the patch into the dictionary of the
    /// patches prepared so ([`pending_dict`], made when there is none) with `constructFrom
    /// set`. The case's own `createPatchDict`, which its scripts may run after meshing, is not
    /// touched: face labels hold for one mesh only. The faces may not be in a patch prepared
    /// there already. The mesh itself doesn't change until `createPatch` has run. When all the
    /// faces come from one patch, each field of the start time whose own entry for that patch
    /// holds no data lists gets a copy of it for the new patch (OpenFOAM reads an entry for a
    /// patch the mesh doesn't have yet without a word), unless the new patch is of a constraint
    /// type. [`Case::save`] writes it all.
    pub fn create_patch(&mut self, fs: &dyn CaseFs, region: &str, new: &NewPatch) -> Result<CreateReport> {
        use umcf_foam_dict::{Entry, Item, Node};
        // The field files get the new patch's entry: they must be read to get it.
        self.fields_read()?;
        let r = self
            .regions
            .iter()
            .position(|r| r.name == region)
            .ok_or_else(|| Error::Bc(format!("the case has no region `{region}`")))?;
        let patches = self
            .regions
            .get(r)
            .and_then(|r| r.patches.clone())
            .ok_or_else(|| Error::Bc(format!("region `{region}` has no mesh")))?;
        let words = [new.name, new.kind]
            .into_iter()
            .chain(new.groups.iter().copied())
            .chain(new.entries.iter().flat_map(|(k, v)| [*k, *v]));
        if let Some(w) = words.into_iter().find(|w| w.contains(umcf_foam_field::PLACEHOLDER)) {
            return Err(Error::Bc(format!("`{w}` can't be written into createPatchDict")));
        }
        if COUPLED.contains(&new.kind) {
            return Err(Error::Bc(format!(
                "a `{}` patch is paired with another: make both with createPatch's own dictionary",
                new.kind
            )));
        }
        self.name_free(r, &patches, new.name)?;
        let mut faces = new.faces.to_vec();
        faces.sort_unstable();
        faces.dedup();
        if faces.is_empty() {
            return Err(Error::Bc("a new patch needs faces".to_string()));
        }
        let dir = umcf_foam_mesh::mesh_dir(region);
        let set = format!("{dir}/sets/{}", new.name);
        let gz = format!("{set}.gz");
        if [&set, &gz].iter().any(|p| self.raw.contains_key(p.as_str()) || fs.is_file(p)) {
            return Err(Error::Bc(format!("{set} is there already")));
        }
        // The patches prepared already, and their faces, on a mesh of this many faces.
        let mesh_faces = patches.iter().map(|p| p.start_face + p.n_faces).max().unwrap_or(0);
        let dict = pending_dict(region);
        let dict_err = |source| Error::Dict { path: dict.clone(), source };
        let mut doc = match self.dict(&dict) {
            Some(doc) => doc.clone(),
            None if fs.is_file(&dict) => {
                return Err(Error::Bc(format!(
                    "{dict} is there but wasn't read as a dictionary: it can't be added to"
                )));
            }
            None => {
                let object = if region.is_empty() { "system".to_string() } else { format!("system/{region}") };
                Document::parse(&format!(
                    "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"{object}\";\n    object      createPatchDict;\n}}\n\n// The patches umcfGui prepared on this mesh, for one run of\n//     {}\n// The face sets hold face labels of this mesh: delete this file once it has run.\n{STAMP}{mesh_faces} faces\n\npointSync false;\n\npatches\n(\n);\n",
                    create_patch_command(region, self.layout.version)
                ))
                .map_err(dict_err)?
            }
        };
        let mut report = CreateReport { new_dict: self.dict(&dict).is_none(), ..Default::default() };
        // Prepared on another mesh: its face labels are not this mesh's.
        let text = doc.to_string();
        match text.split(STAMP).nth(1).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse::<usize>().ok()) {
            Some(n) if n == mesh_faces => {}
            Some(n) => {
                return Err(Error::Bc(format!(
                    "{dict} was prepared on a mesh of {n} faces, and this one has {mesh_faces}: run it on that mesh or delete it (and its face sets) first"
                )));
            }
            None => {
                return Err(Error::Bc(format!(
                    "{dict} doesn't say the mesh it was prepared on (its `{}` line is gone): run it or delete it (and its face sets) first",
                    STAMP.trim()
                )));
            }
        }
        let mut pending: Vec<(String, Vec<u32>)> = Vec::new();
        match doc.get(&["patches"]).map(|e| e.value.as_slice()) {
            Some([Node::List { items, .. }]) => {
                for item in items {
                    let Node::Dict { body, .. } = item else { continue };
                    let get = |k: &str| {
                        body.get(k).and_then(Entry::value_text).map(|v| umcf_foam_dict::unquote(v.trim()).to_string())
                    };
                    if get("name").as_deref() == Some(new.name) {
                        return Err(Error::Bc(format!("{dict} prepares a patch `{}` already", new.name)));
                    }
                    // The mesh has it: the dictionary has run already.
                    if let Some(name) = get("name")
                        && patches.iter().any(|p| p.name == name)
                    {
                        return Err(Error::Bc(format!(
                            "{dict} prepares `{name}`, which the mesh has already: it has run, so delete it (and its face sets) first"
                        )));
                    }
                    let Some(name) = get("name") else { continue };
                    // Made from whole patches: their faces are taken.
                    if get("constructFrom").as_deref() == Some("patches") {
                        let listed = get("patches").unwrap_or_default();
                        let names: Vec<&str> = listed
                            .trim_matches(|c| c == '(' || c == ')')
                            .split_whitespace()
                            .map(umcf_foam_dict::unquote)
                            .collect();
                        // By name or group; a pattern (or anything else) can't be told here.
                        let named = |n: &str| patches.iter().any(|p| p.name == n || in_group(p, n));
                        if let Some(n) = names.iter().find(|n| !named(n)) {
                            return Err(Error::Bc(format!(
                                "{dict} prepares `{name}` from `{n}`, which is no patch or group of this mesh: run that dictionary or delete it first"
                            )));
                        }
                        let labels: Vec<u32> = patches
                            .iter()
                            .filter(|p| names.iter().any(|n| p.name == *n || in_group(p, n)))
                            .flat_map(|p| p.faces())
                            .filter_map(|f| u32::try_from(f).ok())
                            .collect();
                        pending.push((name, labels));
                        continue;
                    }
                    let (Some("set"), Some(set_name)) = (get("constructFrom").as_deref(), get("set")) else {
                        continue;
                    };
                    let limits = umcf_foam_mesh::Limits { max_bytes: self.edit_cap(), ..Default::default() };
                    let path = format!("{dir}/sets/{set_name}");
                    let read = match self.raw.get(&path).or_else(|| self.raw.get(&format!("{path}.gz"))) {
                        Some(f) => umcf_foam_mesh::parse_set(&path, &f.bytes, &limits).map(Some),
                        None => umcf_foam_mesh::read_set(fs, &dir, &set_name, &limits),
                    };
                    let labels = match read {
                        Ok(Some(s)) => s.labels,
                        Ok(None) => {
                            return Err(Error::Bc(format!(
                                "{dict} prepares `{name}` from the face set `{set_name}`, which isn't there: run that dictionary or delete it first"
                            )));
                        }
                        Err(e) => return Err(Error::Bc(format!("{path}: {e}"))),
                    };
                    pending.push((name, labels));
                }
            }
            Some(_) => return Err(Error::Bc(format!("{dict}: `patches` is not a list"))),
            None => doc.set(&["patches"], "()").map_err(dict_err)?,
        }
        for (name, labels) in &pending {
            if let Some(f) = labels.iter().find(|f| faces.binary_search(f).is_ok()) {
                return Err(Error::Bc(format!("face {f} is in `{name}`, prepared in {dict} already")));
            }
        }
        // The faces by the patch they are in now (the faces and the patches both in order).
        let mut ranges: Vec<(usize, std::ops::Range<usize>)> = patches.iter().map(|p| p.faces()).enumerate().collect();
        ranges.sort_by_key(|(_, r)| r.start);
        let count = |labels: &[u32]| -> std::result::Result<Vec<usize>, u32> {
            let mut by_patch = vec![0usize; patches.len()];
            let mut sorted = labels.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            let mut k = 0;
            for &f in &sorted {
                while ranges.get(k).is_some_and(|(_, r)| r.end <= f as usize) {
                    k += 1;
                }
                match ranges.get(k) {
                    Some((i, r)) if r.contains(&(f as usize)) => {
                        if let Some(n) = by_patch.get_mut(*i) {
                            *n += 1;
                        }
                    }
                    _ => return Err(f),
                }
            }
            Ok(by_patch)
        };
        let by_patch =
            count(&faces).map_err(|f| Error::Bc(format!("face {f} is not a boundary face of region `{region}`")))?;
        let mut taken = vec![0usize; patches.len()];
        for (name, labels) in &pending {
            let n = count(labels).map_err(|f| {
                Error::Bc(format!(
                    "`{name}` in {dict} has face {f}, which is not a boundary face of this mesh: delete that dictionary"
                ))
            })?;
            for (t, n) in taken.iter_mut().zip(n) {
                *t += n;
            }
        }
        for ((p, n), t) in patches.iter().zip(&by_patch).zip(&taken).filter(|((_, n), _)| **n > 0) {
            if COUPLED.contains(&p.kind.as_str()) {
                return Err(Error::Bc(format!(
                    "patch `{}` is `{}`: its faces are paired with others and can't be moved to a new patch",
                    p.name, p.kind
                )));
            }
            report.from.push((p.name.clone(), *n));
            if n + t >= p.n_faces {
                report.emptied.push(p.name.clone());
            }
        }
        // The dictionary, with the patch added.
        let mut info = format!("type {};", new.kind);
        if !new.groups.is_empty() {
            info.push_str(&format!(" inGroups List<word> {}({});", new.groups.len(), new.groups.join(" ")));
        }
        for (k, v) in new.entries {
            info.push_str(&format!(" {k} {v};"));
        }
        let item = format!("{{ name {0}; patchInfo {{ {info} }} constructFrom set; set {0}; }}", new.name);
        // OpenFOAM-8 requires it.
        if doc.get(&["pointSync"]).is_none() {
            doc.set(&["pointSync"], "false").map_err(dict_err)?;
        }
        doc.append(&["patches"], &item).map_err(dict_err)?;
        let mut text = format!(
            "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       faceSet;\n    location    \"{dir}/sets\";\n    object      {};\n}}\n\n{}\n(\n",
            new.name,
            faces.len()
        );
        for f in &faces {
            let _ = writeln!(text, "{f}");
        }
        text.push_str(")\n");
        self.put_file(fs, &set, text.into_bytes())?;
        let key = self.resolve(&dict).to_string();
        self.files.insert(key, Arc::new(doc));
        report.set = set;
        report.dict = dict;
        // The fields of the start time.
        let sources: Vec<&str> = report.from.iter().map(|(p, _)| p.as_str()).collect();
        let constraint = is_constraint_type(new.kind);
        let one = match sources.as_slice() {
            [p] if !constraint
                && patches.iter().any(|q| q.name == *p && condition_kind(&q.kind) == condition_kind(new.kind)) =>
            {
                Some(p.to_string())
            }
            _ => None,
        };
        // Its groups, with the ones OpenFOAM gives a patch of its type (`wall` for every wall
        // type, the type for a constraint type).
        let mut groups: Vec<&str> = new.groups.to_vec();
        if is_wall_type(new.kind) {
            groups.push("wall");
        }
        if constraint {
            groups.push(new.kind);
        }
        let fields: Vec<FieldInfo> = self.regions.get(r).map_or(Vec::new(), |reg| reg.fields.clone());
        let limits = umcf_foam_field::Limits { max_bytes: self.options.field_bytes, ..Default::default() };
        for FieldInfo { name, path: fpath, bcs, indirect, too_big, .. } in fields {
            let Some(bcs) = bcs else {
                report.unread.push(fpath);
                continue;
            };
            if bcs.iter().any(|b| b.lists && sources.contains(&b.patch.as_str())) {
                report.resized.push(fpath.clone());
            }
            let copy = one.as_deref().and_then(|p| bcs.iter().rfind(|b| b.patch == p && !b.pattern));
            let Some(source) = copy.filter(|b| !b.lists).map(|b| b.patch.clone()) else {
                // Covered when OpenFOAM gives it a condition itself (`empty`) or an entry for one
                // of its groups does; maybe when a pattern or text from elsewhere does.
                let covered =
                    new.kind == "empty" || bcs.iter().any(|b| !b.pattern && groups.contains(&b.patch.as_str()));
                if !covered {
                    if indirect || bcs.iter().any(|b| b.pattern) {
                        report.maybe.push(fpath);
                    } else {
                        report.unfilled.push(fpath);
                    }
                }
                continue;
            };
            // Read and judged, but too big to edit: the copy is for the user to make.
            if too_big {
                report.unread.push(fpath);
                continue;
            }
            let out = self.rewrite_file(fs, &fpath, |doc| {
                let unsupported = |m: String| umcf_foam_field::Error::Unsupported(m);
                let entry =
                    doc.root.get_mut("boundaryField").ok_or_else(|| unsupported("no boundaryField".to_string()))?;
                let Some(Node::Dict { body: bf, close, .. }) = entry.value.first_mut() else {
                    return Err(unsupported("boundaryField is not a dictionary".to_string()));
                };
                let at = bf
                    .items
                    .iter()
                    .rposition(|i| matches!(i, Item::Entry(e) if e.key.text == source))
                    .ok_or_else(|| unsupported(format!("no entry `{source}`")))?;
                let Some(Item::Entry(e)) = bf.items.get(at) else {
                    return Err(unsupported(format!("no entry `{source}`")));
                };
                let mut e = e.clone();
                e.key.text = new.name.to_string();
                // Indented like the source.
                let indent: String = match e.key.lead.rsplit_once('\n') {
                    Some((_, last)) => last.chars().take_while(|c| *c == ' ' || *c == '\t').collect(),
                    None => "    ".to_string(),
                };
                // On a line of its own after the source, without the comments before that one. A
                // comment on the rest of the source's last line stays with the source, unless a
                // block comment goes on from there: then the copy goes before it.
                let next = match bf.items.get_mut(at + 1) {
                    Some(item) => &mut item.key_tok_mut().lead,
                    None => &mut close.lead,
                };
                let split = next.find('\n').filter(|&i| !opens_block_comment(next.get(..i).unwrap_or("")));
                e.key.lead = match split {
                    Some(i) => {
                        let same = next.get(..i).unwrap_or("").to_string();
                        *next = next.get(i..).unwrap_or("").to_string();
                        format!("{same}\n{indent}")
                    }
                    None => format!("\n{indent}"),
                };
                bf.items.insert(at + 1, Item::Entry(e));
                Ok(())
            })?;
            let parsed =
                Field::parse_with(&out, &limits).map_err(|source| Error::Field { path: fpath.clone(), source })?;
            let summary = summarise(name, fpath.clone(), &parsed);
            if let Some(f) = self.regions.get_mut(r).and_then(|reg| reg.fields.iter_mut().find(|f| f.path == fpath)) {
                *f = summary;
            }
            report.fields.push(fpath);
        }
        Ok(report)
    }

    /// `dir/name`, or `dir/name.gz` when only that one is there.
    fn mesh_file(&self, fs: &dyn CaseFs, dir: &str, name: &str) -> String {
        let plain = format!("{dir}/{name}");
        let gz = format!("{plain}.gz");
        if self.raw.contains_key(&plain) || fs.is_file(&plain) || !(self.raw.contains_key(&gz) || fs.is_file(&gz)) {
            plain
        } else {
            gz
        }
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
            // A file put whole that became a link since (by hand, with the same bytes, so not a
            // conflict): the save would write the file it points to. A description read from
            // disk and edited entry by entry is a dictionary like the others: written through.
            let put = rel == description::PATH
                && self.description_put != 0
                && self.description_put != lock(&self.disk).description_saved;
            // A file put back as it was before a save of one put whole (an undo past it) is
            // written whole too; one put back before a field edit is an edit like it.
            let whole = put
                || match self.raw.get(&rel) {
                    Some(f) => f.whole,
                    None => lock(&self.disk).whole.contains(&rel),
                };
            if whole && fs.is_link(&rel) {
                return Err(Error::IsLink(rel));
            }
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
            } else if let Some((original, fp)) = self.saved_over(&rel) {
                // An undo past a saved command: the original goes back.
                let saved = lock(&self.disk).raw.get(&rel).copied().flatten();
                let on_disk = match fs.read(&rel, MAX_FILE_BYTES) {
                    Err(FsError::TooLarge { .. }) => return Err(Error::Conflict { path: rel }),
                    r => r?,
                };
                if on_disk.as_deref().map(fingerprint) != saved {
                    return Err(Error::Conflict { path: rel });
                }
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
                    let script = matches!(pending, Pending::Raw(..)) && self.raw.get(&rel).is_some_and(|f| f.script);
                    if script { fs.write_executable(&rel, &text) } else { fs.write(&rel, &text) }.map_err(Error::from)
                }
                Ok(_) | Err(FsError::TooLarge { .. }) => Err(Error::Conflict { path: rel.clone() }),
                Err(e) => Err(e.into()),
            };
            if let Err(e) = result {
                return Err(if saved.is_empty() { e } else { Error::Partial { saved, source: Box::new(e) } });
            }
            match pending {
                Pending::Dict(doc) => {
                    let mut disk = lock(&self.disk);
                    // The put behind the description on disk now (0: none, text read or put back).
                    if rel == description::PATH {
                        disk.description_saved = self.description_put;
                    }
                    disk.files.insert(rel.clone(), OnDisk { bytes: text, doc });
                }
                Pending::Raw(_, fp) => {
                    let mut disk = lock(&self.disk);
                    if self.raw.get(&rel).is_some_and(|f| f.whole) {
                        disk.whole.insert(rel.clone());
                    }
                    // What the save replaced, for a case from before the command: kept unless it
                    // is what this session last saved there (then the original is older).
                    let replaced = original.as_deref().map(|b| fingerprint(b));
                    let ours = disk.raw.get(&rel).copied().flatten();
                    if ours.is_none() || ours != replaced {
                        disk.originals.insert(rel.clone(), Original { bytes: original.clone(), fingerprint: replaced });
                    }
                    disk.raw.insert(rel.clone(), Some(fp));
                }
            }
            saved.push(rel);
        }
        Ok(saved)
    }
}

/// The most text outside its lists a field file's outline keeps at open (and reads ahead for
/// bare lists): a real field's is kilobytes, its `boundaryField`.
const OUTLINE_TEXT: u64 = 4 << 20;

/// What reading one field file at open gave.
enum Read {
    Info(FieldInfo),
    Missing,
    TooLarge(String),
    /// Listed, to be read later ([`OpenOptions::defer_fields`]).
    Later(String),
    Failed(String, String),
}

/// The start-time fields of each of `dirs` (in their order), read as outlines
/// ([`Field::outline`]: their lists' numbers aren't kept), on as many threads as the machine has,
/// 16 at most (one after the other where threads can't start: wasm). Only the text
/// outside the lists is bounded, by `field_bytes` (at most [`OUTLINE_TEXT`]): a big field still
/// has its boundary conditions.
fn read_fields(
    fs: &(dyn CaseFs + Sync),
    dirs: &[String],
    options: OpenOptions,
    progress: Option<&umcf_core::Progress>,
    notes: &mut Vec<Note>,
) -> Result<Vec<Vec<FieldInfo>>> {
    let mut jobs = Vec::new();
    for (d, dir) in dirs.iter().enumerate() {
        for name in times::fields(fs, dir)? {
            jobs.push((d, dir.as_str(), name));
        }
    }
    let read = |dir: &str, name: &str| -> Read {
        let plain = join(dir, name);
        let path = if fs.is_file(&plain) { plain.clone() } else { format!("{plain}.gz") };
        if options.defer_fields {
            return Read::Later(path);
        }
        if options.field_bytes == 0 {
            return Read::TooLarge(path);
        }
        match Field::outline(fs, &plain, &umcf_foam_field::Limits::default(), options.field_bytes.min(OUTLINE_TEXT)) {
            Ok(Some(f)) => {
                let mut info = summarise(name.to_string(), path, &f);
                info.too_big = f.size() > options.field_bytes;
                Read::Info(info)
            }
            Ok(None) => Read::Missing,
            Err(umcf_foam_field::Error::Fs(FsError::TooLarge { .. }) | umcf_foam_field::Error::TooLarge { .. }) => {
                Read::TooLarge(path)
            }
            Err(e) => Read::Failed(path, e.to_string()),
        }
    };
    if let Some(p) = progress {
        p.start(jobs.len() as u64);
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let work = || {
        let mut done = Vec::new();
        loop {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some((_, dir, name)) = jobs.get(i) else { break };
            done.push((i, read(dir, name)));
            if let Some(p) = progress {
                p.step();
            }
        }
        done
    };
    // At most 16: each may hold a few times `OUTLINE_TEXT` for a hostile file (its text, the
    // text with placeholders, what bare lists read ahead, the document), a real one's kilobytes:
    // all together about what one file whole held before (256 MB).
    let threads = std::thread::available_parallelism().map_or(1, usize::from).min(16).min(jobs.len()).max(1);
    let mut results: Vec<Option<Read>> = (0..jobs.len()).map(|_| None).collect();
    std::thread::scope(|s| {
        // This thread works too: where no other starts, it reads every file.
        let helpers: Vec<_> =
            (1..threads).filter_map(|_| std::thread::Builder::new().spawn_scoped(s, work).ok()).collect();
        let mut done = work();
        for h in helpers {
            // A reader that failed (it can't panic: no panics in this crate) leaves its
            // files unread, and they are noted below.
            done.extend(h.join().unwrap_or_default());
        }
        for (i, r) in done {
            if let Some(slot) = results.get_mut(i) {
                *slot = Some(r);
            }
        }
    });
    let mut out: Vec<Vec<FieldInfo>> = dirs.iter().map(|_| Vec::new()).collect();
    for ((d, dir, name), r) in jobs.iter().zip(results) {
        let Some(fields) = out.get_mut(*d) else { continue };
        let r = r.unwrap_or_else(|| Read::Failed(join(dir, name), "its reader stopped".to_string()));
        match r {
            Read::Info(f) => {
                if f.too_big {
                    let limit = match options.field_bytes {
                        b if b >= 1 << 20 => format!("{} MB", b >> 20),
                        b => format!("{b} bytes"),
                    };
                    notes.push(note(
                        Severity::Info,
                        &f.path,
                        format!("over {limit}: its boundary conditions are checked, not edited here"),
                    ));
                }
                fields.push(f)
            }
            Read::Missing => {}
            Read::TooLarge(path) => {
                let why = if options.field_bytes == 0 {
                    "not read (field files skipped): its boundary conditions are not checked"
                } else {
                    "too large to read its boundary conditions here"
                };
                notes.push(note(Severity::Info, &path, why));
                fields.push(FieldInfo {
                    name: name.clone(),
                    path,
                    class: None,
                    dimensions: None,
                    internal: None,
                    internal_size: None,
                    bcs: None,
                    too_big: true,
                    indirect: false,
                    boundary_field: false,
                    object: None,
                });
            }
            // Listed without its conditions until they are read.
            Read::Later(path) => fields.push(FieldInfo {
                name: name.clone(),
                path,
                class: None,
                dimensions: None,
                internal: None,
                internal_size: None,
                bcs: None,
                too_big: false,
                indirect: false,
                boundary_field: false,
                object: None,
            }),
            Read::Failed(path, e) => notes.push(note(Severity::Warning, &path, e)),
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

/// What the case keeps of field `f` read from `path`: its class, `internalField` in short and
/// size, and each `boundaryField` entry.
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
                        lists: body.is_some_and(|b| b.to_string().contains(umcf_foam_field::PLACEHOLDER)),
                        // One pass over the entries (a hostile file may have very many).
                        words: body.map_or(Vec::new(), |b| {
                            // The kept words as OpenFOAM reads them (the last of a key given twice
                            // wins), and up to MAX_WORDS others.
                            let mut kept: [Option<String>; KEPT_WORDS.len()] = Default::default();
                            let mut others = Vec::new();
                            for (k, v) in b
                                .entries()
                                .filter(|e| !matches!(e.key(), "type" | "value" | "patchType"))
                                .filter_map(|e| {
                                    let v = umcf_foam_dict::unquote(e.value_text()?.trim()).to_string();
                                    (!v.is_empty() && !v.contains(char::is_whitespace))
                                        .then(|| (e.key().to_string(), v))
                                })
                            {
                                match KEPT_WORDS.iter().position(|w| *w == k) {
                                    Some(i) => {
                                        if let Some(slot) = kept.get_mut(i) {
                                            *slot = Some(v);
                                        }
                                    }
                                    None if others.len() < MAX_WORDS => others.push((k, v)),
                                    None => {}
                                }
                            }
                            KEPT_WORDS
                                .iter()
                                .zip(kept)
                                .filter_map(|(k, v)| Some((k.to_string(), v?)))
                                .chain(others)
                                .collect()
                        }),
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
        internal_size: match f.value(&["internalField"]) {
            Some(Value::List(l)) => Some(l.len()),
            _ => None,
        },
        bcs: Some(bcs),
        too_big: false,
        indirect,
        boundary_field: entries.is_some(),
        object: f.header.object.clone(),
    }
}

#[cfg(test)]
mod tests;
