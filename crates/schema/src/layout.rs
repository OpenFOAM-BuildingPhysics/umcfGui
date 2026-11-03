//! Case layout: the OpenFOAM version a case is written for, and its regions.
//!
//! The file banners can't be trusted (cases ported to OpenFOAM-12 still say `Version: 2.2.2`),
//! so the version comes from what the files are and where they are. Each signal adds weight to
//! one version:
//!
//! | Signal | Points to | Weight |
//! |---|---|---|
//! | `regions { fluid (…); … }` in `system/controlDict` (read by the OF12 port of the solver) | OF12 | 5 |
//! | `constant/regionProperties` with `regions` (read by the OF8 solver) | OF8 | 5 |
//! | `fvModels` / `fvConstraints` in a region's `system/` or `constant/` | OF12 | 2 each |
//! | `fvOptions` in a region's `system/` or `constant/` | OF8 | 2 each |
//! | `constant/<region>/physicalProperties` (OF10+) | OF12 | 2 each |
//! | `castellatedMeshControls/insidePoint` in `snappyHexMeshDict` (OF8 doesn't know it) | OF12 | 2 |
//! | `castellatedMeshControls/locationInMesh` (OF12 still accepts it as a fallback) | OF8 | 1 |
//! | `Version: 8` / `Version: 9`–`12` in the controlDict banner | OF8 / OF12 | 1 |
//!
//! A case without a region list is a meshing step ([`CaseKind::Meshing`]): the urbanMicroclimateFoam
//! workflow builds the multi-region mesh in several single-region cases (terrain blockMesh,
//! snappyHexMesh, extruding facades/ground/roofs into solid regions) before the solver runs.
//!
//! Files that the detected version ignores (a leftover `regionProperties` or `fvOptions` in an
//! OF12 case) are reported as [`Note`]s: they are often what is left over after porting a case.

use umcf_core::{CaseFs, OfVersion};
use umcf_foam_dict::{Document, Entry, Node};

use crate::Error;

/// Dictionaries are small; anything bigger is not one.
const DICT_LIMIT: u64 = 16 << 20;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Regions {
    pub fluid: Vec<String>,
    pub solid: Vec<String>,
    pub vegetation: Vec<String>,
}

impl Regions {
    pub fn all(&self) -> impl Iterator<Item = &str> {
        self.fluid.iter().chain(&self.solid).chain(&self.vegetation).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.all().next().is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub severity: Severity,
    /// The file the note is about (case-relative), or empty for the case as a whole.
    pub path: String,
    pub message: String,
}

/// One piece of evidence for a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub path: String,
    pub points_to: OfVersion,
    pub weight: u32,
    pub why: &'static str,
}

/// What a case is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseKind {
    /// A multi-region urbanMicroclimateFoam case (it defines its regions).
    Solver,
    /// A meshing or pre-processing step: no region list.
    Meshing,
}

/// System dictionaries and the utilities that read them.
const TOOLS: &[(&str, &str)] = &[
    ("blockMeshDict", "blockMesh"),
    ("terrainBlockMesherDict", "terrainBlockMesher"),
    ("snappyHexMeshDict", "snappyHexMesh"),
    ("meshDict", "cartesianMesh (cfMesh)"),
    ("surfaceFeaturesDict", "surfaceFeatures"),
    ("extrudeMeshDict", "extrudeMesh"),
    ("refineMeshDict", "refineMesh"),
    ("topoSetDict", "topoSet"),
    ("createPatchDict", "createPatch"),
    ("changeDictionaryDict", "changeDictionary"),
    ("setFieldsDict", "setFields"),
    ("viewFactorsDict", "viewFactorsGen"),
    ("decomposeParDict", "decomposePar"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub kind: CaseKind,
    /// Utilities this case has dictionaries for (`system/` and `system/<region>/`), in
    /// pipeline order (blockMesh first, decomposePar last).
    pub tools: Vec<&'static str>,
    /// `None` when nothing points to either version.
    pub version: Option<OfVersion>,
    pub evidence: Vec<Evidence>,
    pub regions: Regions,
    /// Where the regions were read from (`system/controlDict` or `constant/regionProperties`).
    pub regions_from: Option<String>,
    pub notes: Vec<Note>,
}

impl Layout {
    pub fn score(&self, v: OfVersion) -> u32 {
        self.evidence.iter().filter(|e| e.points_to == v).map(|e| e.weight).sum()
    }

    /// Both versions have evidence: the case is partly ported.
    pub fn mixed(&self) -> bool {
        OfVersion::ALL.iter().all(|v| self.score(*v) > 0)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Note> {
        self.notes.iter().filter(|n| n.severity == Severity::Warning)
    }
}

const CONTROL_DICT: &str = "system/controlDict";
const REGION_PROPERTIES: &str = "constant/regionProperties";

pub fn detect(fs: &dyn CaseFs) -> Result<Layout, Error> {
    let mut notes = Vec::new();
    // controlDict is the one file a case can't do without: a parse error there is an error.
    let bytes = fs.read(CONTROL_DICT, DICT_LIMIT)?.ok_or(Error::NotACase)?;
    let control = Document::parse_bytes(&bytes).map_err(|source| Error::Dict { path: CONTROL_DICT.into(), source })?;
    let mut evidence = Vec::new();

    let from_control = control.get(&["regions"]).and_then(|e| regions_from_dict(e, CONTROL_DICT, &mut notes));
    if from_control.is_some() {
        evidence.push(Evidence {
            path: CONTROL_DICT.into(),
            points_to: OfVersion::V12,
            weight: 5,
            why: "regions are defined in controlDict (OpenFOAM-12 port)",
        });
    }
    let from_properties = match read_dict(fs, REGION_PROPERTIES, &mut notes)? {
        Some(doc) => doc.get(&["regions"]).and_then(|e| regions_from_list(e, REGION_PROPERTIES, &mut notes)),
        None => None,
    };
    if from_properties.is_some() {
        evidence.push(Evidence {
            path: REGION_PROPERTIES.into(),
            points_to: OfVersion::V8,
            weight: 5,
            why: "regions are defined in constant/regionProperties (OpenFOAM-8 solver)",
        });
    }

    // Regions to look at for the per-region signals: whichever list there is, or the
    // directories under system/ and constant/ when there is none.
    let candidates: Vec<String> = match (&from_control, &from_properties) {
        (Some(r), _) | (None, Some(r)) => r.all().map(str::to_string).collect(),
        (None, None) => {
            let mut dirs = Vec::new();
            for d in ["system", "constant"] {
                for n in fs.list(d)? {
                    if fs.is_dir(&format!("{d}/{n}")) && valid_region_name(&n) && n != "polyMesh" && n != "triSurface" {
                        dirs.push(n);
                    }
                }
            }
            dirs.sort();
            dirs.dedup();
            dirs
        }
    };
    for r in &candidates {
        for (dir, file, v, why) in [
            ("system", "fvOptions", OfVersion::V8, "fvOptions (OpenFOAM-8; replaced by fvModels/fvConstraints)"),
            ("constant", "fvOptions", OfVersion::V8, "fvOptions (OpenFOAM-8; replaced by fvModels/fvConstraints)"),
            ("system", "fvConstraints", OfVersion::V12, "fvConstraints (OpenFOAM-9+)"),
            ("constant", "fvConstraints", OfVersion::V12, "fvConstraints (OpenFOAM-9+)"),
            ("system", "fvModels", OfVersion::V12, "fvModels (OpenFOAM-9+)"),
            ("constant", "fvModels", OfVersion::V12, "fvModels (OpenFOAM-9+)"),
            ("constant", "physicalProperties", OfVersion::V12, "physicalProperties (OpenFOAM-10+)"),
        ] {
            let path = format!("{dir}/{r}/{file}");
            if fs.is_file(&path) {
                evidence.push(Evidence { path, points_to: v, weight: 2, why });
            }
        }
    }
    if let Some(e) = snappy_evidence(fs, &mut notes)? {
        evidence.push(e);
    }
    if let Some(v) = banner_version(fs) {
        evidence.push(Evidence {
            path: CONTROL_DICT.into(),
            points_to: v,
            weight: 1,
            why: "version in the file banner",
        });
    }

    let score = |v: OfVersion| evidence.iter().filter(|e| e.points_to == v).map(|e| e.weight).sum::<u32>();
    let (s8, s12) = (score(OfVersion::V8), score(OfVersion::V12));
    let version = match (s8, s12) {
        (0, 0) => None,
        (a, b) if b >= a => Some(OfVersion::V12),
        _ => Some(OfVersion::V8),
    };

    let (regions, regions_from) = match version {
        Some(OfVersion::V12) => match (from_control, from_properties) {
            (Some(r), _) => (r, Some(CONTROL_DICT.to_string())),
            (None, Some(r)) => {
                notes.push(warn(
                    CONTROL_DICT,
                    "OpenFOAM-12 case without `regions` in controlDict: the solver will not find its regions \
                     (constant/regionProperties is only read by the OpenFOAM-8 solver)",
                ));
                (r, Some(REGION_PROPERTIES.to_string()))
            }
            (None, None) => (Regions::default(), None),
        },
        Some(OfVersion::V8) => match (from_properties, from_control) {
            (Some(r), _) => (r, Some(REGION_PROPERTIES.to_string())),
            (None, Some(r)) => {
                notes.push(warn(
                    REGION_PROPERTIES,
                    "OpenFOAM-8 case without constant/regionProperties: the solver will not find its regions \
                     (`regions` in controlDict is only read by the OpenFOAM-12 port)",
                ));
                (r, Some(CONTROL_DICT.to_string()))
            }
            (None, None) => (Regions::default(), None),
        },
        None => (Regions::default(), None),
    };
    if regions.is_empty() && version.is_some() {
        notes.push(warn("", "no regions defined"));
    }

    // Files the detected version does not read. Weight-1 evidence (banner, `locationInMesh`)
    // is only a hint: the other version reads those files fine.
    if let Some(v) = version {
        for e in evidence.iter().filter(|e| e.points_to != v && e.weight > 1) {
            notes.push(Note {
                severity: Severity::Info,
                path: e.path.clone(),
                message: format!("{} ignores this file: {} (left over from porting?)", v, e.why),
            });
        }
    }
    for r in regions.all() {
        if !fs.is_dir(&format!("constant/{r}")) {
            notes.push(warn(&format!("constant/{r}"), &format!("region `{r}` has no constant/{r} directory")));
        }
    }

    let kind = if regions.is_empty() && regions_from.is_none() { CaseKind::Meshing } else { CaseKind::Solver };
    let tools = tools(fs, &candidates)?;
    // A meshing step has no regions by design: not a warning.
    if kind == CaseKind::Meshing {
        notes.retain(|n| n.message != "no regions defined");
    }
    Ok(Layout { kind, tools, version, evidence, regions, regions_from, notes })
}

fn warn(path: &str, message: &str) -> Note {
    Note { severity: Severity::Warning, path: path.to_string(), message: message.to_string() }
}

/// A dictionary that fails to parse doesn't stop detection: it becomes a warning.
fn read_dict(fs: &dyn CaseFs, path: &str, notes: &mut Vec<Note>) -> Result<Option<Document>, Error> {
    let Some(bytes) = fs.read(path, DICT_LIMIT)? else { return Ok(None) };
    match Document::parse_bytes(&bytes) {
        Ok(doc) => Ok(Some(doc)),
        Err(e) => {
            notes.push(warn(path, &format!("cannot be parsed: {e}")));
            Ok(None)
        }
    }
}

/// Region names become directory names: no `/`, no leading `.`, no empty names.
fn valid_region_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

fn words(node: &Node) -> Option<Vec<String>> {
    let Node::List { items, .. } = node else { return None };
    items
        .iter()
        .map(|n| match n {
            Node::Atom(t) => Some(umcf_foam_dict::unquote(&t.text).to_string()),
            _ => None,
        })
        .collect()
}

fn push_names(kind: &str, names: Vec<String>, regions: &mut Regions, path: &str, notes: &mut Vec<Note>) {
    let target = match kind {
        "fluid" => &mut regions.fluid,
        "solid" => &mut regions.solid,
        "vegetation" => &mut regions.vegetation,
        other => {
            notes.push(warn(path, &format!("unknown region kind `{other}` (expected fluid, solid or vegetation)")));
            return;
        }
    };
    for n in names {
        if valid_region_name(&n) {
            target.push(n);
        } else {
            notes.push(warn(path, &format!("invalid region name `{n}`")));
        }
    }
}

/// OpenFOAM-12 port: `regions { fluid (air); solid (a b); vegetation (veg); }`.
fn regions_from_dict(entry: &Entry, path: &str, notes: &mut Vec<Note>) -> Option<Regions> {
    let Some(dict) = entry.dict() else {
        notes.push(warn(path, "`regions` should be a dictionary: regions { fluid (…); solid (…); vegetation (…); }"));
        return None;
    };
    let mut regions = Regions::default();
    for e in dict.entries() {
        match e.value.as_slice() {
            [list] => match words(list) {
                Some(names) => push_names(e.key(), names, &mut regions, path, notes),
                None => notes.push(warn(path, &format!("regions/{}: expected a list of names", e.key()))),
            },
            _ => notes.push(warn(path, &format!("regions/{}: expected a list of names", e.key()))),
        }
    }
    Some(regions)
}

/// OpenFOAM-8: `regions ( fluid (air) solid (a b) vegetation (veg) );`.
fn regions_from_list(entry: &Entry, path: &str, notes: &mut Vec<Note>) -> Option<Regions> {
    let [Node::List { items, .. }] = entry.value.as_slice() else {
        notes.push(warn(path, "`regions` should be a list: regions ( fluid (…) solid (…) vegetation (…) );"));
        return None;
    };
    let mut regions = Regions::default();
    let mut it = items.iter();
    while let Some(kind) = it.next() {
        let (Node::Atom(kind), Some(list)) = (kind, it.next()) else {
            notes.push(warn(path, "`regions` should alternate a kind and a list of names"));
            break;
        };
        match words(list) {
            Some(names) => push_names(&kind.text, names, &mut regions, path, notes),
            None => notes.push(warn(path, &format!("regions/{}: expected a list of names", kind.text))),
        }
    }
    Some(regions)
}

/// `locationInMesh` (OF8) became `insidePoint` (OF9+) in `castellatedMeshControls`.
fn snappy_evidence(fs: &dyn CaseFs, notes: &mut Vec<Note>) -> Result<Option<Evidence>, Error> {
    const PATH: &str = "system/snappyHexMeshDict";
    let Some(doc) = read_dict(fs, PATH, notes)? else { return Ok(None) };
    let found = |key: &str| doc.get(&["castellatedMeshControls", key]).is_some();
    Ok(if found("locationInMesh") {
        Some(Evidence {
            path: PATH.into(),
            points_to: OfVersion::V8,
            weight: 1,
            why: "snappyHexMesh `locationInMesh` (OpenFOAM-8 name; OpenFOAM-12 still accepts it)",
        })
    } else if found("insidePoint") {
        Some(Evidence {
            path: PATH.into(),
            points_to: OfVersion::V12,
            weight: 2,
            why: "snappyHexMesh `insidePoint` (OpenFOAM-9+)",
        })
    } else {
        None
    })
}

fn tools(fs: &dyn CaseFs, regions: &[String]) -> Result<Vec<&'static str>, Error> {
    let mut names = fs.list("system")?;
    for r in regions {
        names.extend(fs.list(&format!("system/{r}"))?);
    }
    // `extrudeMeshDict.east`, `topoSetDict_cellZones`, … are variants of the same dictionary.
    let base = |n: &str| n.split(['.', '_']).next().unwrap_or("").to_string();
    let bases: Vec<String> = names.iter().map(|n| base(n)).collect();
    Ok(TOOLS.iter().filter(|(dict, _)| bases.iter().any(|b| b == dict)).map(|(_, tool)| *tool).collect())
}

/// `Version:  12` in the controlDict banner comment. `2.2.2` and the like say nothing.
fn banner_version(fs: &dyn CaseFs) -> Option<OfVersion> {
    let bytes = fs.read(CONTROL_DICT, DICT_LIMIT).ok()??;
    let head = String::from_utf8_lossy(bytes.get(..bytes.len().min(2048))?);
    let line = head.lines().find(|l| l.contains("Version:"))?;
    let v = line.split("Version:").nth(1)?.trim().trim_end_matches('|').trim();
    let major: u32 = v.split(['.', ' ', '-']).next()?.parse().ok()?;
    match major {
        8 => Some(OfVersion::V8),
        9..=12 => Some(OfVersion::V12),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
