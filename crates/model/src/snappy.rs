//! Meshing from the regions' STL surfaces (gaps row 6): a region's `snappy` entry in the
//! workflow description, from which its `snappyHexMeshDict` is written.
//!
//! ```c++
//! snappy
//! {
//!     inside    ( 0 0 50 );                  // locationInMesh: a point in the region's cells
//!     surfaces
//!     {
//!         buildings { mode cut; level ( 3 5 ); layers 3; features 3; }   // file buildings.stl
//!         trees     { file trees.stl; mode refine; distance ( ( 2 4 ) ( 4 2 ) ); }
//!     }
//!     boxes     { street { min ( 0 0 0 ); max ( 100 50 10 ); level 2; } }
//!     expansion  1.2;                         // the layers' expansion ratio
//!     firstLayer 0.15;                        // the first layer's thickness; without: relative
//! }
//! ```
//!
//! A surface that `cut`s the mesh becomes a patch of the surface's name (its type from the
//! patch's role, as the blocks' patches): the mesh ends at it, on the side of the inside point
//! (buildings cut out of the air, the trees out of the vegetation), refined to its `level`, with
//! `layers` cells along it. One that `refine`s only refines the cells within its `distance`s of
//! it (the air around the trees). A box refines the cells inside it.

use std::collections::HashSet;
use std::fmt::Write as _;

use umcf_foam_dict::{Dict, Node};

use crate::blocks::{node_count, node_number, non_entries, number, stamped};
use crate::description::{atom, entry_name, list, twice, unescaped, word};
use crate::mesh_box::is_surface_file;
use crate::workflow::{MeshBuild, RootStep};

/// The highest refinement level a surface, distance or box may ask for: each level halves the
/// cells' size, so 10 is a thousandth of the background's.
pub const MAX_LEVEL: u32 = 10;

/// The most layers a surface may ask for.
pub const MAX_LAYERS: u32 = 100;

/// The most surfaces and boxes a region's `snappy` may name.
pub const MAX_ITEMS: usize = 1_000;

/// How a surface takes part in the mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The mesh ends at it: a patch of the surface's name (`refinementSurfaces`).
    Cut,
    /// Only the cells near it are refined (`refinementRegions`, `mode distance`).
    Refine,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Cut => "cut",
            Mode::Refine => "refine",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "cut" => Some(Mode::Cut),
            "refine" => Some(Mode::Refine),
            _ => None,
        }
    }
}

/// A surface of `constant/triSurface` the region's mesh is made with.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    /// Its name in the dictionary, and the patch a cutting surface becomes.
    pub name: String,
    /// The file in `constant/triSurface`: `<name>.stl` unless another is given.
    pub file: String,
    pub mode: Mode,
    /// The levels a cutting surface is refined to, the least and the most (at sharp angles).
    pub level: Option<(u32, u32)>,
    /// `(distance, level)`: the cells within each distance of the surface refined to the level,
    /// the nearest first; required for a surface that refines.
    pub distance: Vec<(f64, u32)>,
    /// Cells added along a cutting surface; 0: none.
    pub layers: u32,
    /// The level its feature edges are refined to (`surfaceFeatures` writes them to
    /// `constant/triSurface/<stem>.eMesh`); `None`: not.
    pub features: Option<u32>,
}

impl Surface {
    /// The file a surface named `name` reads unless it names another.
    pub fn default_file(name: &str) -> String {
        format!("{name}.stl")
    }

    /// Its feature edges' file, `buildings.eMesh` for `buildings.stl`; `None` for a compressed
    /// surface, whose feature file `surfaceFeatures` names otherwise.
    pub fn emesh(&self) -> Option<String> {
        let stem = self.file.strip_suffix(".stl").or_else(|| self.file.strip_suffix(".stlb"))?;
        Some(format!("{stem}.eMesh"))
    }
}

/// A box whose cells are refined.
#[derive(Debug, Clone, PartialEq)]
pub struct RefineBox {
    pub name: String,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub level: u32,
}

/// A region's `snappy` entry. See the module documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct Snappy {
    /// A point in the cells the region keeps (`locationInMesh`).
    pub inside: [f64; 3],
    pub surfaces: Vec<Surface>,
    pub boxes: Vec<RefineBox>,
    /// The layers' expansion ratio; `None`: 1.2.
    pub expansion: Option<f64>,
    /// The first layer's thickness; `None`: the layers' thicknesses are relative to the cells.
    pub first_layer: Option<f64>,
}

/// The layers' expansion ratio when none is given.
pub const EXPANSION: f64 = 1.2;

fn point_text(p: &[f64; 3]) -> String {
    format!("( {} {} {} )", number(p[0]), number(p[1]), number(p[2]))
}

fn finite3(p: &[f64; 3]) -> bool {
    p.iter().all(|x| x.is_finite())
}

impl Snappy {
    /// What is wrong with it (not with its surfaces' files), as `(at, message)`, `at` relative
    /// to the entry (`surfaces/buildings`, or empty for the entry itself).
    pub fn problems(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut put = |at: String, m: String| out.push((at, m));
        if !finite3(&self.inside) {
            put("inside".into(), "isn't a point with finite coordinates".into());
        }
        if self.surfaces.is_empty() && self.boxes.is_empty() {
            put(String::new(), "names no surface or box".into());
        }
        if self.surfaces.len() + self.boxes.len() > MAX_ITEMS {
            put(String::new(), format!("names more than {MAX_ITEMS} surfaces and boxes"));
        }
        let mut names = HashSet::new();
        for s in &self.surfaces {
            let at = format!("surfaces/{}", s.name);
            if !crate::description_edit::plain_name(&s.name) {
                put(at.clone(), "isn't a plain OpenFOAM name (letters, digits, _ . - :)".into());
            }
            if !names.insert(s.name.as_str()) {
                put(at.clone(), "is named twice".into());
            }
            if !is_surface_file(&s.file) {
                put(
                    format!("{at}/file"),
                    format!(
                        "{} isn't an STL file of constant/triSurface (a file name ending .stl, .stlb, .stl.gz or .stlb.gz)",
                        s.file
                    ),
                );
            }
            match (s.mode, s.level) {
                (Mode::Cut, None) => put(at.clone(), "cuts the mesh but has no level".into()),
                (Mode::Cut, Some((lo, hi))) if lo > hi || hi > MAX_LEVEL => put(
                    format!("{at}/level"),
                    format!("is ( least most ), the least not above the most, the most {MAX_LEVEL} at most"),
                ),
                (Mode::Refine, Some(_)) => {
                    put(format!("{at}/level"), "is for a surface that cuts: one that refines takes distance".into())
                }
                _ => {}
            }
            if s.mode == Mode::Refine && s.distance.is_empty() {
                put(at.clone(), "refines but has no distance".into());
            }
            if s.mode == Mode::Refine && s.layers > 0 {
                put(format!("{at}/layers"), "are for a surface that cuts the mesh".into());
            }
            if s.layers > MAX_LAYERS {
                put(format!("{at}/layers"), format!("are {MAX_LAYERS} at most"));
            }
            let mut last: Option<(f64, u32)> = None;
            for (d, l) in &s.distance {
                if !(d.is_finite() && *d > 0.0) || *l > MAX_LEVEL {
                    put(
                        format!("{at}/distance"),
                        format!("is ( ( distance level ) … ), each distance above 0, each level {MAX_LEVEL} at most"),
                    );
                    break;
                }
                if let Some((ld, ll)) = last
                    && (*d <= ld || *l > ll)
                {
                    put(
                        format!("{at}/distance"),
                        "lists the nearest first: the distances growing, the levels not".into(),
                    );
                    break;
                }
                last = Some((*d, *l));
            }
            if let Some(f) = s.features {
                if f > MAX_LEVEL {
                    put(format!("{at}/features"), format!("is a level, {MAX_LEVEL} at most"));
                }
                if s.emesh().is_none() {
                    put(
                        format!("{at}/features"),
                        "are for an uncompressed surface (.stl or .stlb), whose feature edges surfaceFeatures writes to <name>.eMesh".into(),
                    );
                }
            }
        }
        for b in &self.boxes {
            let at = format!("boxes/{}", b.name);
            if !crate::description_edit::plain_name(&b.name) {
                put(at.clone(), "isn't a plain OpenFOAM name (letters, digits, _ . - :)".into());
            }
            if !names.insert(b.name.as_str()) {
                put(at.clone(), "is named twice (the surfaces and boxes share their names)".into());
            }
            let ok = finite3(&b.min) && finite3(&b.max) && b.min.iter().zip(&b.max).all(|(lo, hi)| lo < hi);
            if !ok {
                put(at.clone(), "has min below max along each axis, finite".into());
            }
            if b.level > MAX_LEVEL {
                put(format!("{at}/level"), format!("is {MAX_LEVEL} at most"));
            }
        }
        if self.expansion.is_some_and(|e| !(e.is_finite() && e >= 1.0)) {
            put("expansion".into(), "is a ratio of 1 or more".into());
        }
        if self.first_layer.is_some_and(|t| !(t.is_finite() && t > 0.0)) {
            put("firstLayer".into(), "is a thickness above 0".into());
        }
        out
    }

    /// The patches the cutting surfaces make, by surface name (a surface of several solids
    /// makes `<name>_<solid>` instead: [`patch_names`]).
    pub fn cut_names(&self) -> Vec<&str> {
        self.surfaces.iter().filter(|s| s.mode == Mode::Cut).map(|s| s.name.as_str()).collect()
    }

    /// The keys of the entry with their values as the description writes them; `None` for a
    /// key it leaves out.
    pub fn parts(&self) -> Vec<(&'static str, Option<String>)> {
        let surfaces = (!self.surfaces.is_empty()).then(|| {
            let items: Vec<String> =
                self.surfaces.iter().map(|s| format!("{} {}", word(&s.name), surface_text(s))).collect();
            format!("{{ {} }}", items.join(" "))
        });
        let boxes = (!self.boxes.is_empty()).then(|| {
            let items: Vec<String> = self
                .boxes
                .iter()
                .map(|b| {
                    format!(
                        "{} {{ min {}; max {}; level {}; }}",
                        word(&b.name),
                        point_text(&b.min),
                        point_text(&b.max),
                        b.level
                    )
                })
                .collect();
            format!("{{ {} }}", items.join(" "))
        });
        vec![
            ("inside", Some(point_text(&self.inside))),
            ("surfaces", surfaces),
            ("boxes", boxes),
            ("expansion", self.expansion.map(number)),
            ("firstLayer", self.first_layer.map(number)),
        ]
    }

    /// The `snappy` entry as the description writes it, indented by `indent`.
    pub fn entry(&self, indent: &str) -> String {
        let mut s = format!("{indent}snappy\n{indent}{{\n");
        for (k, v) in self.parts() {
            if let Some(v) = v {
                let _ = writeln!(s, "{indent}    {k:<10} {v};");
            }
        }
        let _ = writeln!(s, "{indent}}}");
        s
    }

    /// The `snappyHexMeshDict` of region `region`, each cutting surface's patch of the type
    /// `type_of` gives. Generated: its header says so. Self-contained: it reads no other
    /// dictionary of the case (OpenFOAM's own mesh quality settings, by `#includeEtc`).
    pub fn dict(&self, region: &str, location: &str, type_of: &dyn Fn(&str) -> String) -> String {
        let cut: Vec<&Surface> = self.surfaces.iter().filter(|s| s.mode == Mode::Cut).collect();
        let features: Vec<(String, u32)> =
            self.surfaces.iter().filter_map(|s| Some((s.emesh()?, s.features?))).collect();
        let layered: Vec<&Surface> = cut.iter().copied().filter(|s| s.layers > 0).collect();
        let yes = |b: bool| if b { "true" } else { "false" };
        let mut s = format!(
            "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"{location}\";\n    object      snappyHexMeshDict;\n}}\n\n"
        );
        let _ = write!(
            s,
            "castellatedMesh true;\nsnap            {};\naddLayers       {};\n\ngeometry\n{{\n",
            yes(!cut.is_empty()),
            yes(!layered.is_empty())
        );
        for sf in &self.surfaces {
            let _ = writeln!(
                s,
                "    {}\n    {{\n        type triSurfaceMesh;\n        file \"{}\";\n    }}",
                word(&sf.name),
                sf.file
            );
        }
        for b in &self.boxes {
            let _ = writeln!(
                s,
                "    {}\n    {{\n        type searchableBox;\n        min  {};\n        max  {};\n    }}",
                word(&b.name),
                point_text(&b.min),
                point_text(&b.max)
            );
        }
        s.push_str("}\n\ncastellatedMeshControls\n{\n    maxLocalCells       100000;\n    maxGlobalCells      100000000;\n    minRefinementCells  10;\n    maxLoadUnbalance    0.10;\n    nCellsBetweenLevels 3;\n    resolveFeatureAngle 30;\n    allowFreeStandingZoneFaces true;\n\n    features\n    (\n");
        for (file, level) in &features {
            let _ = writeln!(s, "        {{ file \"{file}\"; level {level}; }}");
        }
        s.push_str("    );\n\n    refinementSurfaces\n    {\n");
        for sf in &cut {
            let (lo, hi) = sf.level.unwrap_or((0, 0));
            let _ = writeln!(
                s,
                "        {}\n        {{\n            level ({lo} {hi});\n            patchInfo {{ type {}; }}\n        }}",
                word(&sf.name),
                type_of(&sf.name)
            );
        }
        s.push_str("    }\n\n    refinementRegions\n    {\n");
        for sf in self.surfaces.iter().filter(|sf| !sf.distance.is_empty()) {
            let levels: Vec<String> = sf.distance.iter().map(|(d, l)| format!("({} {l})", number(*d))).collect();
            let _ = writeln!(
                s,
                "        {}\n        {{\n            mode distance;\n            levels ({});\n        }}",
                word(&sf.name),
                levels.join(" ")
            );
        }
        for b in &self.boxes {
            let _ = writeln!(
                s,
                "        {}\n        {{\n            mode inside;\n            levels ((1e15 {}));\n        }}",
                word(&b.name),
                b.level
            );
        }
        let _ = write!(s, "    }}\n\n    locationInMesh {};\n}}\n\n", point_text(&self.inside));
        let _ = write!(
            s,
            "snapControls\n{{\n    nSmoothPatch    3;\n    tolerance       2.0;\n    nSolveIter      100;\n    nRelaxIter      5;\n    nFeatureSnapIter 10;\n    explicitFeatureSnap    {};\n    implicitFeatureSnap    {};\n    multiRegionFeatureSnap false;\n}}\n\n",
            yes(!features.is_empty()),
            yes(features.is_empty())
        );
        s.push_str("addLayersControls\n{\n");
        match self.first_layer {
            Some(t) => {
                let _ = write!(
                    s,
                    "    relativeSizes       false;\n    firstLayerThickness {};\n    minThickness        {};\n",
                    number(t),
                    number(t / 10.0)
                );
            }
            None => s.push_str(
                "    relativeSizes       true;\n    finalLayerThickness 0.5;\n    minThickness        0.25;\n",
            ),
        }
        let _ = writeln!(s, "    expansionRatio      {};", number(self.expansion.unwrap_or(EXPANSION)));
        s.push_str("    layers\n    {\n");
        for sf in &layered {
            let _ = writeln!(s, "        {} {{ nSurfaceLayers {}; }}", word(&sf.name), sf.layers);
        }
        s.push_str("    }\n    featureAngle              100;\n    slipFeatureAngle          30;\n    nLayerIter                50;\n    nRelaxedIter              20;\n    nRelaxIter                5;\n    nGrow                     0;\n    nSmoothSurfaceNormals     1;\n    nSmoothNormals            3;\n    nSmoothThickness          10;\n    maxFaceThicknessRatio     0.5;\n    maxThicknessToMedialRatio 0.3;\n    minMedialAxisAngle        90;\n    nMedialAxisIter           10;\n    nBufferCellsNoExtrude     0;\n}\n\n");
        s.push_str("meshQualityControls\n{\n    #includeEtc \"caseDicts/mesh/generation/meshQualityDict.cfg\"\n}\n\nmergeTolerance 1e-6;\n\n// ************************************************************************* //\n");
        stamped(&format!("The snappy entry of region {region} in system/umcfWorkflow (docs/common-workflow.md)."), &s)
    }

    /// The `snappy` entry read from its dictionary; what can't be read is a problem at
    /// `at/<key>`. `None` when a key it needs is missing or doesn't read.
    pub(crate) fn read(body: &Dict, at: &str, problem: &mut impl FnMut(&str, String)) -> Option<Snappy> {
        let mut inside = None;
        let mut surfaces = Vec::new();
        let mut boxes = Vec::new();
        let mut expansion = None;
        let mut first_layer = None;
        let mut ok = true;
        let mut seen = HashSet::new();
        non_entries(body, at, problem);
        for e in body.entries() {
            let here = format!("{at}/{}", e.key());
            if twice(&mut seen, e, &here, problem) {
                continue;
            }
            match e.key() {
                "inside" => match list(e).and_then(three) {
                    Some(p) => inside = Some(p),
                    None => {
                        ok = false;
                        problem(&here, "is a point: ( x y z )".into());
                    }
                },
                "surfaces" | "boxes" => {
                    let Some(d) = e.dict() else {
                        ok = false;
                        problem(&here, "is a dictionary of named entries: { buildings { … } }".into());
                        continue;
                    };
                    non_entries(d, &here, problem);
                    let mut names = HashSet::new();
                    for item in d.entries() {
                        let name = entry_name(item);
                        let iat = format!("{here}/{name}");
                        if !names.insert(name.clone()) {
                            problem(&iat, "is given twice: the later ones are left out".into());
                            continue;
                        }
                        let Some(ib) = item.dict() else {
                            ok = false;
                            problem(&iat, "is a dictionary: { … }".into());
                            continue;
                        };
                        let read = if e.key() == "surfaces" {
                            read_surface(&name, ib, &iat, problem).map(|s| surfaces.push(s))
                        } else {
                            read_box(&name, ib, &iat, problem).map(|b| boxes.push(b))
                        };
                        ok &= read.is_some();
                    }
                }
                "expansion" | "firstLayer" => match e.value.as_slice() {
                    [n] if node_number(n).is_some() => {
                        if e.key() == "expansion" {
                            expansion = node_number(n);
                        } else {
                            first_layer = node_number(n);
                        }
                    }
                    _ => {
                        ok = false;
                        problem(&here, "is a number".into());
                    }
                },
                _ => problem(&here, "isn't a key of snappy: inside, surfaces, boxes, expansion or firstLayer".into()),
            }
        }
        if inside.is_none() && ok && !seen.contains("inside") {
            problem(at, "has no inside: the point in the cells the region keeps".into());
        }
        if !ok {
            return None;
        }
        Some(Snappy { inside: inside?, surfaces, boxes, expansion, first_layer })
    }
}

fn surface_text(s: &Surface) -> String {
    let mut t = String::from("{");
    if s.file != Surface::default_file(&s.name) {
        let _ = write!(t, " file {};", word(&s.file));
    }
    let _ = write!(t, " mode {};", s.mode.name());
    if let Some((lo, hi)) = s.level {
        let _ = write!(t, " level ( {lo} {hi} );");
    }
    if !s.distance.is_empty() {
        let items: Vec<String> = s.distance.iter().map(|(d, l)| format!("( {} {l} )", number(*d))).collect();
        let _ = write!(t, " distance ( {} );", items.join(" "));
    }
    if s.layers > 0 {
        let _ = write!(t, " layers {};", s.layers);
    }
    if let Some(f) = s.features {
        let _ = write!(t, " features {f};");
    }
    t.push_str(" }");
    t
}

fn three(items: &[Node]) -> Option<[f64; 3]> {
    match items {
        [a, b, c] => Some([node_number(a)?, node_number(b)?, node_number(c)?]),
        _ => None,
    }
}

fn level_of(n: &Node) -> Option<u32> {
    node_count(n).and_then(|c| u32::try_from(c).ok())
}

fn read_surface(name: &str, body: &Dict, at: &str, problem: &mut impl FnMut(&str, String)) -> Option<Surface> {
    let mut s = Surface {
        name: name.to_string(),
        file: Surface::default_file(name),
        mode: Mode::Cut,
        level: None,
        distance: Vec::new(),
        layers: 0,
        features: None,
    };
    let mut ok = true;
    let mut mode = None;
    let mut seen = HashSet::new();
    non_entries(body, at, problem);
    for e in body.entries() {
        let here = format!("{at}/{}", e.key());
        if twice(&mut seen, e, &here, problem) {
            continue;
        }
        let wrong: Option<&str> = match e.key() {
            "file" => match e.value.as_slice() {
                [n] if atom(n).is_some() => {
                    s.file = atom(n).map(unescaped).unwrap_or_default();
                    None
                }
                _ => Some("is the surface's file name in constant/triSurface"),
            },
            "mode" => match e.value.as_slice() {
                [n] if atom(n).and_then(Mode::parse).is_some() => {
                    mode = atom(n).and_then(Mode::parse);
                    None
                }
                _ => Some("is cut or refine"),
            },
            "level" => match list(e) {
                Some([a, b]) if level_of(a).is_some() && level_of(b).is_some() => {
                    s.level = level_of(a).zip(level_of(b));
                    None
                }
                _ => Some("is ( least most ): two whole numbers"),
            },
            "distance" => {
                let pairs: Option<Vec<(f64, u32)>> = list(e).and_then(|items| {
                    items
                        .iter()
                        .map(|n| match n {
                            Node::List { items, .. } => match items.as_slice() {
                                [d, l] => node_number(d).zip(level_of(l)),
                                _ => None,
                            },
                            _ => None,
                        })
                        .collect()
                });
                match pairs {
                    Some(p) if !p.is_empty() => {
                        s.distance = p;
                        None
                    }
                    _ => Some("is ( ( distance level ) … )"),
                }
            }
            "layers" | "features" => match e.value.as_slice() {
                [n] if level_of(n).is_some() => {
                    let v = level_of(n).unwrap_or_default();
                    if e.key() == "layers" {
                        s.layers = v;
                    } else {
                        s.features = Some(v);
                    }
                    None
                }
                _ => Some("is a whole number"),
            },
            _ => {
                problem(&here, "isn't a key of a surface: file, mode, level, distance, layers or features".into());
                None
            }
        };
        if let Some(m) = wrong {
            ok = false;
            problem(&here, m.to_string());
        }
    }
    match mode {
        Some(m) => s.mode = m,
        None if !seen.contains("mode") => problem(at, "has no mode: cut or refine".into()),
        None => {}
    }
    (ok && mode.is_some()).then_some(s)
}

fn read_box(name: &str, body: &Dict, at: &str, problem: &mut impl FnMut(&str, String)) -> Option<RefineBox> {
    let mut min = None;
    let mut max = None;
    let mut level = None;
    let mut ok = true;
    let mut seen = HashSet::new();
    non_entries(body, at, problem);
    for e in body.entries() {
        let here = format!("{at}/{}", e.key());
        if twice(&mut seen, e, &here, problem) {
            continue;
        }
        match e.key() {
            "min" | "max" => match list(e).and_then(three) {
                Some(p) if e.key() == "min" => min = Some(p),
                Some(p) => max = Some(p),
                None => {
                    ok = false;
                    problem(&here, "is a point: ( x y z )".into());
                }
            },
            "level" => match e.value.as_slice() {
                [n] if level_of(n).is_some() => level = level_of(n),
                _ => {
                    ok = false;
                    problem(&here, "is a whole number".into());
                }
            },
            _ => problem(&here, "isn't a key of a box: min, max or level".into()),
        }
    }
    for (k, v) in [("min", min.is_some()), ("max", max.is_some()), ("level", level.is_some())] {
        if !v && ok && !seen.contains(k) {
            problem(at, format!("has no {k}"));
            ok = false;
        }
    }
    if !ok {
        return None;
    }
    Some(RefineBox { name: name.to_string(), min: min?, max: max?, level: level? })
}

/// The `snappyHexMeshDict` a region's mesh reads: the one its `snappyHexMesh` step names.
/// `None` for a mesh without one.
pub fn dict_path(build: &MeshBuild) -> Option<String> {
    match build {
        MeshBuild::Root(steps) => steps.iter().find_map(|s| match s {
            RootStep::SnappyHexMesh(d) => Some(d.clone()),
            _ => None,
        }),
        _ => None,
    }
}

/// The patches a cutting surface makes: its name for a surface of one solid, else
/// `<name>_<solid>` for each solid (as snappyHexMesh names them); `regions` are the surface's
/// regions as OpenFOAM names them (the solids' names made into words).
pub fn patch_names(name: &str, regions: &[String]) -> Vec<String> {
    if regions.len() <= 1 { vec![name.to_string()] } else { regions.iter().map(|r| format!("{name}_{r}")).collect() }
}

#[cfg(test)]
#[path = "snappy_tests.rs"]
mod tests;
