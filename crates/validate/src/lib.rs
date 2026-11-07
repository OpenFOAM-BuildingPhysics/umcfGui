//! Checks an urbanMicroclimateFoam case before it runs.
//!
//! [`check`] looks at an opened [`Case`] and returns [`Diagnostic`]s, each with a stable
//! [`Diagnostic::code`] (documented in the user guide), the file it is about and, where there
//! is one, the region, patch and field. Nothing here reads files: everything comes from the
//! case model, so a check is instant and can run on every edit.
//!
//! What is checked (the user guide's "Checking a case" page has every code):
//! - the run controls: values OpenFOAM refuses, `endTime` after `startTime`, positive steps;
//! - every region has a mesh and, in a solver case, fields at the time the run starts from
//!   (`startFrom` resolved as OpenFOAM does, `latestTime` when it is missing);
//! - every field file has an entry for every mesh patch, picked the way OpenFOAM picks it (name,
//!   group with the implicit `wall`/`mapped`/constraint groups, `empty`, POSIX pattern);
//!   constraint patches have the matching type or a `patchType`; entries that match no patch
//!   are pointed out;
//! - `mapped*` patches that couple regions point to a region (and, unless they use no patch
//!   there, a patch) that exist, and a partner pointing back into the region points back to the
//!   same patch;
//! - the model types selected where the solver reads them (`vegetationModel`,
//!   `buildingMaterialModel`, `solarLoadModel`, …) and the urbanMicroclimateFoam boundary
//!   conditions exist in the solver for the case's OpenFOAM version.
//!
//! Standard OpenFOAM boundary conditions (`fixedValue`, `zeroGradient`, …) are not in the
//! solver catalog, so an unknown standard type is not reported.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod description;
mod fields;
mod keywords;
pub mod mesh;
mod needs;
pub mod surface;
mod utilities;
mod workflow;

use umcf_core::OfVersion;
use umcf_foam_dict::{Dict, Item, Node};
use umcf_foam_mesh::Patch;
use umcf_model::{Bc, CONTROL_DICT, Case, Controls, FieldInfo, Region, RegionKind};
use umcf_schema::{CaseKind, Severity, catalog::catalog};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// The run fails, or computes something other than what the case says.
    Error,
    /// Probably a mistake, or something the check couldn't verify.
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: Level,
    /// Stable identifier (`bc.missing`, `coupling.patch`, …): what tools and tests match on.
    pub code: &'static str,
    /// The file it is about (case-relative); empty for the case as a whole.
    pub path: String,
    pub region: Option<String>,
    pub patch: Option<String>,
    pub field: Option<String>,
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn new(level: Level, code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Diagnostic { level, code, path: path.into(), region: None, patch: None, field: None, message: message.into() }
    }

    pub(crate) fn region(mut self, r: &str) -> Self {
        self.region = Some(r.to_string());
        self
    }

    fn patch(mut self, p: &str) -> Self {
        self.patch = Some(p.to_string());
        self
    }

    fn field(mut self, f: &str) -> Self {
        self.field = Some(f.to_string());
        self
    }
}

/// All diagnostics for a case, errors first.
pub fn check(case: &Case) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    notes(case, &mut out);
    if case.fields_pending {
        let at = case.start.as_ref().map_or(String::new(), |t| t.name.clone());
        out.push(Diagnostic::new(
            Level::Info,
            "fields.pending",
            at,
            "the start-time field files are not read yet: their boundary conditions and sizes aren't checked",
        ));
    }
    controls(case, &mut out);
    let mut patterns = Patterns::default();
    for r in &case.regions {
        region(case, r, &mut patterns, &mut out);
    }
    coupling(case, &mut out);
    needs::check(case, &mut out);
    model_types(case, &mut out);
    fields::check(case, &mut out);
    keywords::check(case, &mut out);
    workflow::check(case, &mut out);
    description::check(case, &mut out);
    out.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.path.cmp(&b.path)));
    out
}

/// What opening the case reported (files that didn't read, a missing start time, porting
/// leftovers the layout found).
fn notes(case: &Case, out: &mut Vec<Diagnostic>) {
    for n in case.layout.notes.iter().chain(&case.notes) {
        let level = match n.severity {
            Severity::Warning => Level::Warning,
            Severity::Info => Level::Info,
        };
        out.push(Diagnostic::new(level, "case.note", &n.path, &n.message));
    }
    if case.layout.kind == CaseKind::Solver {
        if case.layout.version.is_none() {
            out.push(Diagnostic::new(
                Level::Warning,
                "case.version",
                CONTROL_DICT,
                "the OpenFOAM version can't be told from the case files: version-specific checks are skipped",
            ));
        } else if case.layout.mixed() {
            out.push(Diagnostic::new(
                Level::Warning,
                "case.version",
                CONTROL_DICT,
                "the case has files for both OpenFOAM-8 and OpenFOAM-12 (a partly ported case?)",
            ));
        }
    }
}

fn controls(case: &Case, out: &mut Vec<Diagnostic>) {
    let c = &case.controls;
    let n = |v: &Option<String>| Controls::number(v.as_ref());
    let diag = |level, code, message: String| Diagnostic::new(level, code, CONTROL_DICT, message);
    if c.application.is_none() {
        out.push(diag(Level::Warning, "control.application", "`application` is not set".into()));
    }
    // Values OpenFOAM refuses (Time::setControls, TimeIO.C).
    for (key, value, allowed) in [
        ("startFrom", &c.start_from, &["firstTime", "startTime", "latestTime"][..]),
        ("stopAt", &c.stop_at, &["endTime", "writeNow", "noWriteNow", "nextWrite"][..]),
    ] {
        if let Some(v) = value
            && !v.starts_with('$')
            && !allowed.contains(&v.as_str())
        {
            out.push(diag(Level::Error, "control.value", format!("{key} `{v}` is not one of {}", allowed.join(", "))));
        }
    }
    if c.stop_at.as_deref().is_none_or(|s| s == "endTime")
        && let Some(end) = n(&c.end_time)
    {
        let first = case.start.as_ref().filter(|_| c.start_from.as_deref() == Some("firstTime")).map(|t| t.value);
        if c.start_from.as_deref() == Some("startTime") || first.is_some() {
            if let Some(start) = first.or_else(|| n(&c.start_time))
                && end <= start
            {
                out.push(diag(
                    Level::Error,
                    "control.end-time",
                    format!(
                        "endTime ({end}) is not after the start time ({start}): the run stops before its first step"
                    ),
                ));
            }
        } else if let Some(t) = &case.start
            && end <= t.value
        {
            // Resuming from the latest time of a case that has reached its end time.
            out.push(diag(
                Level::Info,
                "control.finished",
                format!(
                    "the run resumes at `{}`, which is endTime ({end}) or later: it does nothing until endTime is raised{}",
                    t.name,
                    decomposed(case)
                ),
            ));
        }
    }
    for (key, value) in [("deltaT", &c.delta_t), ("writeInterval", &c.write_interval)] {
        if let Some(v) = n(value)
            && v <= 0.0
        {
            out.push(diag(Level::Error, "control.step", format!("{key} must be positive, not {v}")));
        }
    }
    if c.write_control.as_deref() == Some("timeStep")
        && let Some(v) = n(&c.write_interval)
        && (0.0..1.0).contains(&v)
    {
        out.push(diag(
            Level::Error,
            "control.step",
            format!("with writeControl timeStep, writeInterval counts time steps and must be at least 1, not {v}"),
        ));
    }
}

/// For messages about the start time of a decomposed case.
fn decomposed(case: &Case) -> &'static str {
    if case.processors > 0 { " (the case is decomposed: a parallel run resumes from processor*/ instead)" } else { "" }
}

/// Constraint patch types (those with a point patch field of their own), in OpenFOAM-8 and 12:
/// a field must use the same type on such a patch, unless its `patchType` names it.
const CONSTRAINTS: &[&str] = &[
    "empty",
    "wedge",
    "symmetryPlane",
    "symmetry",
    "cyclic",
    "cyclicAMI",
    "cyclicACMI",
    "cyclicRepeatAMI",
    "cyclicSlip",
    "nonConformalCyclic",
    "nonConformalError",
    "nonConformalProcessorCyclic",
    "processor",
    "processorCyclic",
    "internal",
];

/// The checks of one region: its field sizes, its mesh, its fields at the start time and each
/// field's boundary conditions against the mesh.
fn region(case: &Case, r: &Region, cx: &mut Patterns, out: &mut Vec<Diagnostic>) {
    let mesh = umcf_foam_mesh::mesh_dir(&r.name);
    // The sizes need only the counts, which a time directory's mesh may give without `constant`.
    for f in r.fields.iter().filter(|f| is_field(f)) {
        size(case, r, f, out);
    }
    let Some(patches) = &r.patches else {
        if r.kind != RegionKind::Single || case.layout.kind == CaseKind::Solver {
            out.push(
                Diagnostic::new(
                    Level::Warning,
                    "region.no-mesh",
                    format!("{mesh}/boundary"),
                    "no mesh: the region can't run until it is meshed (or the case is only decomposed)",
                )
                .region(&r.name),
            );
        }
        return;
    };
    // A field too large to read here is still a field.
    if case.layout.kind == CaseKind::Solver
        && case.start.is_some()
        && !r.fields.iter().any(|f| (f.bcs.is_none() && !is_copy(f)) || is_field(f))
    {
        let level = if case.processors > 0 { Level::Warning } else { Level::Error };
        out.push(
            Diagnostic::new(
                level,
                "region.no-fields",
                region_dir(case, r),
                format!(
                    "no fields at the start time `{}` ({}): a run started now fails{}",
                    case.start.as_ref().map_or("", |t| t.name.as_str()),
                    case.controls.describe_start(),
                    decomposed(case)
                ),
            )
            .region(&r.name),
        );
    }
    let mesh = Mesh::new(patches, case.layout.version, case.layout.mixed());
    for f in r.fields.iter().filter(|f| is_field(f)) {
        if let Some(o) = f.object.as_deref().map(umcf_foam_dict::unquote)
            && o != f.name
        {
            out.push(
                Diagnostic::new(
                    Level::Info,
                    "field.object",
                    &f.path,
                    format!("the header says `object {o}`; OpenFOAM reads the file as `{}` anyway", f.name),
                )
                .region(&r.name)
                .field(&f.name),
            );
        }
        field(case, r, &mesh, f, cx, out);
    }
}

/// A `nonuniform` `internalField` as long as the mesh's count for its class: cells for a
/// `vol…Field`, internal faces for a `surface…Field`, points for a `point…Field`, as the
/// `owner` header notes them. OpenFOAM stops on a length that differs (*size … is not equal to
/// the given value of …*). Only a Warning in a decomposed case, whose parallel run reads
/// `processor*/`.
fn size(case: &Case, r: &Region, f: &FieldInfo, out: &mut Vec<Diagnostic>) {
    let (Some(n), Some(class)) = (f.internal_size, f.class.as_deref()) else { return };
    let (want, what) = if class.starts_with("vol") {
        (r.counts.cells, "cells")
    } else if class.starts_with("surface") {
        (r.counts.internal_faces, "internal faces")
    } else if class.starts_with("point") {
        (r.counts.points, "points")
    } else {
        return;
    };
    let Some(want) = want else { return };
    if n as u64 != want {
        let copy = f.object.as_deref().is_some_and(|o| umcf_foam_dict::unquote(o) != f.name);
        out.push(
            Diagnostic::new(
                if copy {
                    Level::Info
                } else if case.processors > 0 {
                    Level::Warning
                } else {
                    Level::Error
                },
                "field.size",
                &f.path,
                format!("internalField lists {n} values, the mesh has {want} {what}: is the field from another mesh?"),
            )
            .region(&r.name)
            .field(&f.name),
        );
    }
}

/// A region's patches with their groups, computed once for all its fields.
struct Mesh<'a> {
    patches: &'a [Patch],
    groups: Vec<Vec<&'a str>>,
    names: std::collections::HashSet<&'a str>,
    all_groups: std::collections::HashSet<&'a str>,
}

impl<'a> Mesh<'a> {
    fn new(patches: &'a [Patch], version: Option<OfVersion>, mixed: bool) -> Self {
        let names: std::collections::HashSet<&str> = patches.iter().map(|p| p.name.as_str()).collect();
        // OpenFOAM drops a group that has the name of a patch (polyBoundaryMesh::groupPatchIDs).
        let groups: Vec<Vec<&str>> = patches
            .iter()
            .map(|p| groups(p, version, mixed).into_iter().filter(|g| !names.contains(g)).collect())
            .collect();
        let all_groups = groups.iter().flatten().copied().collect();
        Mesh { patches, groups, names, all_groups }
    }
}

fn region_dir(case: &Case, r: &Region) -> String {
    let t = case.start.as_ref().map_or("0", |t| t.name.as_str());
    if r.name.is_empty() { t.to_string() } else { format!("{t}/{}", r.name) }
}

/// How an entry was found for a patch.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Via {
    Name,
    Group,
    Pattern,
}

/// Which `boundaryField` entry OpenFOAM uses for a patch, as `GeometricBoundaryField::readField`
/// does it (the same in OpenFOAM-8 and 12). A key written twice is merged into its first
/// place, with the later values winning:
/// 1. the entry with the patch's name;
/// 2. else a group entry, trying the entries from the last one written;
/// 3. else, for an `empty` patch, none is needed;
/// 4. else a quoted key with exactly the patch's name, else the last quoted pattern that
///    matches.
enum Match {
    Entry(usize, Via),
    /// An `empty` patch without an entry: OpenFOAM gives it `empty`.
    Empty,
    None,
}

/// The groups a patch is in: its `inGroups`, plus what the patch constructors add (constraint
/// types their own type; wall types, `mappedWallVariableThickness` included, `wall`;
/// `mappedPatch` (OpenFOAM-8) and `mapped` (12) their own type, the other mapped types the one
/// of their version, both when the version is unknown or mixed; `mappedInternal` its own).
fn groups(p: &Patch, version: Option<OfVersion>, mixed: bool) -> Vec<&str> {
    let mut g: Vec<&str> = p.groups.iter().map(String::as_str).collect();
    let kind = p.kind.as_str();
    if CONSTRAINTS.contains(&kind) || matches!(kind, "mappedInternal" | "mapped" | "mappedPatch") {
        g.push(kind);
    }
    if kind == "wall" || kind.contains("Wall") {
        g.push("wall");
    }
    if kind.starts_with("mapped") && !matches!(kind, "mappedInternal" | "mapped" | "mappedPatch") {
        match (version, mixed) {
            (Some(OfVersion::V8), false) => g.push("mappedPatch"),
            (Some(OfVersion::V12), false) => g.push("mapped"),
            _ => g.extend(["mapped", "mappedPatch"]),
        }
    }
    g
}

/// One `boundaryField` key after OpenFOAM's merging (`#inputMode merge`, the default in
/// OpenFOAM-8 and 12): a key written twice, quoted or not, keeps its first place and whether
/// it was first a pattern, and each value comes from the last time it was written.
struct Merged<'a> {
    key: &'a str,
    pattern: bool,
    kind: Option<&'a str>,
    patch_type: Option<&'a str>,
    /// The compiled pattern; `None` for names and for patterns that couldn't be read here.
    re: Option<regex::Regex>,
}

fn merge(bcs: &[Bc]) -> Vec<Merged<'_>> {
    let mut out: Vec<Merged> = Vec::new();
    let mut at: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for b in bcs {
        match at.get(b.patch.as_str()).and_then(|&i| out.get_mut(i)) {
            Some(m) => {
                m.kind = b.kind.as_deref().or(m.kind);
                m.patch_type = b.patch_type.as_deref().or(m.patch_type);
            }
            None => {
                at.insert(&b.patch, out.len());
                out.push(Merged {
                    key: &b.patch,
                    pattern: b.pattern,
                    kind: b.kind.as_deref(),
                    patch_type: b.patch_type.as_deref(),
                    re: None,
                });
            }
        }
    }
    out
}

/// The condition type of field `f` on each patch of `r`, as OpenFOAM picks its entry (names,
/// then groups, then the last matching pattern; see [`entry_for`]): `None` for a patch with no
/// entry (or an `empty` one without), and nothing at all when the entries can't all be read
/// here (an `#include`, a pattern that can't be read).
pub(crate) fn kinds_by_patch<'a>(
    case: &Case,
    r: &'a Region,
    f: &'a FieldInfo,
) -> Option<Vec<(&'a str, Option<&'a str>)>> {
    let patches = r.patches.as_deref()?;
    let bcs = f.bcs.as_ref()?;
    if f.indirect {
        return None;
    }
    let mesh = Mesh::new(patches, case.layout.version, case.layout.mixed());
    let mut merged = merge(bcs);
    let mut cx = Patterns::default();
    for m in merged.iter_mut().filter(|m| m.pattern) {
        m.re = Some(cx.get(m.key).as_ref().ok()?.clone());
    }
    let names: std::collections::HashMap<&str, usize> =
        merged.iter().enumerate().filter(|(_, m)| !m.pattern).map(|(i, m)| (m.key, i)).collect();
    Some(
        patches
            .iter()
            .enumerate()
            .map(|(k, p)| {
                let kind = match entry_for(&mesh, k, &names, &merged) {
                    Match::Entry(i, _) => merged.get(i).and_then(|m| m.kind),
                    Match::Empty | Match::None => None,
                };
                (p.name.as_str(), kind)
            })
            .collect(),
    )
}

fn entry_for(mesh: &Mesh, k: usize, names: &std::collections::HashMap<&str, usize>, merged: &[Merged]) -> Match {
    let Some(patch) = mesh.patches.get(k) else { return Match::None };
    if let Some(&i) = names.get(patch.name.as_str()) {
        return Match::Entry(i, Via::Name);
    }
    // The group entry placed last wins (merged places are in first-written order).
    let groups = mesh.groups.get(k).map(Vec::as_slice).unwrap_or(&[]);
    if let Some(&i) = groups.iter().filter_map(|g| names.get(g)).max() {
        return Match::Entry(i, Via::Group);
    }
    if patch.kind == "empty" {
        return Match::Empty;
    }
    // The pattern step looks the name up as a key first (`dictionary::lookupEntryPtr`), so a
    // quoted key with exactly the patch's name wins over any pattern, also one that wouldn't
    // match as a regular expression (`"wall(1)"`).
    if let Some(i) = merged.iter().position(|m| m.pattern && m.key == patch.name) {
        return Match::Entry(i, Via::Pattern);
    }
    match merged.iter().enumerate().rev().find(|(_, m)| m.re.as_ref().is_some_and(|re| re.is_match(&patch.name))) {
        Some((i, _)) => Match::Entry(i, Via::Pattern),
        None => Match::None,
    }
}

/// Names of copies the solver never reads: `T~`, `T.orig`, `T.bak`, `T.old`, …
fn is_copy(f: &FieldInfo) -> bool {
    const COPIES: [&str; 7] = [".orig", ".bak", ".old", ".save", ".backup", ".prev", ".tmp"];
    f.name.contains('~') || COPIES.iter().any(|c| f.name.ends_with(c))
}

/// Field files: `vol*Field`, `surface*Field`, `point*Field`. Data tables in the time
/// directory (`Tambient`, read by a boundary condition) are not fields, and copies the solver
/// never reads (`T~`, `T.bak`, `T.old`, …) are not checked. OpenFOAM reads a file by its name,
/// whatever its header's `object` says.
fn is_field(f: &FieldInfo) -> bool {
    let class = f.class.as_deref().is_some_and(|c| c.ends_with("Field") && !c.starts_with("uniformDimensioned"));
    class && !is_copy(f)
}

/// Compiled patch patterns, shared by every field of a check.
#[derive(Default)]
struct Patterns(std::collections::HashMap<String, Result<regex::Regex, String>>);

impl Patterns {
    /// OpenFOAM matches quoted keys with POSIX extended regular expressions (`regExp.C`, the
    /// same in OpenFOAM-8 and 12), against the whole name. What can't be read the same way here
    /// is an error message instead: `\d`/`\D` (a literal `d` for glibc's matcher),
    /// unbalanced parentheses, and patterns too large to check quickly.
    fn get(&mut self, pattern: &str) -> &Result<regex::Regex, String> {
        self.0.entry(pattern.to_string()).or_insert_with(|| {
            if pattern.len() > 1000 {
                return Err("is too long to check".into());
            }
            // glibc's POSIX matcher knows `\w \s \b` (GNU extensions) but reads `\d` as `d`. An
            // escaped backslash (`\\d`) is a backslash and a `d`, not the class.
            let mut chars = pattern.chars();
            while let Some(c) = chars.next() {
                if c == '\\'
                    && let Some(n @ ('d' | 'D')) = chars.next()
                {
                    return Err(format!("uses `\\{n}`, which OpenFOAM's POSIX patterns don't read as a class"));
                }
            }
            let build = |p: &str| regex::RegexBuilder::new(p).size_limit(1 << 18).dfa_size_limit(1 << 18).build();
            // The bare pattern first: wrapping it would accept an unbalanced `a)|(b`.
            build(pattern)
                .and_then(|_| build(&format!("^(?:{pattern})$")))
                .map_err(|_| "can't be read as a pattern here".to_string())
        })
    }
}

fn field(case: &Case, r: &Region, mesh: &Mesh, f: &FieldInfo, cx: &mut Patterns, out: &mut Vec<Diagnostic>) {
    let patches = mesh.patches;
    // Too large to read here: opening the case already noted that.
    let Some(bcs) = &f.bcs else { return };
    // A file whose header names another field is probably a copy (`T_old` saying `object T`):
    // OpenFOAM would read it if asked, but its problems are only worth an Info.
    let copy = f.object.as_deref().is_some_and(|o| umcf_foam_dict::unquote(o) != f.name);
    let level = |l: Level| if copy && l == Level::Error { Level::Info } else { l };
    let diag =
        |l, code, message: String| Diagnostic::new(level(l), code, &f.path, message).region(&r.name).field(&f.name);
    if !f.boundary_field && !f.indirect {
        out.push(diag(Level::Error, "bc.no-boundary-field", "there is no boundaryField".into()));
        return;
    }
    let mut merged = merge(bcs);
    let mut unreadable = false;
    for m in merged.iter_mut().filter(|m| m.pattern) {
        match cx.get(m.key) {
            Ok(re) => m.re = Some(re.clone()),
            Err(why) => {
                unreadable = true;
                out.push(diag(
                    Level::Warning,
                    "bc.pattern",
                    format!("`\"{}\"` {why}: the patches it covers aren't checked", m.key),
                ));
            }
        }
    }
    let names: std::collections::HashMap<&str, usize> =
        merged.iter().enumerate().filter(|(_, m)| !m.pattern).map(|(i, m)| (m.key, i)).collect();
    // Entries may come from an `#include` (`setConstraintTypes`, whose group entries beat any
    // pattern), or from a pattern that couldn't be read here.
    let uncertain = f.indirect || unreadable;
    let mut chosen = vec![false; merged.len()];
    for (k, p) in patches.iter().enumerate() {
        let (i, via) = match entry_for(mesh, k, &names, &merged) {
            Match::Entry(i, via) => (i, via),
            Match::Empty => continue,
            Match::None => {
                let (l, also) = if uncertain {
                    (Level::Info, " here (the boundaryField has entries this check can't read)")
                } else {
                    (Level::Error, "")
                };
                out.push(diag(l, "bc.missing", format!("no entry for patch `{}`{also}", p.name)).patch(&p.name));
                continue;
            }
        };
        let Some(m) = merged.get(i) else { continue };
        if let Some(c) = chosen.get_mut(i) {
            *c = true;
        }
        if CONSTRAINTS.contains(&p.kind.as_str())
            && let Some(kind) = m.kind
            && kind != p.kind
            && !kind.starts_with(['$', '#'])
            && m.patch_type.is_none_or(|t| t != p.kind && !t.starts_with(['$', '#']))
        {
            // An included group entry would win over a group or pattern written here.
            let l = if uncertain && via != Via::Name { Level::Info } else { Level::Error };
            out.push(
                diag(
                    l,
                    "bc.constraint",
                    format!(
                        "patch `{}` is `{}` in the mesh, so its condition must be `{}` (or say `patchType {}`), not `{kind}`",
                        p.name, p.kind, p.kind, p.kind
                    ),
                )
                .patch(&p.name),
            );
        }
    }
    // Each entry's type once, not once per patch it covers.
    for (m, _) in merged.iter().zip(&chosen).filter(|(_, c)| **c) {
        if let Some(kind) = m.kind {
            let start = out.len();
            bc_type(case, r, f, m.key, kind, out);
            for d in out.iter_mut().skip(start) {
                d.level = level(d.level);
            }
        }
    }
    // An entry is in use when it covers a patch, even where another entry wins for that patch
    // (a group entry next to explicit ones, a pattern before a later one). `processor` entries
    // (by name or type, `"procBoundary.*"`) are kept for the decomposed mesh of the same case.
    for (i, m) in merged.iter().enumerate() {
        let processor = |t: &str| matches!(t, "processor" | "processorCyclic");
        let covers = processor(m.key)
            || m.kind.is_some_and(processor)
            || chosen.get(i).copied().unwrap_or(false)
            || if m.pattern {
                m.re.as_ref().is_none_or(|re| patches.iter().any(|p| re.is_match(&p.name)))
            } else {
                mesh.names.contains(m.key) || mesh.all_groups.contains(m.key)
            };
        if !covers {
            let what = if m.pattern { "pattern" } else { "entry" };
            out.push(
                diag(Level::Warning, "bc.unused", format!("{what} `{}` matches no patch of the mesh", m.key))
                    .patch(m.key),
            );
        }
    }
}

const BC_FAMILIES: [&str; 2] = ["fvPatchScalarField", "fvPatchVectorField"];

/// A boundary-condition type against the solver's own: an urbanMicroclimateFoam type must exist
/// in the case's version.
fn bc_type(case: &Case, r: &Region, f: &FieldInfo, entry: &str, kind: &str, out: &mut Vec<Diagnostic>) {
    let cat = catalog();
    let known = BC_FAMILIES.iter().find_map(|fam| cat.find(fam, kind));
    let diag = |level, code, message: String| {
        Diagnostic::new(level, code, &f.path, message).region(&r.name).patch(entry).field(&f.name)
    };
    match (known, case.layout.version) {
        (Some(t), Some(v)) if !t.in_version(v) => out.push(diag(
            Level::Error,
            "bc.version",
            format!("`{kind}` is not in urbanMicroclimateFoam for {v} (it is for {})", other(v)),
        )),
        (None, _) if kind.starts_with("compressible::CFDHAM") => out.push(diag(
            Level::Error,
            "bc.unknown",
            format!("`{kind}` is not an urbanMicroclimateFoam boundary condition{}", suggest(kind, BC_FAMILIES)),
        )),
        _ => {}
    }
}

fn other(v: OfVersion) -> OfVersion {
    match v {
        OfVersion::V8 => OfVersion::V12,
        OfVersion::V12 => OfVersion::V8,
    }
}

/// `; did you mean X?` for the closest catalog name, when one is close.
fn suggest<const N: usize>(name: &str, families: [&str; N]) -> String {
    let cat = catalog();
    let best =
        families.iter().flat_map(|f| cat.types_of(f)).map(|t| (distance(name, t.name), t.name)).min_by_key(|(d, _)| *d);
    match best {
        Some((d, n)) if d <= 3.max(name.len() / 5) => format!("; did you mean `{n}`?"),
        _ => String::new(),
    }
}

/// Edit distance, for suggestions (names are short).
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row.first().copied().unwrap_or(0);
        if let Some(first) = row.first_mut() {
            *first = i + 1;
        }
        for (j, cb) in b.iter().enumerate() {
            let above = row.get(j + 1).copied().unwrap_or(0);
            let left = row.get(j).copied().unwrap_or(0);
            let value = (prev + usize::from(ca != *cb)).min(above + 1).min(left + 1);
            prev = above;
            if let Some(cell) = row.get_mut(j + 1) {
                *cell = value;
            }
        }
    }
    row.last().copied().unwrap_or(0)
}

/// A `mapped*` patch's partner: (region, patch). OpenFOAM-12 reads
/// `neighbourRegion`/`neighbourPatch` (and still `sampleRegion`/`samplePatch`), OpenFOAM-8
/// only the `sample*` ones. A missing region means the patch's own; a missing patch, `none`
/// or an empty name means no patch.
fn partner(p: &Patch, own: &str, version: Option<OfVersion>) -> Option<(String, String)> {
    let get =
        |key: &str| p.params.iter().find(|(k, _)| k == key).map(|(_, v)| umcf_foam_dict::unquote(v.trim()).to_string());
    let either = |new: &str, old: &str| match version {
        Some(OfVersion::V8) => get(old),
        _ => get(new).or_else(|| get(old)),
    };
    let patch = either("neighbourPatch", "samplePatch").filter(|p| !p.is_empty() && p != "none")?;
    let region = either("neighbourRegion", "sampleRegion").unwrap_or_else(|| own.to_string());
    Some((region, patch))
}

/// `mappedInternal`, or a `sampleMode` that samples cells or faces anywhere (`nearestCell`,
/// `nearestOnlyCell`, `nearestFace`): any patch name the patch carries is not used.
fn samples_cells(p: &Patch) -> bool {
    p.kind == "mappedInternal"
        || p.params
            .iter()
            .any(|(k, v)| k == "sampleMode" && matches!(v.trim(), "nearestCell" | "nearestOnlyCell" | "nearestFace"))
}

fn coupling(case: &Case, out: &mut Vec<Diagnostic>) {
    // A meshing step names regions that only exist once the steps are put together.
    if case.layout.kind != CaseKind::Solver {
        return;
    }
    let version = case.layout.version;
    for r in &case.regions {
        let Some(patches) = &r.patches else { continue };
        let boundary = format!("{}/boundary", umcf_foam_mesh::mesh_dir(&r.name));
        for p in patches.iter().filter(|p| p.kind.starts_with("mapped")) {
            let diag = |level, code, message: String| {
                Diagnostic::new(level, code, &boundary, message).region(&r.name).patch(&p.name)
            };
            let param = |key: &str| {
                p.params.iter().find(|(k, _)| k == key).map(|(_, v)| umcf_foam_dict::unquote(v.trim()).to_string())
            };
            let v8 = version == Some(OfVersion::V8);
            // OpenFOAM-12 `mappedInternal` has its own base and reads no `coupleGroup`.
            let group = p.kind != "mappedInternal" && param("coupleGroup").is_some();
            let region =
                if v8 { param("sampleRegion") } else { param("neighbourRegion").or_else(|| param("sampleRegion")) };
            // A missing region is the patch's own (both versions); an empty one is not a word,
            // and OpenFOAM stops reading the boundary file. OpenFOAM-8 reads it even next to a
            // `coupleGroup`; OpenFOAM-12 then stops on the pair (below).
            if region.as_deref() == Some("") && !(group && !v8) {
                let key = if !v8 && param("neighbourRegion").is_some() { "neighbourRegion" } else { "sampleRegion" };
                out.push(diag(
                    Level::Error,
                    "coupling.region",
                    format!("`{}` has an empty `{key}`: OpenFOAM stops reading the boundary file", p.name),
                ));
                continue;
            }
            // With `coupleGroup`, OpenFOAM finds the partner (region and patch) by group.
            // OpenFOAM-12 refuses one together with partner keywords (mappedPatchBaseBase.C:
            // "Either neighbourRegion/Patch information or a coupleGroup should be specified,
            // not both"); OpenFOAM-8 ignores them.
            if group {
                let same_patch =
                    param("samePatch").is_some_and(|v| matches!(v.as_str(), "true" | "yes" | "on" | "y" | "t" | "any"));
                let key = ["neighbourRegion", "sampleRegion", "neighbourPatch", "samplePatch"]
                    .into_iter()
                    .find(|k| param(k).is_some())
                    .or(same_patch.then_some("samePatch"));
                if !v8 && let Some(k) = key {
                    out.push(diag(
                        Level::Error,
                        "coupling.group",
                        format!(
                            "`{}` has both a `coupleGroup` and `{k}`: OpenFOAM-12 stops, it takes one or the other",
                            p.name
                        ),
                    ));
                }
                continue;
            }
            if let Some(nr) = &region
                && case.region(nr).is_none()
            {
                out.push(diag(
                    Level::Error,
                    "coupling.region",
                    format!("`{}` couples to region `{nr}`, which the case doesn't have", p.name),
                ));
                continue;
            }
            // Sampling the other region's cells uses no patch there.
            if samples_cells(p) {
                continue;
            }
            let Some((nr, np)) = partner(p, &r.name, version) else { continue };
            let Some(other_patches) = case.region(&nr).and_then(|o| o.patches.as_ref()) else { continue };
            let Some(q) = other_patches.iter().find(|q| q.name == np) else {
                out.push(diag(
                    Level::Error,
                    "coupling.patch",
                    format!("`{}` couples to patch `{np}` of region `{nr}`, which has no such patch", p.name),
                ));
                continue;
            };
            // One-way sampling is normal (vegetation reads the air's patches); only a partner
            // that points back into this region, at another patch, looks like a mix-up.
            if q.kind.starts_with("mapped")
                && !samples_cells(q)
                && let Some((br, bp)) = partner(q, &nr, version)
                && br == r.name
                && bp != p.name
            {
                out.push(diag(
                    Level::Warning,
                    "coupling.not-mutual",
                    format!("`{}` couples to `{nr}/{np}`, but that one couples to `{br}/{bp}`", p.name),
                ));
            }
        }
    }
}

/// Where the solver reads its model selectors: the top level of `vegetationProperties`,
/// `grassProperties` and `solarLoadProperties` (the catalog's `dict_file`s; the solar
/// absorption and scattering selectors live in `solarLoadProperties` too), and each entry of
/// the `buildingMaterials` list in a solid region's `transportProperties`
/// (`solid/setRegionSolidFields.H`). Directly in `constant/<region>/` or `constant/`.
fn model_types(case: &Case, out: &mut Vec<Diagnostic>) {
    let cat = catalog();
    let selectors: Vec<(&str, &'static str)> =
        cat.families.iter().filter_map(|f| f.selector.map(|s| (s, f.base))).collect();
    let dict_files: Vec<&str> = cat.families.iter().filter_map(|f| f.dict_file).collect();
    let mut dirs: Vec<String> =
        case.regions.iter().filter(|r| !r.name.is_empty()).map(|r| format!("constant/{}", r.name)).collect();
    dirs.push("constant".to_string());
    // A type from a library the case loads itself is not in the catalog.
    let libs = case.dict(CONTROL_DICT).is_some_and(|d| d.get(&["libs"]).is_some());
    for rel in case.dict_paths() {
        let Some((dir, name)) = rel.rsplit_once('/') else { continue };
        if !dirs.iter().any(|d| d == dir) {
            continue;
        }
        let Some(doc) = case.dict(rel) else { continue };
        let mut found: Vec<(&'static str, String)> = Vec::new();
        if dict_files.contains(&name) {
            top_level(&doc.root, &selectors, &mut found);
        } else if name == "transportProperties"
            && let Some(materials) = doc.get(&["buildingMaterials"])
        {
            for n in &materials.value {
                if let Node::List { items, .. } = n {
                    for item in items {
                        if let Node::Dict { body, .. } = item {
                            top_level(body, &selectors, &mut found);
                        }
                    }
                }
            }
        }
        let region = dir.strip_prefix("constant/").unwrap_or("");
        for (family, value) in found {
            let diag = |level, code, message: String| {
                let d = Diagnostic::new(level, code, rel, message);
                if region.is_empty() { d } else { d.region(region) }
            };
            match (cat.find(family, &value), case.layout.version) {
                (Some(t), Some(v)) if !t.in_version(v) => out.push(diag(
                    Level::Error,
                    "model.version",
                    format!("{family} `{value}` is not in urbanMicroclimateFoam for {v} (it is for {})", other(v)),
                )),
                (Some(_), _) => {}
                (None, v) => {
                    let choices = match v {
                        Some(v) => cat.choices(family, v),
                        None => {
                            let mut all: Vec<&str> = cat.types_of(family).map(|t| t.name).collect();
                            all.sort_unstable();
                            all
                        }
                    };
                    let (level, also) = if libs {
                        (Level::Warning, " (unless one of the libraries in controlDict's `libs` adds it)")
                    } else {
                        (Level::Error, "")
                    };
                    out.push(diag(
                        level,
                        "model.unknown",
                        format!(
                            "{family} `{value}` is not one the solver has{also}{}; it offers: {}",
                            suggest(&value, [family]),
                            choices.join(", ")
                        ),
                    ));
                }
            }
        }
    }
}

/// `(family, chosen type)` for the selector entries at the top of a dictionary; written twice,
/// the last one counts, as for OpenFOAM. `$macro` values are skipped.
fn top_level(d: &Dict, selectors: &[(&str, &'static str)], found: &mut Vec<(&'static str, String)>) {
    let mut here: Vec<(&'static str, String)> = Vec::new();
    for item in &d.items {
        let Item::Entry(e) = item else { continue };
        if let Some((_, family)) = selectors.iter().find(|(s, _)| *s == e.key())
            && let Some(v) = e.value_text()
        {
            here.retain(|(f, _)| f != family);
            if !v.starts_with('$') {
                here.push((family, umcf_foam_dict::unquote(v.trim()).to_string()));
            }
        }
    }
    found.extend(here);
}

#[cfg(test)]
mod mesh_tests;
#[cfg(test)]
mod surface_tests;
#[cfg(test)]
mod tests;
