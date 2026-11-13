//! A region's `snappy` entry read from a case's `snappyHexMeshDict` (`workflow.import`): what the
//! entry can hold of it (the surfaces that cut or refine with their levels, distances, layers
//! and feature levels, the boxes that refine, the inside point, the layers' expansion and first
//! thickness), with a note for each part it can't. The dictionary itself is left as it is.

use umcf_foam_dict::{Dict, Entry, Item, Node};

use crate::blocks::{node_count, node_number};
use crate::description::{atom, entry_name, list, single, unescaped};
use umcf_core::OfVersion;

use crate::snappy::{EXPANSION, MAX_DISTANCES, MAX_ITEMS, Mode, RefineBox, Snappy, Surface};

fn level(n: &Node) -> Option<u32> {
    node_count(n).and_then(|c| u32::try_from(c).ok())
}

fn point(e: &Entry) -> Option<[f64; 3]> {
    match list(e)? {
        [a, b, c] => Some([node_number(a)?, node_number(b)?, node_number(c)?]),
        _ => None,
    }
}

/// `( ( distance level ) … )`, `None` when it isn't that (or is longer than an entry may be).
fn levels(e: &Entry) -> Option<Vec<(f64, u32)>> {
    let items = list(e)?;
    if items.len() > MAX_DISTANCES {
        return None;
    }
    items
        .iter()
        .map(|n| match n {
            Node::List { items, .. } => match items.as_slice() {
                [d, l] => node_number(d).zip(level(l)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Whether `d` has an `#include`-like directive or a `$macro` of its own, which the import
/// doesn't follow.
fn includes(d: &Dict) -> bool {
    d.items.iter().any(|i| matches!(i, Item::Directive { .. } | Item::Macro { .. }))
}

/// Whether a key of a dictionary of patches is a pattern (`"walls.*"`, `"(a|b)"`) rather than a
/// name.
fn pattern(e: &Entry) -> bool {
    e.key.text.starts_with('"') || e.key.text.contains(['*', '|', '(', '[', '?'])
}

fn number_of(d: &Dict, key: &str) -> Option<f64> {
    match d.get(key)?.value.as_slice() {
        [n] => node_number(n),
        _ => None,
    }
}

/// The snappy entry `root` (a `snappyHexMeshDict`) makes, and notes on what it leaves out
/// (`path` names the dictionary in them); `Err` with the reason when it can't make one, and the
/// notes on what it left out on the way (which may be why).
/// `version` is the case's: OpenFOAM-8 reads the inside point from `locationInMesh` only.
pub fn import(
    root: &Dict,
    path: &str,
    version: Option<OfVersion>,
) -> Result<(Snappy, Vec<String>), (String, Vec<String>)> {
    let mut notes = Vec::new();
    match read(root, path, version, &mut notes) {
        Ok(sn) => Ok((sn, notes)),
        Err(why) => Err((why, notes)),
    }
}

fn read(root: &Dict, path: &str, version: Option<OfVersion>, notes: &mut Vec<String>) -> Result<Snappy, String> {
    let geometry = root.get("geometry").and_then(Entry::dict).ok_or("it has no geometry dictionary")?;
    let castellated = root
        .get("castellatedMeshControls")
        .and_then(Entry::dict)
        .ok_or("it has no castellatedMeshControls dictionary")?;
    // OpenFOAM-12 takes insidePoints, then insidePoint, then locationInMesh; OpenFOAM-8 reads
    // locationInMesh only.
    let bad_point = "its inside point isn't ( x y z ) in numbers";
    let v8 = version == Some(OfVersion::V8);
    let inside = if v8 {
        point(castellated.get("locationInMesh").ok_or("it names no inside point (locationInMesh)")?).ok_or(bad_point)?
    } else if let Some(e) = castellated.get("insidePoints") {
        match list(e) {
            Some([Node::List { items, .. }]) => match items.as_slice() {
                [a, b, c] => [node_number(a), node_number(b), node_number(c)]
                    .into_iter()
                    .collect::<Option<Vec<f64>>>()
                    .and_then(|v| v.try_into().ok())
                    .ok_or(bad_point)?,
                _ => return Err(bad_point.into()),
            },
            Some(_) => return Err("it names several inside points (insidePoints): the entry holds one".into()),
            None => return Err(bad_point.into()),
        }
    } else if let Some(e) = castellated.get("insidePoint").or_else(|| castellated.get("locationInMesh")) {
        point(e).ok_or(bad_point)?
    } else if castellated.get("locationsInMesh").is_some() {
        return Err("it names several inside points (locationsInMesh): the entry holds one".into());
    } else {
        return Err("it names no inside point (locationInMesh)".into());
    };
    if root.get("singleRegionName").and_then(single).is_some_and(|v| matches!(v.as_str(), "false" | "no" | "off")) {
        notes.push(format!(
            "{path}: singleRegionName is off, so a surface of one solid makes the patch <surface>_<solid>: the entry names it <surface>"
        ));
    }
    if castellated.get("outsidePoints").is_some() || castellated.get("outsidePoint").is_some() {
        notes.push(format!(
            "{path}: its outside points (outsidePoint, outsidePoints) aren't held by the entry: left out"
        ));
    }
    for (what, d) in [("geometry", Some(geometry)), ("castellatedMeshControls", Some(castellated))]
        .into_iter()
        .chain([("addLayersControls", root.get("addLayersControls").and_then(Entry::dict))])
    {
        if d.is_some_and(includes) {
            notes.push(format!(
                "{path}: its {what} has an #include or a $macro, which isn't followed: what it brings isn't read"
            ));
        }
    }
    let surfaces_dict = castellated.get("refinementSurfaces").and_then(Entry::dict);
    let regions_dict = castellated.get("refinementRegions").and_then(Entry::dict);
    for (what, d) in [("refinementSurfaces", surfaces_dict), ("refinementRegions", regions_dict)] {
        let keys: Vec<String> =
            d.map(|d| d.entries().filter(|e| pattern(e)).take(5).map(entry_name).collect()).unwrap_or_default();
        if !keys.is_empty() {
            notes.push(format!(
                "{path}: its {what} name surfaces by a pattern ({}), which isn't matched: what they give those surfaces is left out",
                keys.join(", ")
            ));
        }
    }
    let mut surfaces: Vec<Surface> = Vec::new();
    let mut boxes: Vec<RefineBox> = Vec::new();
    if geometry.entries().count() > MAX_ITEMS {
        return Err(format!("its geometry names more than {MAX_ITEMS} surfaces and boxes"));
    }
    for g in geometry.entries() {
        let key = entry_name(g);
        let Some(body) = g.dict() else {
            notes.push(format!("{path}: geometry {key} isn't a dictionary: left out"));
            continue;
        };
        let kind = body.get("type").and_then(single).unwrap_or_default();
        // OpenFOAM-8's form names the file by the key, the surface by `name`.
        let name = body.get("name").and_then(single).unwrap_or_else(|| key.clone());
        let refine = regions_dict.and_then(|d| d.get(&name)).and_then(Entry::dict);
        let cut = surfaces_dict.and_then(|d| d.get(&name)).and_then(Entry::dict);
        match kind.as_str() {
            "triSurfaceMesh" => {
                let file = match body.get("file") {
                    Some(e) => match e.value.as_slice() {
                        [n] => atom(n).map(unescaped),
                        _ => None,
                    },
                    // Without a file, the key names it.
                    None => Some(key.clone()),
                };
                let Some(file) = file else {
                    notes.push(format!("{path}: geometry {key}'s file isn't a name: left out"));
                    continue;
                };
                if let Some(sc) = body.get("scale") {
                    let one = match sc.value.as_slice() {
                        [n] => node_number(n) == Some(1.0),
                        _ => false,
                    };
                    if !one {
                        notes.push(format!(
                            "{path}: geometry {key} is scaled (scale), which the entry doesn't hold (geometry.scale scales the file): left out"
                        ));
                        continue;
                    }
                }
                if body.get("regions").is_some() {
                    notes.push(format!(
                        "{path}: geometry {key} renames its solids (regions): the patches are named <surface>_<solid> in the entry"
                    ));
                }
                let other: Vec<&str> = body
                    .keys()
                    .filter(|k| !matches!(*k, "type" | "file" | "name" | "regions" | "scale"))
                    .take(5)
                    .collect();
                if !other.is_empty() {
                    notes.push(format!("{path}: geometry {key}'s {} aren't held by the entry", other.join(", ")));
                }
                let mut s = Surface {
                    name: name.clone(),
                    file,
                    mode: if cut.is_some() { Mode::Cut } else { Mode::Refine },
                    level: None,
                    distance: Vec::new(),
                    layers: 0,
                    features: None,
                    solids: Vec::new(),
                };
                // A surface making zones doesn't cut the mesh: only its refinement is kept.
                let zones = cut.is_some_and(|c| c.get("faceZone").is_some() || c.get("cellZone").is_some());
                if zones {
                    notes.push(format!(
                        "{path}: surface {name} makes a faceZone or cellZone rather than cutting the mesh, which the entry doesn't hold: its zone and level are left out"
                    ));
                    s.mode = Mode::Refine;
                }
                if let Some(c) = cut.filter(|_| !zones) {
                    s.level = match c.get("level").and_then(list) {
                        Some([a, b]) => level(a).zip(level(b)),
                        _ => None,
                    };
                    if s.level.is_none() {
                        notes.push(format!(
                            "{path}: surface {name}'s level isn't ( least most ) in whole numbers: left out"
                        ));
                        continue;
                    }
                    if c.get("regions").is_some() {
                        notes.push(format!(
                            "{path}: surface {name} has levels or types per solid (regions): the entry gives each solid the surface's level, and its patch the type of its role"
                        ));
                    }
                    let other: Vec<&str> =
                        c.keys().filter(|k| !matches!(*k, "level" | "patchInfo" | "regions")).take(5).collect();
                    if !other.is_empty() {
                        notes.push(format!(
                            "{path}: surface {name}'s {} aren't held by the entry: left out",
                            other.join(", ")
                        ));
                    }
                }
                if let Some(r) = refine {
                    let mode = r.get("mode").and_then(single).unwrap_or_default();
                    match (mode.as_str(), r.get("levels").and_then(levels)) {
                        ("distance", Some(l)) if !l.is_empty() => s.distance = l,
                        _ => notes.push(format!(
                            "{path}: surface {name}'s refinement region (mode {mode}) isn't a refinement by distance in numbers: left out"
                        )),
                    }
                }
                if s.mode == Mode::Refine && s.distance.is_empty() {
                    if (cut.is_none() || zones) && refine.is_none() {
                        notes.push(format!(
                            "{path}: surface {name} neither cuts nor refines (not in refinementSurfaces or refinementRegions): left out"
                        ));
                    }
                    continue;
                }
                surfaces.push(s);
            }
            "searchableBox" => {
                let (Some(min), Some(max)) = (body.get("min").and_then(point), body.get("max").and_then(point)) else {
                    notes.push(format!("{path}: box {key}'s min and max aren't ( x y z ) in numbers: left out"));
                    continue;
                };
                // OpenFOAM-12 reads `level`, else `levels` of one pair; OpenFOAM-8 `levels`.
                let inside_levels = refine.filter(|r| r.get("mode").and_then(single).as_deref() == Some("inside"));
                let lvl = inside_levels.and_then(|r| match r.get("level").filter(|_| !v8) {
                    Some(l) => match l.value.as_slice() {
                        [n] => level(n),
                        _ => None,
                    },
                    None => match r.get("levels").and_then(levels)?.as_slice() {
                        [(_, l)] => Some(*l),
                        _ => None,
                    },
                });
                match (lvl, cut) {
                    (Some(level), None) => boxes.push(RefineBox { name: name.clone(), min, max, level }),
                    _ => notes.push(format!(
                        "{path}: box {key} isn't a refinement of the cells inside it (refinementRegions, mode inside): left out"
                    )),
                }
            }
            other => notes.push(format!(
                "{path}: geometry {key} is a {}: the entry holds STL surfaces and boxes only, left out",
                if other.is_empty() { "geometry without a type" } else { other }
            )),
        }
    }
    // The feature edges' levels, by the surfaces' .eMesh files.
    if let Some(e) = castellated.get("features") {
        let items = list(e).unwrap_or_default();
        if items.len() > MAX_ITEMS {
            return Err(format!("its features list more than {MAX_ITEMS} files"));
        }
        for n in items {
            let Node::Dict { body, .. } = n else {
                notes.push(format!("{path}: an entry of features isn't {{ file …; level …; }}: left out"));
                continue;
            };
            let file = body.get("file").and_then(|f| match f.value.as_slice() {
                [n] => atom(n).map(unescaped),
                _ => None,
            });
            // As OpenFOAM reads them: `levels` when given, else `level`.
            let level_of = match (body.get("level"), body.get("levels").and_then(levels)) {
                (Some(l), None) if body.get("levels").is_none() => match l.value.as_slice() {
                    [n] => level(n),
                    _ => None,
                },
                (_, None) => None,
                (_, Some(l)) => {
                    if l.len() > 1 || l.first().is_some_and(|x| x.0 > 0.0) {
                        notes.push(format!(
                            "{path}: the features of {} are refined by distance: the entry takes the nearest's level, at the edges",
                            file.as_deref().unwrap_or("?")
                        ));
                    }
                    l.first().map(|x| x.1)
                }
            };
            let target =
                file.as_ref().and_then(|f| surfaces.iter_mut().find(|s| s.emesh().as_deref() == Some(f.as_str())));
            match (target, level_of) {
                (Some(s), Some(l)) => s.features = Some(l),
                _ => notes.push(format!(
                    "{path}: features {} aren't the <stem>.eMesh of a surface the entry holds, with a level: left out",
                    file.as_deref().unwrap_or("without a file")
                )),
            }
        }
    }
    // The layers, by the patches each cutting surface makes.
    let layers_controls = root.get("addLayersControls").and_then(Entry::dict);
    let layers_on = root.get("addLayers").and_then(single).is_some_and(|v| matches!(v.as_str(), "true" | "yes" | "on"));
    if root.get("addLayers").is_none() && includes(root) {
        notes.push(format!("{path}: addLayers isn't given in it (an #include may give it): its layers aren't taken"));
    }
    if let Some(ld) = layers_controls.and_then(|d| d.get("layers")).and_then(Entry::dict) {
        if ld.entries().count() > MAX_ITEMS {
            return Err(format!("its layers list more than {MAX_ITEMS} patches"));
        }
        // Each cutting surface's prefix for its <name>_<solid> patches.
        let prefixes: Vec<(usize, String)> = surfaces
            .iter()
            .enumerate()
            .filter(|(_, s)| s.mode == Mode::Cut)
            .map(|(i, s)| (i, format!("{}_", s.name)))
            .collect();
        let mut per: Vec<Vec<u32>> = vec![Vec::new(); surfaces.len()];
        let mut prefixed: Vec<String> = Vec::new();
        let mut listed = false;
        for e in ld.entries() {
            let patch = entry_name(e);
            let n = e.dict().and_then(|d| d.get("nSurfaceLayers")).and_then(|x| match x.value.as_slice() {
                [n] => level(n),
                _ => None,
            });
            if pattern(e) {
                notes.push(format!("{path}: the layers of {patch} are given by a pattern: left out"));
                continue;
            }
            let own: Vec<&str> =
                e.dict().map(|d| d.keys().filter(|k| *k != "nSurfaceLayers").take(5).collect()).unwrap_or_default();
            if !own.is_empty() {
                notes.push(format!(
                    "{path}: the layers of {patch} have settings of their own ({}), which the entry doesn't hold: left out",
                    own.join(", ")
                ));
            }
            // The surface of that name, else the one whose <name>_<solid> it is.
            let at = prefixes
                .iter()
                .find(|(i, _)| surfaces.get(*i).is_some_and(|s| s.name == patch))
                .or_else(|| prefixes.iter().filter(|(_, p)| patch.starts_with(p.as_str())).max_by_key(|(_, p)| p.len()))
                .map(|(i, _)| *i);
            let by_prefix = at.is_some_and(|i| surfaces.get(i).is_some_and(|s| s.name != patch));
            match (at.and_then(|i| per.get_mut(i)), n) {
                (Some(v), Some(n)) => {
                    if by_prefix && n > 0 {
                        prefixed.push(patch.clone());
                    }
                    listed |= n > 0;
                    if n > 0 {
                        v.push(n);
                    }
                }
                _ => notes.push(format!(
                    "{path}: the layers of {patch} aren't those of a cutting surface's patch in whole numbers: left out"
                )),
            }
        }
        if layers_on && !prefixed.is_empty() {
            let shown: Vec<&str> = prefixed.iter().take(5).map(String::as_str).collect();
            notes.push(format!(
                "{path}: the layers of {} are taken as their surface's, by name (its solids aren't read here): the entry gives them to every patch of that surface, a solid without layers here too",
                shown.join(", ")
            ));
        }
        if layers_on {
            for (s, v) in surfaces.iter_mut().zip(&per) {
                let Some(&most) = v.iter().max() else { continue };
                if v.iter().any(|x| *x != most) {
                    notes.push(format!(
                        "{path}: the patches of surface {} have different numbers of layers: the entry gives each {most}",
                        s.name
                    ));
                }
                s.layers = most;
            }
        } else if listed {
            notes.push(format!("{path}: addLayers is off, so the layers it lists aren't taken"));
        }
    }
    let (mut expansion, mut first_layer) = (None, None);
    if let Some(ld) = layers_controls {
        if let Some(e) = number_of(ld, "expansionRatio") {
            expansion = (e != EXPANSION).then_some(e);
        }
        let relative =
            ld.get("relativeSizes").and_then(single).is_none_or(|v| matches!(v.as_str(), "true" | "yes" | "on"));
        // The sizes the entry writes: relative, the last layer half a cell and at least a
        // quarter; absolute, the first layer's thickness and a tenth of it at least.
        let layered = surfaces.iter().any(|s| s.layers > 0);
        let differs = |key: &str, want: Option<f64>| match (number_of(ld, key), ld.get(key).is_some()) {
            (_, false) => false,
            (Some(x), true) => want.is_none_or(|w| (x - w).abs() > 1e-12 * w.abs().max(1.0)),
            (None, true) => true,
        };
        if !relative {
            first_layer = number_of(ld, "firstLayerThickness");
            if first_layer.is_none() {
                notes.push(format!(
                    "{path}: its layers have absolute sizes without a firstLayerThickness: the entry's are relative to the cells"
                ));
            }
        }
        let mut sizes: Vec<&str> = Vec::new();
        let first = first_layer;
        for (key, want) in [
            ("firstLayerThickness", if relative { None } else { first }),
            ("finalLayerThickness", if relative { Some(0.5) } else { None }),
            ("minThickness", if relative { Some(0.25) } else { first.map(|t| t / 10.0) }),
            ("thickness", None),
        ] {
            if differs(key, want) {
                sizes.push(key);
            }
        }
        if layered && !sizes.is_empty() {
            notes.push(format!(
                "{path}: its layers' {} aren't what the entry writes ({}): left out",
                sizes.join(", "),
                if relative {
                    "finalLayerThickness 0.5, minThickness 0.25"
                } else {
                    "minThickness a tenth of the first layer"
                }
            ));
        }
    }
    // An expansion or first layer the entry would refuse: left out, the rest kept.
    if expansion.is_some_and(|e| !(e.is_finite() && e >= 1.0)) {
        notes.push(format!("{path}: its expansionRatio is below 1: left out"));
        expansion = None;
    }
    if first_layer.is_some_and(|t| !(t.is_finite() && t > 0.0)) {
        notes.push(format!("{path}: its firstLayerThickness isn't above 0: left out"));
        first_layer = None;
    }
    let mut sn = Snappy { inside, surfaces, boxes, expansion, first_layer };
    // A surface or box the entry can't hold is left out, the rest kept.
    let problems = sn.problems();
    let of = |key: &str, name: &str| {
        let at = format!("{key}/{name}");
        problems
            .iter()
            .filter(|(p, _)| *p == at || p.starts_with(&format!("{at}/")))
            .map(|(p, m)| format!("{p}: {m}"))
            .collect::<Vec<_>>()
    };
    let mut kept = Vec::new();
    for s in std::mem::take(&mut sn.surfaces) {
        let why = of("surfaces", &s.name);
        if why.is_empty() {
            kept.push(s);
        } else {
            notes.push(format!("{path}: surface {} can't be held by the entry ({}): left out", s.name, why.join("; ")));
        }
    }
    sn.surfaces = kept;
    let mut kept = Vec::new();
    for b in std::mem::take(&mut sn.boxes) {
        let why = of("boxes", &b.name);
        if why.is_empty() {
            kept.push(b);
        } else {
            notes.push(format!("{path}: box {} can't be held by the entry ({}): left out", b.name, why.join("; ")));
        }
    }
    sn.boxes = kept;
    let problems = sn.problems();
    if !problems.is_empty() {
        let list: Vec<String> =
            problems.iter().map(|(at, m)| if at.is_empty() { m.clone() } else { format!("{at}: {m}") }).collect();
        return Err(format!("what it holds isn't a snappy entry: {}", list.join("; ")));
    }
    Ok(sn)
}

/// The type snappyHexMesh gives the patch of cutting surface `surface` in the
/// `snappyHexMeshDict` `root` (of its solid `solid`, for a surface of several): its own
/// `regions` entry's `patchInfo`, the surface's, else `wall`. With no solid given, a `regions` of
/// one entry is that solid's.
pub fn patch_type(root: &Dict, surface: &str, solid: Option<&str>) -> String {
    let body = root
        .get("castellatedMeshControls")
        .and_then(Entry::dict)
        .and_then(|d| d.get("refinementSurfaces"))
        .and_then(Entry::dict)
        .and_then(|d| d.get(surface))
        .and_then(Entry::dict);
    let type_in = |d: &Dict| d.get("patchInfo").and_then(Entry::dict).and_then(|p| p.get("type")).and_then(single);
    let regions = body.and_then(|b| b.get("regions")).and_then(Entry::dict);
    let own = match solid {
        Some(x) => regions.and_then(|r| r.get(x)).and_then(Entry::dict),
        None => regions.and_then(|r| {
            let mut it = r.entries();
            match (it.next(), it.next()) {
                (Some(e), None) => e.dict(),
                _ => None,
            }
        }),
    };
    own.and_then(type_in).or_else(|| body.and_then(type_in)).unwrap_or_else(|| "wall".to_string())
}

#[cfg(test)]
#[path = "snappy_import_tests.rs"]
mod tests;
