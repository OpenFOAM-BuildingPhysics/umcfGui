//! Edits to the case's workflow description (`system/umcfWorkflow`), entry by entry: each
//! changes only the entries it names (`foam-dict`'s set, remove and rename), so the file's
//! comments, order and layout stay. They change the description only; the scripts and
//! dictionaries follow once they are written from it (gaps rows 4–8).

use umcf_foam_dict::{Document, Node, unquote};

use crate::description::{PATH, Role};
use crate::{Case, Error, RegionKind, Result};

/// A change to a patch of the description; `None` leaves that part as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PatchEdit {
    /// `Some(None)` takes the role away.
    pub role: Option<Option<Role>>,
    /// The region and patch it is coupled to; `Some(None)` takes the partner away.
    pub partner: Option<Option<(String, String)>>,
    /// A new name; the partners that name the patch follow it.
    pub rename: Option<String>,
}

/// What an edit did, besides the change itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditReport {
    /// Whether the entry it changed is new.
    pub added: bool,
    /// Other patches whose partner was changed with it (`region/patch`).
    pub followed: Vec<String>,
    /// Patches whose partner now names what isn't in the description.
    pub dangling: Vec<String>,
}

/// A name as a description entry takes it from a command: one OpenFOAM word that needs no
/// quotes (`air`, `air_to_street`, `wall-1`), so that it is found and written the same way.
pub fn plain_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':'))
        && !s.starts_with(|c: char| c.is_ascii_digit() || c == '-')
}

/// The word for `k` in the description (`Single` never gets there: it is refused first).
fn kind_name(k: RegionKind) -> &'static str {
    match k {
        RegionKind::Solid => "solid",
        RegionKind::Vegetation => "vegetation",
        RegionKind::Fluid | RegionKind::Single => "fluid",
    }
}

/// An edit refused, with why.
fn refuse(message: String) -> Error {
    Error::Describe(message)
}

/// `s` as a name the edit takes, or why not (`what` names it in the message).
fn name(what: &str, s: &str) -> Result<()> {
    if plain_name(s) {
        Ok(())
    } else {
        Err(refuse(format!(
            "{what} `{s}` is not a plain OpenFOAM name (letters, digits, _ . - :, not starting with a digit or -)"
        )))
    }
}

/// How many entries `path` names: each key on it must be there once at most, as the reader
/// takes the first and an edit the last of keys given twice.
fn once(doc: &Document, path: &[&str]) -> Result<bool> {
    let mut dict = &doc.root;
    for (i, key) in path.iter().enumerate() {
        let n = dict.keys().filter(|k| k == key).count();
        if n > 1 {
            return Err(refuse(format!(
                "{PATH}: `{}` is given {n} times: mend the file by hand first",
                path.get(..=i).map(|p| p.join("/")).unwrap_or_default()
            )));
        }
        let Some(e) = dict.get(key) else { return Ok(false) };
        if i + 1 < path.len() {
            match e.dict() {
                Some(d) => dict = d,
                None => {
                    let at = path.get(..=i).map(|p| p.join("/")).unwrap_or_default();
                    return Err(refuse(format!("{PATH}: `{at}` is not a dictionary")));
                }
            }
        }
    }
    Ok(true)
}

/// Every patch's partner as the file has it (region, patch, partner region, partner patch),
/// whatever else the reader makes of the region or patch (a region whose kind it can't read is
/// one it leaves out): the partners a rename follows, or a region taken out leaves.
fn partners(doc: &Document) -> Vec<(String, String, String, String)> {
    let mut out = Vec::new();
    let Some(regions) = doc.get(&["regions"]).and_then(|e| e.dict()) else { return out };
    for r in regions.entries() {
        let Some(patches) = r.dict().and_then(|b| b.get("patches")).and_then(|e| e.dict()) else { continue };
        for p in patches.entries() {
            let Some(partner) = p.dict().and_then(|b| b.get("partner")) else { continue };
            let [Node::List { items, .. }] = partner.value.as_slice() else { continue };
            if let [Node::Atom(a), Node::Atom(b)] = items.as_slice() {
                out.push((
                    r.key().to_string(),
                    p.key().to_string(),
                    unquote(&a.text).to_string(),
                    unquote(&b.text).to_string(),
                ));
            }
        }
    }
    out
}

/// An entry of the dictionary at `path` that ends without `;` (`{ role inlet }`, which OpenFOAM
/// reads): a key set after it would be read as part of its value, so the edit is refused.
fn unended(doc: &Document, path: &[&str]) -> Result<()> {
    let Some(body) = doc.get(path).and_then(|e| e.dict()) else { return Ok(()) };
    for e in body.entries() {
        if e.end.is_none() && e.dict().is_none() {
            return Err(refuse(format!(
                "{PATH}: `{}/{}` has no `;` at its end: mend the file by hand first",
                path.join("/"),
                e.key()
            )));
        }
    }
    Ok(())
}

/// The entry at `path` written again on one line with `changes` (`key`, its new value or
/// `None` to take it out), as the description writes a patch or a material: `{ role inlet; }`.
/// What else it holds is kept as it is written (a value the reader can't read, a quoted name, a
/// key it doesn't know); a new key goes last. `None` when it can't be: the entry isn't one line
/// of entries without comments (a macro, a directive, a dictionary laid out by hand), or isn't a
/// dictionary; then it is edited key by key.
fn one_line(doc: &Document, path: &[&str], changes: &[(&str, Option<String>)]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut done: Vec<&str> = Vec::new();
    if let Some(e) = doc.get(path) {
        let body = e.dict()?;
        let text: String = e.value.iter().map(ToString::to_string).collect();
        if text.trim_start().contains(['\n', '\r']) || text.contains("//") || text.contains("/*") {
            return None;
        }
        for item in &body.items {
            let umcf_foam_dict::Item::Entry(entry) = item else { return None };
            match changes.iter().find(|(k, _)| *k == entry.key()) {
                Some((k, v)) => {
                    if let Some(v) = v {
                        parts.push(format!("{k} {v};"));
                    }
                    done.push(k);
                }
                None => parts.push(item.to_string().trim().to_string()),
            }
        }
    }
    for (k, v) in changes {
        if let (false, Some(v)) = (done.contains(k), v) {
            parts.push(format!("{k} {v};"));
        }
    }
    Some(if parts.is_empty() { "{ }".to_string() } else { format!("{{ {} }}", parts.join(" ")) })
}

impl Case {
    /// The description as the edits find it: loaded as a dictionary of the case's own, not a
    /// link to another.
    fn editable(&self) -> Result<&Document> {
        if self.resolve(PATH) != PATH || self.aliases().any(|(_, p)| p == PATH) {
            return Err(Error::IsDict(PATH.to_string()));
        }
        self.dict(PATH).ok_or_else(|| {
            refuse(format!(
                "the case has no {PATH} (or it doesn't load): workflow.import writes one from the case first"
            ))
        })
    }

    /// The kind of `region` in the description, `None` when it isn't there.
    fn described_kind(&self, region: &str) -> Option<RegionKind> {
        self.description().and_then(|(d, _)| d.region(region).map(|r| r.kind))
    }

    /// Adds `region` of `kind` to the description, or gives it that kind. A solid with materials
    /// stays a solid (the materials would be left out).
    pub fn describe_region(&mut self, region: &str, kind: RegionKind) -> Result<EditReport> {
        name("region", region)?;
        if kind == RegionKind::Single {
            return Err(refuse("a region is fluid, solid or vegetation".into()));
        }
        let doc = self.editable()?;
        let there = once(doc, &["regions", region])?;
        if there {
            once(doc, &["regions", region, "kind"])?;
            once(doc, &["regions", region, "materials"])?;
            // Materials as the reader may find them: any item, or a value that isn't a dictionary.
            let materials = doc
                .get(&["regions", region, "materials"])
                .is_some_and(|e| e.dict().is_none_or(|m| !m.items.is_empty()));
            if kind != RegionKind::Solid && materials {
                return Err(refuse(format!(
                    "region {region} has materials: remove them first (only a solid has materials)"
                )));
            }
        }
        // An empty `materials { }` goes with the solid it was for: another kind may have none.
        if there && kind != RegionKind::Solid {
            self.remove(PATH, &["regions", region, "materials"])?;
        }
        self.set(PATH, &["regions", region, "kind"], kind_name(kind))?;
        Ok(EditReport { added: !there, ..EditReport::default() })
    }

    /// Takes `region` out of the description. The patches that were coupled to it are listed.
    pub fn undescribe_region(&mut self, region: &str) -> Result<EditReport> {
        name("region", region)?;
        let doc = self.editable()?;
        if !once(doc, &["regions", region])? {
            return Err(refuse(format!("region {region} is not in {PATH}")));
        }
        self.remove(PATH, &["regions", region])?;
        let mut dangling = Vec::new();
        for (r, p, pr, _) in partners(self.editable()?) {
            if pr == region {
                dangling.push(format!("{r}/{p}"));
            }
        }
        if let Some(rad) = self.description().and_then(|(d, _)| d.radiation) {
            if rad.region == region {
                dangling.push("radiation".to_string());
            }
            if rad.lai_region.as_deref() == Some(region) {
                dangling.push("radiation lai".to_string());
            }
        }
        Ok(EditReport { dangling, ..EditReport::default() })
    }

    /// Changes a patch of `region` in the description: its role, its partner, its name. A
    /// patch the region doesn't list is added (not when it is to be renamed). A new name is
    /// followed by the partners that named the old one.
    pub fn describe_patch(&mut self, region: &str, patch: &str, edit: &PatchEdit) -> Result<EditReport> {
        name("region", region)?;
        name("patch", patch)?;
        if let Some(Some((r, p))) = &edit.partner {
            name("partner region", r)?;
            name("partner patch", p)?;
        }
        if let Some(to) = &edit.rename {
            name("new name", to)?;
        }
        if edit == &PatchEdit::default() {
            return Err(refuse("nothing to change: give a role, a partner or a new name".into()));
        }
        let doc = self.editable()?;
        if !once(doc, &["regions", region])? {
            return Err(refuse(format!("region {region} is not in {PATH}: describe it first")));
        }
        let there = once(doc, &["regions", region, "patches", patch])?;
        let base = ["regions", region, "patches", patch];
        if there {
            once(doc, &[&base[..], &["role"]].concat())?;
            once(doc, &[&base[..], &["partner"]].concat())?;
            unended(doc, &base)?;
        } else if edit.rename.is_some() {
            return Err(refuse(format!("patch {patch} of region {region} is not in {PATH}: nothing to rename")));
        }
        let mut followed = Vec::new();
        // The partners that name the patch, found before the rename (its own too, when it is
        // coupled to itself), each checked before anything changes.
        let mut naming: Vec<(String, String)> = Vec::new();
        if let Some(to) = &edit.rename {
            if to == patch {
                return Err(refuse(format!("patch {patch} of region {region} has that name already")));
            }
            if once(doc, &["regions", region, "patches", to])? {
                return Err(refuse(format!("region {region} has a patch {to} already")));
            }
            // Its blocks follow the rename: only from blocks that read whole.
            if doc.get(&["regions", region, "blocks"]).is_some() {
                self.blocks_read(region)?;
                // `defaultFaces` in the blocks is the faces left to blockMesh: a patch renamed to
                // it would join them, one renamed from it would take them all.
                if to == crate::blocks::DEFAULT_FACES || patch == crate::blocks::DEFAULT_FACES {
                    return Err(refuse(format!(
                        "region {region} has blocks, where {} is the faces left to blockMesh: renaming {patch} to {to} would join or split them; change the blocks with mesh.block",
                        crate::blocks::DEFAULT_FACES
                    )));
                }
            }
            // Nor one its blocks name: the two would become one.
            if let Some(g) = self.description().and_then(|(d, _)| d.region(region).and_then(|r| r.blocks.clone()))
                && (g.patch_names().contains(&to.as_str()) || g.given_type(to).is_some())
            {
                return Err(refuse(format!(
                    "region {region}'s blocks name a patch {to} already: renaming {patch} to it would make the two one"
                )));
            }
            for (r, p, pr, pp) in partners(doc) {
                // Its own partner, when this call sets it, is the one given.
                let own = r == region && p == patch;
                if pr == region && pp == patch && !(own && edit.partner.is_some()) {
                    naming.push((r, p));
                }
            }
            for (r, p) in &naming {
                once(doc, &["regions", r, "patches", p, "partner"])?;
            }
        }
        let changes: Vec<(&str, Option<String>)> = [
            edit.role.map(|r| ("role", r.map(|r| r.name().to_string()))),
            edit.partner.as_ref().map(|p| ("partner", p.as_ref().map(|(r, q)| format!("( {r} {q} )")))),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !changes.is_empty() {
            match one_line(self.editable()?, &base, &changes) {
                // As the description writes a patch, on its line: `west { role inlet; }`.
                Some(value) => self.set_inline(PATH, &base, &value)?,
                // Laid out by hand: each key set where it is, the rest as it was.
                None => {
                    for (k, v) in &changes {
                        let path = [&base[..], &[*k]].concat();
                        match v {
                            Some(v) => self.set(PATH, &path, v)?,
                            None => {
                                self.remove(PATH, &path)?;
                            }
                        }
                    }
                }
            }
        }
        // The region's blocks that name the patch, renamed with it.
        let blocks = edit.rename.as_ref().and_then(|to| {
            let mut g = self.description()?.0.region(region)?.blocks.clone()?;
            g.rename_patch(patch, to).then_some(g)
        });
        if let Some(to) = &edit.rename {
            self.rename_described(&base, to)?;
            if let Some(g) = &blocks {
                self.describe_blocks(region, g)?;
            }
            for (r, p) in naming {
                // Its own partner is under its new name now.
                let p = if r == region && p == patch { to.clone() } else { p };
                self.set(PATH, &["regions", &r, "patches", &p, "partner"], &format!("( {region} {to} )"))?;
                followed.push(format!("{r}/{p}"));
            }
        }
        Ok(EditReport { added: !there, followed, ..EditReport::default() })
    }

    /// Adds a material to a solid `region` of the description, or gives it `model`
    /// (`Some(None)` takes the model away).
    pub fn describe_material(
        &mut self,
        region: &str,
        material: &str,
        model: Option<Option<&str>>,
    ) -> Result<EditReport> {
        name("region", region)?;
        name("material", material)?;
        if let Some(Some(m)) = model {
            name("model", m)?;
        }
        let doc = self.editable()?;
        if !once(doc, &["regions", region])? {
            return Err(refuse(format!("region {region} is not in {PATH}: describe it first")));
        }
        if self.described_kind(region) != Some(RegionKind::Solid) {
            return Err(refuse(format!("region {region} is not a solid: only a solid has materials")));
        }
        let doc = self.editable()?;
        let base = ["regions", region, "materials", material];
        let there = once(doc, &base)?;
        if there {
            once(doc, &[&base[..], &["model"]].concat())?;
            unended(doc, &base)?;
        }
        let changes: Vec<(&str, Option<String>)> =
            model.map(|m| ("model", m.map(str::to_string))).into_iter().collect();
        if !there || !changes.is_empty() {
            match one_line(self.editable()?, &base, &changes) {
                Some(value) => self.set_inline(PATH, &base, &value)?,
                None => {
                    for (k, v) in &changes {
                        let path = [&base[..], &[*k]].concat();
                        match v {
                            Some(v) => self.set(PATH, &path, v)?,
                            None => {
                                self.remove(PATH, &path)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(EditReport { added: !there, ..EditReport::default() })
    }

    /// Takes a material out of a solid `region` of the description.
    pub fn undescribe_material(&mut self, region: &str, material: &str) -> Result<EditReport> {
        name("region", region)?;
        name("material", material)?;
        let doc = self.editable()?;
        if !once(doc, &["regions", region, "materials", material])? {
            return Err(refuse(format!("region {region} has no material {material} in {PATH}")));
        }
        self.remove(PATH, &["regions", region, "materials", material])?;
        Ok(EditReport::default())
    }

    /// Gives `region` of the description the grid of blocks `grid` (its `blocks` entry). Each key
    /// of the entry is set on its own, and only when it changes, so the comments and layout of
    /// the others stay; a key the grid leaves out (no missing blocks, say) is taken out. The
    /// grid isn't checked here ([`crate::blocks::BlockGrid::problems`]), only its names.
    pub fn describe_blocks(&mut self, region: &str, grid: &crate::blocks::BlockGrid) -> Result<EditReport> {
        name("region", region)?;
        for n in grid.patch_names() {
            name("patch", n)?;
        }
        let doc = self.editable()?;
        if !once(doc, &["regions", region])? {
            return Err(refuse(format!("region {region} is not in {PATH}: describe it first")));
        }
        // A blocks entry that doesn't read: what isn't read would be taken for absent, and lost.
        self.blocks_read(region)?;
        let base = ["regions", region, "blocks"];
        let there = once(doc, &base)?;
        let mut current = None;
        if there {
            if doc.get(&base).is_some_and(|e| e.dict().is_none()) {
                return Err(refuse(format!(
                    "{PATH}: `regions/{region}/blocks` is not a dictionary: mend it by hand first"
                )));
            }
            for (k, _) in grid.parts() {
                once(doc, &[&base[..], &[k]].concat())?;
            }
            unended(doc, &base)?;
            current = self.description().and_then(|(d, _)| d.region(region).and_then(|r| r.blocks.clone()));
        }
        let old: Vec<(&str, Option<String>)> = current.as_ref().map(|g| g.parts()).unwrap_or_default();
        for (k, v) in grid.parts() {
            // As it is: left alone, its comments with it.
            if current.is_some() && old.iter().any(|(ok, ov)| *ok == k && *ov == v) {
                continue;
            }
            let path = [&base[..], &[k]].concat();
            match v {
                Some(v) => self.set_inline(PATH, &path, &v)?,
                None => {
                    self.remove(PATH, &path)?;
                }
            }
        }
        Ok(EditReport { added: !there, ..EditReport::default() })
    }

    /// Refused, naming the first read problem, while `region`'s `blocks` entry in the description
    /// doesn't read whole: an edit from what was read would lose what wasn't.
    pub fn blocks_read(&self, region: &str) -> Result<()> {
        let here = format!("regions/{region}/blocks");
        let Some((_, problems)) = self.description() else { return Ok(()) };
        match problems.iter().find(|p| p.at == here || p.at.starts_with(&format!("{here}/"))) {
            Some(p) => Err(refuse(format!(
                "the blocks of region {region} in {PATH} don't read ({}: {}): mend them by hand first",
                p.at, p.message
            ))),
            None => Ok(()),
        }
    }

    /// Takes the `blocks` entry out of `region` of the description (its `blockMeshDict` stays
    /// as it is, the case's own again).
    pub fn undescribe_blocks(&mut self, region: &str) -> Result<EditReport> {
        name("region", region)?;
        let doc = self.editable()?;
        if !once(doc, &["regions", region, "blocks"])? {
            return Err(refuse(format!("region {region} has no blocks in {PATH}")));
        }
        self.remove(PATH, &["regions", region, "blocks"])?;
        Ok(EditReport::default())
    }

    /// Gives the description's entry at `path` the key `to`; an error when it isn't there.
    fn rename_described(&mut self, path: &[&str], to: &str) -> Result<()> {
        let doc = self.dict_mut(PATH)?;
        let renamed = doc.rename(path, to).map_err(|source| Error::Dict { path: PATH.to_string(), source })?;
        if !renamed {
            return Err(Error::Dict {
                path: PATH.to_string(),
                source: umcf_foam_dict::Error::NotFound(path.join("/")),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "description_edit_tests.rs"]
mod tests;
