//! The `createPatchDict`s, generated from `system/umcfWorkflow`: the patches `createPatch` makes
//! of others (`constructFrom patches`), each typed by its role, as the reference cases make
//! them (docs/common-workflow.md, *What the known cases do*): blockMesh's own `defaultFaces`
//! made into the buildings' walls (`buildings`, a solid's `sides`), the vegetation region's
//! `region1_to_<zone>` patch, which `splitMeshRegions` leaves, made into its canopy
//! (`air_to_vegetation`).
//!
//! A patch's `from` in the description names the patches it is made of, as the dictionary has
//! them: a bare name is one patch, a quoted one a pattern (`"region1_to_.*"`), as `createPatch`
//! reads them (a quoted name without a regular expression's characters is the patch it names). Two
//! dictionaries are read:
//!
//! - `system/<region>/createPatchDict`, by `createPatch -region <region>` (a step after the
//!   mesh, `steps ( createPatch … )`);
//! - `system/createPatchDict`, by `createPatch` on `constant/polyMesh` (a step of the mesh,
//!   `mesh ( blockMesh … createPatch … )`): one for every region meshed so, which therefore make
//!   the same patches.
//!
//! The type is the patch's own `type` where it has one (a plain wall without a partner, as the
//! canyons' `buildings`, has no role that gives it), else `wall` for a wall's role (`ground`,
//! `coupled`, `interior`, `bottom`), the canopy and every solid's patch (the reference cases'
//! solids' `sides`), else `patch`; `changeDictionary` makes a coupled one and the canopy mapped
//! after. OpenFOAM-8 reads `pointSync`, OpenFOAM-12 doesn't: it is written
//! for OpenFOAM-8 only.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use umcf_core::OfVersion;
use umcf_foam_dict::{Dict, Item, Node, unquote};

use crate::RegionKind;
use crate::description::{Description, RegionDesc, Role};
use crate::workflow::{MeshBuild, RegionStep, RootStep};

/// The `createPatchDict` of `createPatch` on `constant/polyMesh`.
pub const ROOT_PATH: &str = "system/createPatchDict";

/// A patch `createPatch` makes: its name, type and the patches it is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub patch: String,
    pub kind: String,
    /// As the description has them: a name that isn't a plain word is a pattern.
    pub from: Vec<String>,
}

/// One `createPatchDict` the description's steps read: where, the regions whose `createPatch`
/// reads it, and the patches it makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub regions: Vec<String>,
    pub made: Vec<Made>,
}

/// Where `region`'s own `createPatchDict` is (the same in OpenFOAM-8 and -12).
pub fn dict_path(region: &str) -> String {
    format!("system/{region}/createPatchDict")
}

/// Whether `region`'s steps after the mesh run `createPatch -region`.
pub fn runs(r: &RegionDesc) -> bool {
    r.steps.iter().any(|s| matches!(s, RegionStep::CreatePatch))
}

/// Whether `region`'s mesh runs `createPatch` on `constant/polyMesh`.
pub fn runs_root(r: &RegionDesc) -> bool {
    matches!(&r.mesh, Some(MeshBuild::Root(steps)) if steps.contains(&RootStep::CreatePatch))
}

/// The type `createPatch` makes a patch of `role` in a region of `kind` (without a `type` of its
/// own): `wall` for a wall's role, the canopy and every solid's patch, else `patch`.
pub fn role_type(kind: RegionKind, role: Role) -> &'static str {
    match (kind, role) {
        (RegionKind::Solid, _) | (_, Role::Ground | Role::Coupled | Role::Interior | Role::Bottom | Role::Canopy) => {
            "wall"
        }
        _ => "patch",
    }
}

/// The patches `createPatch` makes in region `r`, from its patches' `from`; an error says why
/// they can't be written (neither a role nor a type, a name that isn't a plain OpenFOAM word).
fn made(r: &RegionDesc) -> Result<Vec<Made>, String> {
    let mut out = Vec::new();
    for p in r.patches.iter().filter(|p| !p.from.is_empty()) {
        if !crate::description_edit::plain_name(&p.name) {
            return Err(format!(
                "patch `{}` of region {} isn't a plain OpenFOAM word: rename it to write the createPatchDict",
                p.name, r.name
            ));
        }
        let Some(kind) = p.kind.clone().or_else(|| p.role.map(|role| role_type(r.kind, role).to_string())) else {
            return Err(format!(
                "patch {} of region {} is made by createPatch, but has neither a role nor a type, which gives its type: give it one",
                p.name, r.name
            ));
        };
        out.push(Made { patch: p.name.clone(), kind, from: p.from.clone() });
    }
    Ok(out)
}

/// Every `createPatchDict` the description's steps read, with the patches each makes; with
/// `only`, those region `only`'s mesh and steps read (and the regions sharing
/// `system/createPatchDict` with it). An error says why one can't be written (also: a region
/// whose mesh and steps both run `createPatch` with patches to make, so which makes them isn't
/// told; regions meshed in `constant/polyMesh` whose patches made by `createPatch` differ, as
/// they read the same dictionary).
pub fn targets(d: &Description, only: Option<&str>) -> Result<Vec<Target>, String> {
    let shared = only.and_then(|n| d.region(n)).is_some_and(runs_root);
    let mut out = Vec::new();
    let mut root: Option<Target> = None;
    for r in &d.regions {
        let (own, at_root) = (runs(r), runs_root(r));
        if only.is_some_and(|n| n != r.name && !(shared && at_root)) {
            continue;
        }
        if !own && !at_root {
            if let Some(p) = r.patches.iter().find(|p| !p.from.is_empty()) {
                return Err(format!(
                    "patch {} of region {} is made by createPatch (from), but the region's mesh and steps don't run createPatch",
                    p.name, r.name
                ));
            }
            continue;
        }
        let made = made(r)?;
        if own && at_root && !made.is_empty() {
            return Err(format!(
                "region {} runs createPatch both in its mesh (system/createPatchDict) and after it (system/{}/createPatchDict): which makes its patches isn't told: take one out",
                r.name, r.name
            ));
        }
        if own && only.is_none_or(|n| n == r.name) {
            crate::coupling::directory(&r.name, "createPatchDict")?;
            out.push(Target { path: dict_path(&r.name), regions: vec![r.name.clone()], made: made.clone() });
        }
        if at_root {
            match &mut root {
                None => root = Some(Target { path: ROOT_PATH.to_string(), regions: vec![r.name.clone()], made }),
                Some(t) => {
                    // As createPatch takes them: by patch, the sources their union.
                    let key = |m: &[Made]| {
                        let mut v: Vec<(String, String, Vec<String>)> =
                            m.iter().map(|x| (x.patch.clone(), x.kind.clone(), source_set(&x.from))).collect();
                        v.sort();
                        v
                    };
                    if key(&t.made) != key(&made) {
                        return Err(format!(
                            "regions {} and {} are meshed in constant/polyMesh with createPatch, which reads system/createPatchDict for both: the patches they make (from) must be the same",
                            t.regions[0], r.name
                        ));
                    }
                    let at: HashMap<&str, usize> =
                        made.iter().enumerate().map(|(i, m)| (m.patch.as_str(), i)).collect();
                    if let Some((a, b)) = out_of_order(t.made.iter().map(|m| m.patch.as_str()), &at) {
                        return Err(format!(
                            "regions {} and {} are meshed in constant/polyMesh with createPatch, which reads system/createPatchDict for both: they make {a} and {b} in another order (createPatch gives a face to the last that takes it, and any two may take the same faces: a source both name, a pattern, a patch group): list them in the same order",
                            t.regions[0], r.name
                        ));
                    }
                    t.regions.push(r.name.clone());
                }
            }
        }
    }
    out.extend(root);
    Ok(out)
}

/// The `createPatchDict` of `t` for `version`, generated ([`crate::blocks::stamped`]).
pub fn dict(t: &Target, version: OfVersion) -> String {
    let location = t.path.rsplit_once('/').map_or("system", |(dir, _)| dir);
    let mut s = format!(
        "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"{location}\";\n    object      createPatchDict;\n}}\n\n"
    );
    if version == OfVersion::V8 {
        s.push_str("pointSync false;\n\n");
    }
    s.push_str("patches\n(\n");
    for m in &t.made {
        let _ = write!(
            s,
            "    {{\n        name            {};\n        patchInfo\n        {{\n            type            {};\n        }}\n        constructFrom   patches;\n        patches         ( {} );\n    }}\n",
            m.patch,
            m.kind,
            m.from.join(" ")
        );
    }
    s.push_str(");\n\n// ************************************************************************* //\n");
    let whose = if t.regions.len() == 1 { "region" } else { "regions" };
    crate::blocks::stamped(
        &format!(
            "The patches createPatch makes of others in {whose} {} in system/umcfWorkflow (from), by role (docs/common-workflow.md).",
            t.regions.join(", ")
        ),
        &s,
    )
}

/// One item of a `createPatchDict`'s `patches`, as far as it is plain.
struct There {
    name: Option<String>,
    construct: Option<String>,
    from: Option<Vec<String>>,
    kind: Option<String>,
    /// Keys of its `patchInfo` besides `type` (a cyclic's `neighbourPatch`, `inGroups`, …).
    info: Vec<String>,
    /// Its `patchInfo` is there but isn't a dictionary.
    info_odd: bool,
    /// Its keys besides `name`, `patchInfo`, `constructFrom`, `patches` and `set`.
    other: Vec<String>,
    /// No directive or macro in it: all it holds is seen.
    told: bool,
    /// A key given twice in it or its `patchInfo`, which OpenFOAM merges.
    twice: bool,
}

/// A name as a message shows it: at most 64 characters, then `…`.
pub(crate) fn cut(s: &str) -> String {
    match s.char_indices().nth(64) {
        Some((at, _)) => format!("{}…", s.get(..at).unwrap_or_default()),
        None => s.to_string(),
    }
}

/// At most a few of `items`, each cut, then how many more: what a hostile file can't make long.
fn few(items: &[String]) -> String {
    const FEW: usize = 5;
    let shown: Vec<String> = items.iter().take(FEW).map(|s| cut(s)).collect();
    match items.len().saturating_sub(FEW) {
        0 => shown.join(" "),
        more => format!("{} … and {more} more", shown.join(" ")),
    }
}

/// What a macro or directive gives: not compared.
fn untold(t: &str) -> bool {
    t.starts_with(['$', '#'])
}

/// Whether `values` hold what a macro or directive gives (`$x`, `#calc`), at any depth.
fn holds_untold(values: &[Node]) -> bool {
    let mut stack: Vec<&Node> = values.iter().collect();
    while let Some(n) = stack.pop() {
        match n {
            Node::Atom(t) if untold(&t.text) => return true,
            Node::Atom(_) => {}
            Node::List { items, .. } | Node::Dims { items, .. } => stack.extend(items),
            Node::Dict { body, .. } => {
                if body.items.iter().any(|i| !matches!(i, Item::Entry(_))) {
                    return true;
                }
                stack.extend(body.entries().flat_map(|e| e.value.iter()));
            }
        }
    }
    false
}

/// Whether an item of `patches`, or its `patchInfo`, gives a key twice (OpenFOAM merges them).
pub(crate) fn keys_twice(d: &Dict) -> bool {
    let twice = |x: &Dict| {
        let mut keys = HashSet::new();
        x.entries().any(|e| !keys.insert(e.key()))
    };
    twice(d) || d.entries().filter(|e| e.key() == "patchInfo").filter_map(|e| e.dict()).any(twice)
}

/// Whether an item of `patches` is whole: no macro or directive anywhere in it (as an item or a
/// value), and its `patchInfo`, where there is one, a dictionary.
pub(crate) fn whole(d: &Dict) -> bool {
    let info = d.get("patchInfo");
    !(d.items.iter().any(|i| !matches!(i, Item::Entry(_)))
        || d.entries().any(|e| holds_untold(&e.value))
        || info.is_some_and(|e| e.dict().is_none()))
}

/// The items of `root`'s `patches`, or what is wrong with the list (`None`: not told, a macro
/// or directive gives it).
fn items(root: &Dict) -> Result<Option<Vec<There>>, String> {
    let directives = root.items.iter().any(|i| matches!(i, Item::Directive { .. } | Item::Macro { .. }));
    let Some(list) = root.get("patches") else {
        return if directives {
            Ok(None)
        } else {
            Err("it has no patches list, without which createPatch stops".into())
        };
    };
    let items = match (listed(&list.value), list.value.as_slice()) {
        (Some(items), _) => items,
        (None, [Node::Atom(t)]) if untold(&t.text) => return Ok(None),
        (None, [Node::Atom(_), Node::List { .. }]) => {
            return Err("its patches list's count isn't the number of its items, at which OpenFOAM stops".into());
        }
        _ => return Err("its patches isn't a list of dictionaries".into()),
    };
    let one = |d: &Dict, k: &str| match d.get(k).map(|e| e.value.as_slice()) {
        Some([Node::Atom(t)]) => Some(unquote(&t.text).to_string()),
        _ => None,
    };
    let mut out = Vec::new();
    for n in items {
        let Node::Dict { body: d, .. } = n else {
            if let Node::Atom(t) = n
                && untold(&t.text)
            {
                out.push(There {
                    name: None,
                    construct: None,
                    from: None,
                    kind: None,
                    info: Vec::new(),
                    info_odd: false,
                    other: Vec::new(),
                    told: false,
                    twice: false,
                });
                continue;
            }
            return Err("its patches isn't a list of dictionaries".into());
        };
        let info_entry = d.get("patchInfo");
        let info_dict = info_entry.and_then(|e| e.dict());
        // Not a dictionary, nor what a macro gives (which is said as such).
        let info_odd = info_entry.is_some_and(|e| e.dict().is_none() && !holds_untold(&e.value));
        let other: Vec<String> = d
            .entries()
            .map(|e| e.key().to_string())
            .filter(|k| !matches!(k.as_str(), "name" | "patchInfo" | "constructFrom" | "patches" | "set"))
            .collect();
        let told = whole(d);
        let from = match d.get("patches").and_then(|e| listed(&e.value)) {
            Some(items) => items
                .iter()
                .map(|n| match n {
                    Node::Atom(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect(),
            None => None,
        };
        let kind = info_dict.and_then(|p| one(p, "type"));
        let info = info_dict
            .map(|p| p.entries().map(|e| e.key().to_string()).filter(|k| k != "type").collect())
            .unwrap_or_default();
        let twice = keys_twice(d);
        out.push(There {
            name: one(d, "name"),
            construct: one(d, "constructFrom"),
            from,
            kind,
            info,
            info_odd,
            other,
            told,
            twice,
        });
    }
    Ok(Some(out))
}

/// Whether OpenFOAM takes the quoted `inner` for a regular expression (`wordRe::compile` with
/// `DETECT`): one of `. * + ? ( ) | [ ]` (`regExp::meta`), or a character a word can't hold.
fn pattern(inner: &str) -> bool {
    inner.chars().any(|c| {
        matches!(c, '.' | '*' | '+' | '?' | '(' | ')' | '|' | '[' | ']' | '\'' | '/' | ';' | '{' | '}' | '$')
            || c.is_whitespace()
    })
}

/// How a source compares: a quoted one OpenFOAM doesn't take for a pattern is the name it holds,
/// as `createPatch` matches it, unless the name written bare would be read otherwise (`123`, a
/// number, or starting with a sign or a dot); else as written.
pub(crate) fn normal(t: &str) -> String {
    match t.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner)
            if t.len() >= 2
                && !pattern(inner)
                && !inner.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '.')) =>
        {
            inner.to_string()
        }
        _ => t.to_string(),
    }
}

/// Sources as `createPatch` takes them: their union, in no order.
fn source_set(from: &[String]) -> Vec<String> {
    let mut v: Vec<String> = from.iter().map(|f| normal(f)).collect();
    v.sort();
    v.dedup();
    v
}

/// Two patches of `seq` (the names of a dictionary's items, in its order) that `at` (a patch's
/// place elsewhere) has the other way round, or a name given again around another: the one
/// before, and the one after. `createPatch` gives a face to the last item that takes it, and any
/// two may take the same faces (a source both name, a pattern, a name that is a patch group, as
/// OpenFOAM takes a name no patch has), so their order decides where those faces go. Items `at`
/// doesn't place aren't compared (one the description doesn't make is a difference of its own,
/// and a name given again around it isn't seen as such); one given again right after itself is
/// one item. In one pass.
fn out_of_order<'a>(seq: impl IntoIterator<Item = &'a str>, at: &HashMap<&str, usize>) -> Option<(&'a str, &'a str)> {
    let mut last: Option<(&str, usize)> = None;
    for n in seq {
        let Some(&p) = at.get(n) else { continue };
        if let Some((m, q)) = last {
            if m == n {
                continue;
            }
            if p <= q {
                return Some((m, n));
            }
        }
        last = Some((n, p));
    }
    None
}

/// A list as OpenFOAM reads one: `( … )`, or with its count first (`2 ( … )`), the count the
/// items'. `None`: neither.
pub(crate) fn listed(values: &[Node]) -> Option<&[Node]> {
    match values {
        [Node::List { items, .. }] => Some(items),
        [Node::Atom(n), Node::List { items, .. }] if n.text.parse::<usize>().ok() == Some(items.len()) => Some(items),
        _ => None,
    }
}

/// How a `createPatchDict` (`root`, the file's top level) differs from the patches `made`, for
/// `version`: a patch not made there, made of a set or of other patches (in any order: their
/// union), of both (in two items), of what isn't a list of names, of another type, one made
/// there the description doesn't make, an item read whole without a name or a constructFrom, two patches in the
/// other order (or one given again around another), and the `patches` list or OpenFOAM-8's `pointSync` missing, which
/// `createPatch` stops without. A name or type quoted or not is the same, and so is a source
/// quoted that OpenFOAM doesn't take for a pattern; what a macro or directive gives isn't
/// compared (an item holding one is compared by its name only); a `patches` list that isn't one
/// of dictionaries is said so.
pub fn differences(root: &Dict, made: &[Made], version: OfVersion) -> Vec<String> {
    let mut out = Vec::new();
    let directives = root.items.iter().any(|i| matches!(i, Item::Directive { .. } | Item::Macro { .. }));
    if version == OfVersion::V8 {
        match root.get("pointSync").map(|e| e.value.as_slice()) {
            None if !directives => {
                out.push("it has no pointSync, without which OpenFOAM-8's createPatch stops".to_string());
            }
            None => {}
            // A word or a number: a quoted string isn't a switch to OpenFOAM-8.
            Some([Node::Atom(t)])
                if untold(&t.text) || (!t.text.starts_with('"') && umcf_foam_dict::switch(&t.text).is_some()) => {}
            Some(v) if holds_untold(v) => {}
            Some(_) => out.push(
                "its pointSync isn't a switch (true or false), at which OpenFOAM-8's createPatch stops".to_string(),
            ),
        }
    }
    let there = match items(root) {
        Ok(Some(t)) => t,
        Ok(None) => return out,
        Err(e) => {
            out.push(e);
            return out;
        }
    };
    // By name: a name given twice is one patch of both items' sources (createPatch moves both
    // items' faces into the first), compared whole only when both are.
    let mut by_name: HashMap<&str, There> = HashMap::with_capacity(there.len());
    // How a name's items read whole make it, in any order: one without a constructFrom (which
    // stops createPatch), one of patches, and what else they are made of (the first by name, so
    // the message doesn't depend on the order: a set, a zone): what that moves isn't the
    // description's, whatever an item that can't be read whole holds.
    let mut ways: HashMap<&str, (bool, bool, Option<&str>)> = HashMap::with_capacity(there.len());
    for t in &there {
        let Some(n) = t.name.as_deref() else { continue };
        if t.told {
            let w = ways.entry(n).or_default();
            match t.construct.as_deref() {
                None => w.0 = true,
                Some("patches") => w.1 = true,
                Some(c) => w.2 = Some(w.2.map_or(c, |o: &str| o.min(c))),
            }
        }
        match by_name.get_mut(n) {
            None => {
                by_name.insert(
                    n,
                    There {
                        name: t.name.clone(),
                        construct: t.construct.clone(),
                        from: t.from.clone(),
                        kind: t.kind.clone(),
                        info: Vec::new(),
                        info_odd: false,
                        other: Vec::new(),
                        told: t.told,
                        twice: false,
                    },
                );
            }
            Some(first) => {
                first.told &= t.told;
                match (&mut first.from, &t.from) {
                    (Some(a), Some(b)) => a.extend(b.iter().cloned()),
                    (a, _) => *a = None,
                }
            }
        }
    }
    // An item whose name a macro or directive may give: what isn't made may be made by it. One
    // read whole without a name stops createPatch.
    let unnamed = there.iter().any(|t| t.name.as_deref().map_or(!t.told, untold));
    if there.iter().any(|t| t.name.is_none() && t.told) {
        out.push("an item there has no name (a word), at which createPatch stops".to_string());
    }
    let names: HashSet<&str> = made.iter().map(|m| m.patch.as_str()).collect();
    for m in made {
        let Some(t) = by_name.get(m.patch.as_str()) else {
            if unnamed {
                continue;
            }
            out.push(format!("patch {} isn't made there, the description makes it of ( {} )", m.patch, shown(&m.from)));
            continue;
        };
        let (bare, of_patches, of_set) = ways.get(m.patch.as_str()).copied().unwrap_or_default();
        if bare {
            out.push(format!("patch {} has no constructFrom there, which createPatch stops without", m.patch));
            continue;
        }
        match of_set {
            Some(_) if of_patches => {
                out.push(format!(
                    "patch {} is made in two ways there (items of other patches and of a set or zone), the description makes it of ( {} )",
                    m.patch,
                    shown(&m.from)
                ));
                continue;
            }
            Some(c) => {
                out.push(format!(
                    "patch {} is made of a {} there, the description makes it of ( {} )",
                    m.patch,
                    cut(c),
                    shown(&m.from)
                ));
                continue;
            }
            None => {}
        }
        if !t.told {
            continue;
        }
        match t.construct.as_deref() {
            Some("patches") => {}
            Some(c) => {
                out.push(format!(
                    "patch {} is made of a {} there, the description makes it of ( {} )",
                    m.patch,
                    cut(c),
                    shown(&m.from)
                ));
                continue;
            }
            None => {
                out.push(format!("patch {} has no constructFrom there, which createPatch stops without", m.patch));
                continue;
            }
        }
        match &t.from {
            Some(from) if source_set(from) != source_set(&m.from) => out.push(format!(
                "patch {} is made of ( {} ) there, the description makes it of ( {} )",
                m.patch,
                few(from),
                shown(&m.from)
            )),
            Some(_) => {}
            None => out.push(format!(
                "patch {} is made of what isn't a list of names there, the description makes it of ( {} )",
                m.patch,
                shown(&m.from)
            )),
        }
        match &t.kind {
            Some(k) if *k != m.kind => {
                out.push(format!("patch {}: type is {} there, the description makes it {}", m.patch, cut(k), m.kind));
            }
            Some(_) => {}
            None => out.push(format!(
                "patch {} has no type there (patchInfo), which createPatch needs for a new patch; the description makes it {}",
                m.patch, m.kind
            )),
        }
    }
    // The order: any two may take the same faces, and the last takes them (a name given again
    // around another is in both places).
    let at: HashMap<&str, usize> = made.iter().enumerate().map(|(i, m)| (m.patch.as_str(), i)).collect();
    if let Some((a, b)) = out_of_order(there.iter().filter_map(|t| t.name.as_deref()), &at) {
        out.push(format!(
            "patches {a} and {b} are made in the other order there, or one is given again around the other (createPatch gives a face to the last that takes it, and any two may take the same faces: a source both name, a pattern, a patch group)"
        ));
    }
    let mut said: HashSet<&str> = HashSet::new();
    for t in &there {
        if let Some(n) = &t.name
            && !untold(n)
            && !names.contains(n.as_str())
            && said.insert(n.as_str())
        {
            out.push(format!("patch {} is made there, the description doesn't make it (from)", cut(n)));
        }
    }
    out
}

/// What a `createPatchDict` (`root`) holds that the description can't, so writing it over from
/// `made` for `version` would drop it: a patch made there the description doesn't make, an item
/// that can't be read whole (no plain name, a macro or directive in it), a patch given twice, one
/// made of a face set or of sources the description can't hold, a key given twice in an item or
/// its `patchInfo` (OpenFOAM merges them), the entries of a `patchInfo` besides its `type` and
/// other keys of an item, a `patches` list that can't be read, a top-level entry
/// besides `patches` (OpenFOAM-8's `pointSync` unless it is off, as written), a key of its
/// `FoamFile` header besides version, format, class, location and object, or one whose value a
/// macro or directive gives. Empty: the description holds all of it.
pub fn kept(root: &Dict, made: &[Made], version: OfVersion) -> Vec<String> {
    let mut out = Vec::new();
    if root.items.iter().any(|i| matches!(i, Item::Directive { .. } | Item::Macro { .. })) {
        out.push("a directive or macro at its top".to_string());
    }
    if root.items.iter().any(|i| matches!(i, Item::Value(_))) {
        out.push("a value without a key at its top".to_string());
    }
    if root.entries().filter(|e| e.key() == "patches").count() > 1 {
        out.push("its patches, given twice".to_string());
    }
    for e in root.entries() {
        match e.key() {
            "FoamFile" => match e.dict() {
                Some(h) => {
                    for x in h.entries() {
                        let k = x.key();
                        if !matches!(k, "version" | "format" | "class" | "location" | "object") {
                            out.push(format!("its FoamFile {}", cut(k)));
                        } else if holds_untold(&x.value) {
                            // Written over, the value would be lost.
                            out.push(format!("its FoamFile {k}, given by a macro or directive"));
                        }
                        if out.len() > 2 * MAX_KEPT {
                            break;
                        }
                    }
                    if h.items.iter().any(|i| !matches!(i, Item::Entry(_))) {
                        out.push("its FoamFile's macro or directive".to_string());
                    }
                }
                None => out.push("its FoamFile, which isn't a dictionary".to_string()),
            },
            "patches" => {}
            "pointSync" if version == OfVersion::V12 => {}
            "pointSync" => {
                let off = match e.value.as_slice() {
                    [Node::Atom(t)] => !t.text.starts_with('"') && umcf_foam_dict::switch(&t.text) == Some(false),
                    _ => false,
                };
                if !off {
                    out.push("its pointSync, which isn't off".to_string());
                }
            }
            k => out.push(format!("its {}", cut(k))),
        }
        if out.len() > 2 * MAX_KEPT {
            out.truncate(MAX_KEPT);
            out.push("and more".to_string());
            return out;
        }
    }
    let there = match items(root) {
        Ok(Some(t)) => t,
        Ok(None) => {
            out.push("its patches given by a macro or directive".to_string());
            return out;
        }
        Err(_) => {
            out.push("its patches list, which can't be read whole".to_string());
            return out;
        }
    };
    let names: HashSet<&str> = made.iter().map(|m| m.patch.as_str()).collect();
    let mut seen: HashSet<&str> = HashSet::with_capacity(there.len());
    let mut unnamed = false;
    for t in &there {
        let Some(n) = t.name.as_deref().filter(|n| !untold(n)) else {
            // Said once: the items can't be told apart, and a file may hold many.
            if !std::mem::replace(&mut unnamed, true) {
                out.push("a patch without a plain name".to_string());
            }
            continue;
        };
        // An empty list moves no faces, which the description can't say (an empty from makes none).
        let sources_held = t.from.as_ref().is_some_and(|f| !f.is_empty() && f.iter().all(|s| source_ok(s)));
        let shown = cut(n);
        let first = seen.insert(n);
        if !names.contains(n) {
            if first {
                out.push(format!("patch {shown}"));
            }
        } else if !first {
            out.push(format!("patch {shown}, given twice (one item of both, by hand)"));
        } else if t.info_odd {
            out.push(format!("patch {shown}'s patchInfo, which isn't a dictionary"));
        } else if !t.told {
            out.push(format!("patch {shown}'s macro or directive"));
        } else if t.construct.as_deref() != Some("patches") {
            out.push(format!("patch {shown}, made of a {}", cut(t.construct.as_deref().unwrap_or("what isn't said"))));
        } else if !sources_held {
            out.push(format!("patch {shown}'s sources"));
        } else if t.twice {
            out.push(format!("patch {shown}'s keys given twice (OpenFOAM merges them)"));
        } else if !t.info.is_empty() {
            out.push(format!("patch {shown}'s patchInfo {}", few(&t.info)));
        } else if !t.other.is_empty() {
            out.push(format!("patch {shown}'s {}", few(&t.other)));
        }
        // A cap on what a hostile file makes long.
        if out.len() > 2 * MAX_KEPT {
            out.truncate(MAX_KEPT);
            out.push("and more".to_string());
            return out;
        }
    }
    out
}

/// What [`kept`] (or [`differences`]) lists, as a message gives it: the first, and how many more.
pub fn summary(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, .., last] if last == "and more" => format!("{first}, and more"),
        [first, rest @ ..] => format!("{first}, and {} more", rest.len()),
    }
}

/// The most of what [`kept`] lists, about.
const MAX_KEPT: usize = 20;

/// A source given by name (`workflow.patch`) as the dictionaries write it: as given when quoted,
/// a plain word bare, anything else quoted (a pattern).
pub fn source_token(s: &str) -> String {
    if s.starts_with('"') || crate::description_edit::plain_name(s) { s.to_string() } else { format!("\"{s}\"") }
}

/// Whether `t` is a source the description holds: a plain word, or a quoted pattern without
/// quotes, backslashes, white space or control characters inside (which the dictionaries would
/// have to escape, and `createPatch` reads otherwise).
pub fn source_ok(t: &str) -> bool {
    if crate::description_edit::plain_name(t) {
        return true;
    }
    match t.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) => {
            t.len() >= 2
                && !inner.is_empty()
                && inner.len() <= 255
                && !inner.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\\' | '{' | '}'))
                && (!pattern(inner) || regex_ok(inner))
        }
        None => false,
    }
}

/// The character classes POSIX names (`[[:digit:]]`), which every `regcomp` knows.
const POSIX_CLASSES: [&str; 12] =
    ["alnum", "alpha", "blank", "cntrl", "digit", "graph", "lower", "print", "punct", "space", "upper", "xdigit"];

/// Whether `re` is a regular expression OpenFOAM compiles, as far as can be told without its
/// regex library (POSIX extended, `regcomp` with `REG_EXTENDED`, in OpenFOAM-8 and -12): brackets
/// closed and not empty, no bracket inside one but a POSIX class (`[:digit:]`), ranges upward, groups closed and not starting
/// `(?`, a repeat (`* + ?`) only after what it repeats. Stricter than both, never looser:
/// what may not compile is refused.
fn regex_ok(re: &str) -> bool {
    // A leading `(?i)` is OpenFOAM's: case ignored, the rest compiled.
    let re = match re.strip_prefix("(?i)") {
        Some("") => return false,
        Some(rest) => rest,
        None => re,
    };
    let mut depth = 0usize;
    // Whether what came last can be repeated.
    let mut atom = false;
    let mut chars = re.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' => {
                if chars.peek() == Some(&'^') {
                    chars.next();
                }
                // Its members: a character, or a POSIX class (`[:digit:]`, None), which no range
                // may end at.
                let mut class: Vec<Option<char>> = Vec::new();
                loop {
                    match chars.next() {
                        None => return false,
                        Some(']') => break,
                        Some('[') => {
                            if chars.next() != Some(':') {
                                return false;
                            }
                            let mut name = String::new();
                            loop {
                                match chars.next() {
                                    Some(':') => break,
                                    Some(x) if x.is_ascii_lowercase() && name.len() < 8 => name.push(x),
                                    _ => return false,
                                }
                            }
                            if chars.next() != Some(']') || !POSIX_CLASSES.contains(&name.as_str()) {
                                return false;
                            }
                            class.push(None);
                        }
                        Some(x) => class.push(Some(x)),
                    }
                }
                if class.is_empty() {
                    return false;
                }
                let mut i = 1;
                while i + 1 < class.len() {
                    // Upward, between characters, and not chained (`a-c-e`, which POSIX leaves
                    // undefined).
                    if class.get(i) == Some(&Some('-')) {
                        match (class.get(i - 1), class.get(i + 1)) {
                            (Some(Some(lo)), Some(Some(hi))) if lo <= hi => {}
                            _ => return false,
                        }
                        // (A hyphen ending the bracket is a character: `[a-c-]`.)
                        if class.get(i + 2) == Some(&Some('-')) && class.get(i + 3).is_some() {
                            return false;
                        }
                    }
                    i += 1;
                }
                atom = true;
            }
            ']' => return false,
            '(' => {
                if chars.peek() == Some(&'?') {
                    return false;
                }
                depth += 1;
                atom = false;
            }
            ')' => {
                let Some(d) = depth.checked_sub(1) else { return false };
                depth = d;
                atom = true;
            }
            '|' => atom = false,
            '*' | '+' | '?' => {
                if !atom {
                    return false;
                }
                atom = false;
            }
            '^' | '$' => atom = false,
            _ => atom = true,
        }
    }
    depth == 0
}

fn shown(from: &[String]) -> String {
    few(from)
}

#[cfg(test)]
#[path = "create_patch_tests.rs"]
mod tests;
