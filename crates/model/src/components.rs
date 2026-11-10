//! A component switched on or off in a case (`workflow.component`): the grass, the temperature
//! limit, the blending layer, in the case's files as `case.new` writes them and in the
//! workflow description's `components` when the case has one.
//!
//! - grass: `constant/air/grassProperties` (where `case.new` puts it). On: its model (the one
//!   its coefficients are for, else the configuration's) and the patches given; the file from
//!   the configuration when there is none, what it lacks of it filled in. Off: `grassModel none;`
//!   (the solver's `noGrass`) in every fluid's, its coefficients and patches kept for switching
//!   it back on.
//! - temperatureLimit: a `limitTemperature` entry in the file each fluid's limit is read from
//!   (OpenFOAM-12: `system/fvConstraints`, else `system/fvOptions`, else `constant/fvOptions`;
//!   OpenFOAM-8: `constant/fvOptions`, else `system/fvOptions`; a new one in `system/`). On: one
//!   with the description's bounds, else the configuration's, in each fluid without one there.
//!   Off: every one taken out of those files, the bounds kept in the description's settings
//!   when it has none (those not kept noted; one in a file the solver doesn't read noted).
//! - blending: `blending` in `controlDict`. On: `true`, with `blendingCoeffs` (the description's,
//!   else the configuration's) when it has none; each fluid's mesh needs the patches `west`,
//!   `east`, `north` and `south`, which the solver's blending layer finds by name. Off: `false`,
//!   the coefficients kept.

use std::sync::Arc;

use umcf_core::{CaseFs, OfVersion};
use umcf_foam_dict::Document;
use umcf_schema::{Template, defaults};

use crate::description::PATH;
use crate::new_case::template;
use crate::{CONTROL_DICT, Case, Error, RegionKind, Result};

/// The fluid region the solver's grass looks up by name.
const AIR: &str = "air";

/// The components [`Case::switch_component`] switches.
pub const COMPONENTS: [&str; 3] = ["grass", "temperatureLimit", "blending"];

/// What [`Case::switch_component`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Switched {
    /// The files changed or added (not saved yet).
    pub changed: Vec<String>,
    /// Whether the description's `components` say so now (the case has one).
    pub described: bool,
    /// What is left to do, or was kept.
    pub notes: Vec<String>,
}

fn refuse<T>(message: String) -> Result<T> {
    Err(Error::NewCase(message))
}

fn dict_error(rel: &str) -> impl Fn(umcf_foam_dict::Error) -> Error + '_ {
    move |source| Error::Dict { path: rel.to_string(), source }
}

impl Case {
    /// Switches the component `word` ([`COMPONENTS`]) on or off, in the case's files (see
    /// [the module](self)) and in the description's `components`. `grass_patches`: the patches of
    /// `air` the grass grows on (on only). Refused, with nothing changed: a case whose version
    /// isn't known, the grass without a fluid named `air` or without the radiation, a patch
    /// `air` doesn't have, the blending layer in a case whose gravity isn't along z or whose
    /// fluid's mesh lacks a side (`west`, `east`, `north`, `south`), a link where a file would
    /// go. [`Case::save`] writes it all.
    pub fn switch_component(
        &mut self,
        fs: &dyn CaseFs,
        word: &str,
        on: bool,
        grass_patches: Option<&[String]>,
    ) -> Result<Switched> {
        let mut next = self.clone();
        let s = next.switch_component_to(fs, word, on, grass_patches)?;
        *self = next;
        Ok(s)
    }

    fn switch_component_to(
        &mut self,
        fs: &dyn CaseFs,
        word: &str,
        on: bool,
        grass_patches: Option<&[String]>,
    ) -> Result<Switched> {
        if !COMPONENTS.contains(&word) {
            return refuse(format!("{word} is not a component ({})", COMPONENTS.join(", ")));
        }
        if grass_patches.is_some() && !(word == "grass" && on) {
            return refuse("patches are the grass's, when it is switched on".to_string());
        }
        let Some(v) = self.layout.version else {
            return refuse("the case's OpenFOAM version isn't known: its files can't be told".to_string());
        };
        let mut s = Switched::default();
        match word {
            "grass" => self.switch_grass(fs, v, on, grass_patches, &mut s)?,
            "temperatureLimit" => self.switch_limit(fs, v, on, &mut s)?,
            _ => self.switch_blending(on, &mut s)?,
        }
        // The description says so, its other components as they are.
        if let Some((mut d, _)) = self.description() {
            let before = d.components;
            match word {
                "grass" => d.components.grass = on,
                "temperatureLimit" => d.components.temperature_limit = on,
                _ => d.components.blending = on,
            }
            if d.components != before {
                let words: Vec<&str> = d.components.words().into_iter().filter(|(_, x)| *x).map(|(w, _)| w).collect();
                if words.is_empty() {
                    self.remove(PATH, &["components"])?;
                } else {
                    self.set_inline(PATH, &["components"], &format!("( {} )", words.join(" ")))?;
                }
                s.changed.push(PATH.to_string());
            }
            s.described = true;
        }
        Ok(s)
    }

    /// A file of the configuration put into the case, where none is.
    fn put_new(&mut self, fs: &dyn CaseFs, rel: &str, doc: Document) -> Result<()> {
        if fs.is_link(rel) {
            return Err(Error::IsLink(rel.to_string()));
        }
        if fs.is_dir(rel) {
            return refuse(format!("{rel} is a directory: the file can't go there"));
        }
        self.files.insert(rel.to_string(), Arc::new(doc));
        Ok(())
    }

    fn switch_grass(
        &mut self,
        fs: &dyn CaseFs,
        v: OfVersion,
        on: bool,
        patches: Option<&[String]>,
        s: &mut Switched,
    ) -> Result<()> {
        let rel = format!("constant/{AIR}/grassProperties");
        let there = self.dict(&rel).is_some();
        if !on {
            // Every fluid's: the solver makes a grass model in each.
            let fluids: Vec<String> =
                self.regions.iter().filter(|r| r.kind == RegionKind::Fluid).map(|r| r.name.clone()).collect();
            let mut found = false;
            for f in fluids {
                let rel = format!("constant/{f}/grassProperties");
                if self.dict(&rel).is_none() {
                    if fs.is_file(&rel) {
                        return refuse(format!("{rel} doesn't load: switch the grass off in it by hand"));
                    }
                    continue;
                }
                found = true;
                if self.dict(&rel).and_then(|d| d.value(&["grassModel"])).as_deref() != Some("none") {
                    self.set(&rel, &["grassModel"], "none")?;
                    s.notes.push(format!(
                        "{rel}: grassModel none; its coefficients and patches kept for switching it back on"
                    ));
                    s.changed.push(rel);
                }
            }
            if !found {
                s.notes.push("no fluid has a grassProperties: the case has no grass".into());
            } else if s.changed.is_empty() {
                s.notes.push("the grass is off already (grassModel none)".into());
            }
            return Ok(());
        }
        let Some(air) = self.region(AIR).filter(|r| r.kind == RegionKind::Fluid) else {
            return refuse(
                "the grass needs a fluid region named `air` (the solver looks it up by that name)".to_string(),
            );
        };
        let air_patches = air.patches.clone();
        let radiation = self.description().and_then(|(d, _)| d.radiation).is_some()
            || self.exists(&format!("constant/{AIR}/radiationProperties")) == Some(true);
        if !radiation {
            return refuse("the grass needs the radiation: it reads qr and qs on its patches".to_string());
        }
        if let Some(ps) = patches {
            for (i, p) in ps.iter().enumerate() {
                if !crate::description_edit::plain_name(p) {
                    return refuse(format!("grass patch `{p}`: not a plain name"));
                }
                if ps[..i].contains(p) {
                    return refuse(format!("grass patch {p} is given twice"));
                }
                if let Some(known) = &air_patches
                    && !known.iter().any(|x| x.name == *p)
                {
                    return refuse(format!("grass patch {p}: region {AIR} has no such patch"));
                }
            }
        }
        if !there {
            if fs.is_file(&rel) {
                return refuse(format!("{rel} doesn't load: switch the grass on in it by hand"));
            }
            self.put_new(fs, &rel, template(v, Template::Grass)?)?;
        }
        let doc = self.dict(&rel).ok_or_else(|| Error::NotLoaded { path: rel.clone() })?;
        // The model whose coefficients the file has (one of the solver's), else the
        // configuration's.
        let models = umcf_schema::catalog::catalog().choices("grassModel", v);
        let model = match doc.value(&["grassModel"]) {
            Some(m) if m != "none" => m,
            _ => doc
                .root
                .entries()
                .filter_map(|e| e.key().strip_suffix("Coeffs").filter(|_| e.dict().is_some()))
                .find(|m| models.contains(m))
                .map(str::to_string)
                .unwrap_or_else(|| "simpleGrass".to_string()),
        };
        let had = doc.value(&["grassModel"]);
        if had.as_deref() != Some(model.as_str()) {
            self.set(&rel, &["grassModel"], &model)?;
        }
        let list = [model.as_str(), "Coeffs"].concat();
        // What the configuration's file has and this one lacks (a file of `grassModel none;`
        // only): its coefficients, without the tutorial's patches.
        let mut filled = Vec::new();
        if there {
            let t = template(v, Template::Grass)?;
            for e in t.root.entries().filter(|e| !matches!(e.key(), "FoamFile" | "grassModel")) {
                let missing = self.dict(&rel).is_some_and(|d| d.get(&[e.key()]).is_none());
                let text = match e.dict() {
                    Some(d) => Some(inline(d)),
                    None => e.value_text(),
                };
                if let (true, Some(text)) = (missing, text) {
                    self.set_inline(&rel, &[e.key()], text.trim())?;
                    filled.push(e.key().to_string());
                }
            }
        }
        let switched_off = self
            .dict(&rel)
            .and_then(|d| d.value(&["grass"]))
            .is_some_and(|w| umcf_foam_dict::switch(&w) == Some(false));
        if switched_off {
            self.set(&rel, &["grass"], "true")?;
            s.notes.push(format!("{rel}: grass was off: switched on (grass true)"));
        }
        if filled.contains(&list) && patches.is_none() {
            let doc = self.dict_mut(&rel)?;
            doc.set_inline(&[&list, "grassPatches"], "( )").map_err(dict_error(&rel))?;
        }
        if !filled.is_empty() {
            s.notes.push(format!("{rel}: {} from the configuration", filled.join(", ")));
        }
        if let Some(ps) = patches {
            let value = format!("( {} )", ps.join(" "));
            let doc = self.dict_mut(&rel)?;
            doc.set_inline(&[&list, "grassPatches"], &value).map_err(dict_error(&rel))?;
        } else if !there {
            // The configuration's patches are the tutorial's: none of them this case's.
            let doc = self.dict_mut(&rel)?;
            doc.set_inline(&[&list, "grassPatches"], "( )").map_err(dict_error(&rel))?;
        }
        let none = self
            .dict(&rel)
            .and_then(|d| d.get(&[&list, "grassPatches"]))
            .and_then(|e| e.value_text())
            .is_none_or(|t| t.trim().trim_start_matches('(').trim_end_matches(')').trim().is_empty());
        if none {
            s.notes.push(format!("{rel}: no grass patches: name the patches the grass grows on"));
        }
        if !there || had.as_deref() != Some(model.as_str()) || patches.is_some() || !filled.is_empty() || switched_off {
            s.changed.push(rel);
        } else {
            s.notes.push(format!("{rel}: the grass is on already"));
        }
        Ok(())
    }

    /// The file fluid `r`'s limit is read from ([`crate::settings::limit_file`]), or one on disk
    /// that doesn't load in its place (and whether it is there); a new one goes into `system/`.
    fn limit_file(&self, fs: &dyn CaseFs, v: OfVersion, r: &str) -> (String, bool) {
        let order = crate::settings::limit_files(v, r);
        let read = crate::settings::limit_file(self, v, r);
        match order.into_iter().find(|x| read.as_ref() == Some(x) || fs.is_file(x)) {
            Some(rel) => (rel, true),
            None => match v {
                OfVersion::V12 => (format!("system/{r}/fvConstraints"), false),
                OfVersion::V8 => (format!("system/{r}/fvOptions"), false),
            },
        }
    }

    fn switch_limit(&mut self, fs: &dyn CaseFs, v: OfVersion, on: bool, s: &mut Switched) -> Result<()> {
        let fluids: Vec<String> =
            self.regions.iter().filter(|r| r.kind == RegionKind::Fluid).map(|r| r.name.clone()).collect();
        if fluids.is_empty() {
            return refuse("the case has no fluid region: the temperature limit is a fluid's".to_string());
        }
        let described = self.description().map(|(d, _)| d.settings.temperature_limit);
        if !on {
            // The bounds the case has, kept in the description when it has none; those not kept
            // are noted.
            let (had, _) = crate::settings::Settings::from_case(self);
            let kept = match (&had.temperature_limit, &described) {
                (Some((min, max)), Some(None)) => {
                    self.set_inline(PATH, &["settings", "temperatureLimit"], &format!("( {min} {max} )"))?;
                    s.notes.push(format!("its bounds ( {min} {max} ) kept in {PATH}'s settings"));
                    had.temperature_limit.clone()
                }
                (_, Some(Some(d))) => Some(d.clone()),
                _ => None,
            };
            for r in &fluids {
                for (rel, at) in crate::settings::limit_entries(self, r) {
                    let path: Vec<&str> = at.iter().map(String::as_str).collect();
                    let bound = |k: &str| {
                        let mut p = path.clone();
                        p.push(k);
                        self.dict(&rel).and_then(|d| d.value(&p)).unwrap_or_else(|| "?".into())
                    };
                    let (min, max) = (bound("min"), bound("max"));
                    if kept.as_ref() != Some(&(min.clone(), max.clone())) {
                        s.notes.push(format!("{rel}: {} had the bounds ( {min} {max} )", at[0]));
                    }
                    self.remove(&rel, &[at[0].as_str()])?;
                    if !s.changed.contains(&rel) {
                        s.changed.push(rel);
                    }
                }
            }
            // Entries in a file the solver doesn't read: left as they are, noted.
            let mut unread_noted = false;
            for r in &fluids {
                let read = crate::settings::limit_file(self, v, r);
                for f in ["fvOptions", "fvConstraints", "fvModels"] {
                    for rel in [format!("constant/{r}/{f}"), format!("system/{r}/{f}")] {
                        let unread = read.as_ref() != Some(&rel)
                            && self.dict(&rel).is_some_and(|d| {
                                d.root.entries().any(|e| {
                                    e.dict().and_then(|b| b.get("type")).and_then(|t| t.words()).is_some_and(
                                    |w| matches!(w.as_slice(), [t] if umcf_foam_dict::unquote(t) == "limitTemperature"),
                                )
                                })
                            });
                        if unread {
                            unread_noted = true;
                            s.notes.push(format!(
                                "{rel}: a limitTemperature entry in a file the solver doesn't read: left as it is"
                            ));
                        }
                    }
                }
            }
            if s.changed.is_empty() && !unread_noted {
                s.notes.push(
                    "the files the case's fluids' limits are read from have no limitTemperature entry: the case has no temperature limit"
                        .into(),
                );
            }
            return Ok(());
        }
        let (min, max) = match described.flatten() {
            Some((a, b)) => (a, b),
            None => (defaults::TEMPERATURE_LIMIT.0.to_string(), defaults::TEMPERATURE_LIMIT.1.to_string()),
        };
        for r in &fluids {
            let (rel, there) = self.limit_file(fs, v, r);
            // A limit counts where the solver reads it.
            let has = crate::settings::limit_entries(self, r).iter().any(|(f, _)| *f == rel);
            if has {
                continue;
            }
            if there && self.dict(&rel).is_none() {
                return refuse(format!("{rel} doesn't load: add the temperature limit in it by hand"));
            }
            if !there {
                let t = match v {
                    OfVersion::V12 => Template::FluidConstraints,
                    OfVersion::V8 => Template::FluidOptions,
                };
                let mut doc = template(v, t)?;
                // The configuration's entries are the tutorial's (OpenFOAM-8's is the limit).
                let keys: Vec<String> = doc.root.entries().map(|e| e.key().to_string()).collect();
                for k in keys.iter().filter(|k| *k != "FoamFile") {
                    doc.remove(&[k]).map_err(dict_error(&rel))?;
                }
                self.put_new(fs, &rel, doc)?;
            }
            let key = (0..)
                .map(|i| if i == 0 { "limitT".to_string() } else { format!("limitT{i}") })
                .find(|k| self.dict(&rel).is_some_and(|d| d.get(&[k.as_str()]).is_none()))
                .unwrap_or_default();
            let select = match v {
                OfVersion::V12 => "select all;",
                OfVersion::V8 => "selectionMode all;",
            };
            let entry = format!("{{ type limitTemperature; {select} min {min}; max {max}; }}");
            self.set_inline(&rel, &[key.as_str()], &entry)?;
            s.changed.push(rel);
        }
        if s.changed.is_empty() {
            s.notes.push("the case's fluids have a limitTemperature entry each already".into());
        }
        Ok(())
    }

    fn switch_blending(&mut self, on: bool, s: &mut Switched) -> Result<()> {
        let Some(doc) = self.dict(CONTROL_DICT) else {
            return refuse(format!("{CONTROL_DICT} isn't there or doesn't load"));
        };
        let now = doc
            .get(&["blending"])
            .and_then(|e| e.words())
            .is_some_and(|w| matches!(w.as_slice(), [x] if umcf_foam_dict::switch(x) == Some(true)));
        let has_coeffs = doc.get(&["blendingCoeffs"]).is_some();
        if !on {
            if now {
                self.set(CONTROL_DICT, &["blending"], "false")?;
                s.changed.push(CONTROL_DICT.to_string());
                if has_coeffs {
                    s.notes.push(format!("{CONTROL_DICT}: blendingCoeffs kept for switching it back on"));
                }
            } else {
                s.notes.push("the case has no blending layer".into());
            }
            return Ok(());
        }
        // The blending layer takes the sides along x and y: the gravity is along z.
        let g = self
            .regions
            .iter()
            .filter(|r| r.kind == RegionKind::Fluid)
            .find_map(|r| self.dict(&format!("constant/{}/g", r.name)).and_then(|d| d.value(&["value"])));
        if let Some(g) = g {
            let parts: Vec<f64> =
                g.trim_matches(|c| c == '(' || c == ')').split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if parts.len() == 3 && (parts[0] != 0.0 || parts[1] != 0.0 || parts[2] >= 0.0) {
                return refuse(format!(
                    "the blending layer needs z up (it takes the sides west and east along x, north and south along y): the gravity is {g}"
                ));
            }
        }
        // The solver's blending layer finds the sides by name in each fluid's mesh.
        const SIDES: [&str; 4] = ["west", "east", "north", "south"];
        for r in self.regions.iter().filter(|r| r.kind == RegionKind::Fluid) {
            match &r.patches {
                Some(ps) => {
                    let missing: Vec<&str> = SIDES.into_iter().filter(|w| !ps.iter().any(|p| p.name == *w)).collect();
                    if !missing.is_empty() {
                        return refuse(format!(
                            "the blending layer needs the patches west, east, north and south in each fluid's mesh: region {} has no {}",
                            r.name,
                            missing.join(", ")
                        ));
                    }
                }
                None => s.notes.push(format!(
                    "region {}: no mesh yet: the blending layer needs its patches west, east, north and south",
                    r.name
                )),
            }
        }
        if !now {
            self.set(CONTROL_DICT, &["blending"], "true")?;
        }
        if !has_coeffs {
            let chosen = self.description().and_then(|(d, _)| d.settings.blending);
            self.set_inline(CONTROL_DICT, &["blendingCoeffs"], defaults::BLENDING_COEFFS)?;
            if let Some(c) = chosen {
                for (k, v) in ["dampingThickness", "alphaCoeffU", "alphaCoeffT"].into_iter().zip(c.iter()) {
                    self.set(CONTROL_DICT, &["blendingCoeffs", k], v)?;
                }
            }
        }
        if !now || !has_coeffs {
            s.changed.push(CONTROL_DICT.to_string());
        } else {
            s.notes.push("the case has the blending layer already".into());
        }
        Ok(())
    }
}

/// Dictionary `d` on one line: `{ Cd 0.2; grassPatches ( a b ); }`.
fn inline(d: &umcf_foam_dict::Dict) -> String {
    let mut parts = Vec::new();
    for e in d.entries() {
        match e.dict() {
            Some(inner) => parts.push(format!("{} {}", e.key(), inline(inner))),
            None => parts.push(format!("{} {};", e.key(), e.value_text().unwrap_or_default().trim())),
        }
    }
    format!("{{ {} }}", parts.join(" "))
}

#[cfg(test)]
#[path = "components_tests.rs"]
mod tests;
