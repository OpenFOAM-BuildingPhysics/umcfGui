//! A region added to a case (`region.add`): its name in the region list the case's version
//! reads, the settings files of its kind (as `case.new`
//! writes them), the region in the case as one without a mesh yet, and in the workflow
//! description when the case has one.

use std::sync::Arc;

use umcf_core::{CaseFs, OfVersion};
use umcf_foam_dict::Document;
use umcf_schema::Template;

use crate::new_case::{NewRegion, Up, check_region, region_files, template};
use crate::{CONTROL_DICT, Case, Error, Region, RegionKind, Result};

/// The OpenFOAM-8 region list.
const REGION_PROPERTIES: &str = "constant/regionProperties";

/// What [`Case::add_region`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AddedRegion {
    /// The file whose region list names it now.
    pub listed_in: String,
    /// The files it wrote (not saved yet).
    pub written: Vec<String>,
    /// The files of its kind that were there already: kept as they are.
    pub kept: Vec<String>,
    /// Whether the workflow description names it now (the case has one).
    pub described: bool,
    /// What is left to do (a material without coefficients, the vegetation's radiation).
    pub notes: Vec<String>,
}

fn refuse<T>(message: String) -> Result<T> {
    Err(Error::NewCase(message))
}

fn kind_word(k: RegionKind) -> &'static str {
    match k {
        RegionKind::Solid => "solid",
        RegionKind::Vegetation => "vegetation",
        RegionKind::Fluid | RegionKind::Single => "fluid",
    }
}

impl Case {
    /// Adds region `r` to the case: its name in the region list (OpenFOAM-12's `controlDict`,
    /// OpenFOAM-8's `constant/regionProperties`), the settings files of its kind (the
    /// configuration's, as `case.new` writes them): a fluid's with the gravity of the case's
    /// fluids and the RAS model and temperature-limit bounds of the description's settings, else
    /// of the case's fluids when they agree; the vegetation's model in `air` when it isn't there.
    /// A file that is there already is kept. The region is in the case without a mesh, and in
    /// the workflow description when the case has one. [`Case::save`] writes it all. Refused,
    /// with nothing changed: a case without a version or a region list, a name the case has (or
    /// one differing only by case), what `case.new` refuses of a region, a link where a file
    /// would go.
    pub fn add_region(&mut self, fs: &dyn CaseFs, r: &NewRegion) -> Result<AddedRegion> {
        // Made on a copy, which becomes the case once all of it is done: a refusal half-way
        // leaves the case as it was. (A copy is cheap: the files are shared until changed.)
        let mut next = self.clone();
        let added = next.add_region_to(fs, r)?;
        *self = next;
        Ok(added)
    }

    fn add_region_to(&mut self, fs: &dyn CaseFs, r: &NewRegion) -> Result<AddedRegion> {
        let Some(v) = self.layout.version else {
            return refuse("the case's OpenFOAM version isn't known: its regions can't be listed".to_string());
        };
        let listed_in = match v {
            OfVersion::V12 => CONTROL_DICT,
            OfVersion::V8 => REGION_PROPERTIES,
        };
        if self.dict(listed_in).is_none() || self.layout.regions_from.as_deref() != Some(listed_in) {
            return refuse(format!(
                "the case has no region list in {listed_in}, where {} reads it (a meshing step?)",
                v.label()
            ));
        }
        let description = self.description().map(|(d, _)| d);
        let mut notes = Vec::new();
        // A solid described already brings its materials, when none are given.
        let mut region = r.clone();
        if region.kind == RegionKind::Solid
            && region.materials.is_empty()
            && let Some(d) = description.as_ref().and_then(|d| d.region(&r.name))
        {
            for m in &d.materials {
                match m.model {
                    Some(_) => region.materials.push(m.clone()),
                    None => notes.push(format!(
                        "region {}, material {}: the description gives it no model: add it with workflow.material and to transportProperties",
                        r.name, m.name
                    )),
                }
            }
            if !region.materials.is_empty() {
                notes.push(format!("region {}: its materials are the description's", r.name));
            }
        }
        let from_description = r.materials.is_empty() && !region.materials.is_empty();
        let r = &region;
        check_region(r, v).map_err(|e| match e {
            Error::NewCase(m) if from_description => Error::NewCase(format!(
                "{m}: the material is the description's (fix it there, or give the region its materials)"
            )),
            e => e,
        })?;
        let n = r.name.as_str();
        if let Some(other) = self.regions.iter().find(|x| x.name.eq_ignore_ascii_case(n)) {
            return if other.name == n {
                refuse(format!("region {n} is there already"))
            } else {
                refuse(format!(
                    "region {n}: region {} differs from it only by case (they can't live together on Windows or macOS)",
                    other.name
                ))
            };
        }
        let components = match &description {
            Some(d) => d.components,
            None => crate::description::case_components(self),
        };
        // The gravity of the case's fluids, so that a new one falls the same way.
        let gravity = self
            .regions
            .iter()
            .filter(|x| x.kind == RegionKind::Fluid)
            .find_map(|x| self.dict(&format!("constant/{}/g", x.name)).and_then(|d| d.value(&["value"])))
            // A vector written out, not a macro or a directive the new file wouldn't have.
            .filter(|g| g.starts_with('(') && !g.contains(['$', '#']))
            .unwrap_or_else(|| Up::Z.gravity().to_string());
        // A new fluid is turbulent and limited as the description chose, else as the case's
        // fluids all are, else as the configuration has it (with a note).
        let (mut case_settings, case_notes) = crate::settings::Settings::from_case(self);
        let mut chosen = description.as_ref().map(|d| d.settings.clone()).unwrap_or_default();
        // A model written into the description by hand that the version doesn't have.
        // (One the case's fluids all have already, of its own library say, is kept.)
        if chosen.turbulence != case_settings.turbulence
            && let Err(Error::NewCase(m)) = crate::settings::check_turbulence(&chosen, v)
        {
            notes.push(format!("{}: {m}: left out", crate::description::PATH));
            chosen.turbulence = None;
        }
        if chosen.turbulence.is_some() {
            case_settings.turbulence = chosen.turbulence.clone();
        }
        if chosen.temperature_limit.is_some() {
            case_settings.temperature_limit = chosen.temperature_limit.clone();
        }
        // A model written into the description's library by hand that the solver doesn't have.
        let bad: Vec<String> = chosen
            .materials
            .iter()
            .filter(|(m, _)| {
                crate::settings::check_materials(
                    &crate::settings::Settings { materials: vec![(m.clone(), Vec::new())], ..Default::default() },
                    v,
                )
                .is_err()
            })
            .map(|(m, _)| m.clone())
            .collect();
        for m in &bad {
            notes.push(format!(
                "{}: materials/{m} is not a material model of the solver for {}: left out",
                crate::description::PATH,
                v.label()
            ));
        }
        chosen.materials.retain(|(m, _)| !bad.contains(m));
        // The library the description chose first, then what the case's solids have.
        for (model, coeffs) in chosen.materials.iter().rev() {
            case_settings.materials.retain(|(m, _)| m != model);
            case_settings.materials.insert(0, (model.clone(), coeffs.clone()));
        }
        if r.kind == RegionKind::Fluid {
            let open = |n: &&String| {
                (n.contains("RAS model") && chosen.turbulence.is_none())
                    || (n.contains("temperature limit") && chosen.temperature_limit.is_none())
            };
            for n in case_notes.iter().filter(open) {
                notes.push(format!("{n}: region {} gets the configuration's", r.name));
            }
        }
        let mut files = region_files(v, r, &gravity, components.temperature_limit, &case_settings, &mut notes)?;
        if r.kind == RegionKind::Vegetation {
            if self.region("air").is_some_and(|a| a.kind == RegionKind::Fluid) {
                files.push((
                    "constant/air/vegetationProperties".to_string(),
                    template(v, Template::Vegetation)?.to_string(),
                ));
            } else {
                notes.push("the trees' model is air's: the case has no fluid region named air".to_string());
            }
            let on = description.as_ref().and_then(|d| d.radiation.as_ref()).map(|x| x.region.as_str());
            if on != Some("vegetation") {
                notes.push(
                    "the vegetation needs the radiation on it (radiationProperties and solarLoadProperties in constant/vegetation/): the trees' model reads its qr on air_to_vegetation"
                        .to_string(),
                );
            }
        }
        // Every file checked before anything changes.
        let mut written = Vec::new();
        let mut kept = Vec::new();
        let mut docs = Vec::new();
        for (rel, text) in files {
            let there = self.dict(&rel).is_some()
                || self.raw.contains_key(&rel)
                || fs.is_file(&rel)
                || fs.is_dir(&rel)
                || fs.is_link(&rel);
            if there {
                // A link to nothing is there too: the save would write the file it points to.
                if fs.is_link(&rel) && !fs.is_file(&rel) {
                    return Err(Error::IsLink(rel));
                }
                if fs.is_dir(&rel) {
                    return refuse(format!("{rel} is a directory: the region's file can't go there"));
                }
                kept.push(rel);
                continue;
            }
            let doc = Document::parse(&text).map_err(|source| Error::Dict { path: rel.clone(), source })?;
            written.push(rel.clone());
            docs.push((rel, doc));
        }
        let word = kind_word(r.kind);
        let list = self.dict_mut(listed_in)?;
        match v {
            OfVersion::V12 => {
                let path = ["regions", word];
                let edit = if list.get(&path).is_some() {
                    // In the list as it is laid out, its comments kept.
                    list.add_word(&path, n)
                } else {
                    list.set_inline(&path, &format!("( {n} )"))
                };
                if edit.is_err() {
                    return refuse(format!("{listed_in}: regions/{word} isn't a plain list of names: add {n} by hand"));
                }
            }
            OfVersion::V8 => {
                list.add_to_sublist("regions", word, n)
                    .map_err(|source| Error::Dict { path: listed_in.to_string(), source })?;
            }
        }
        self.edited(listed_in);
        for (rel, doc) in docs {
            self.files.insert(rel, Arc::new(doc));
        }
        let names = match r.kind {
            RegionKind::Solid => &mut self.layout.regions.solid,
            RegionKind::Vegetation => &mut self.layout.regions.vegetation,
            RegionKind::Fluid | RegionKind::Single => &mut self.layout.regions.fluid,
        };
        names.push(n.to_string());
        // After the last region of its kind, in the order fluids, solids, vegetation.
        let rank = |k: RegionKind| match k {
            RegionKind::Fluid | RegionKind::Single => 0,
            RegionKind::Solid => 1,
            RegionKind::Vegetation => 2,
        };
        let at = self.regions.iter().rposition(|x| rank(x.kind) <= rank(r.kind)).map_or(0, |i| i + 1);
        self.regions.insert(
            at,
            Region {
                name: n.to_string(),
                kind: r.kind,
                patches: None,
                counts: Default::default(),
                fields: Vec::new(),
                cell_zones: None,
                zones_from: None,
            },
        );
        let described = description.is_some();
        if !described && fs.is_file(crate::description::PATH) {
            notes.push(format!(
                "{} is there but doesn't load: the region isn't described in it (workflow.show says why)",
                crate::description::PATH
            ));
        }
        if described {
            self.describe_region(n, r.kind)?;
            for m in &r.materials {
                self.describe_material(n, &m.name, Some(m.model.as_deref()))?;
            }
        }
        Ok(AddedRegion { listed_in: listed_in.to_string(), written, kept, described, notes })
    }
}

#[cfg(test)]
#[path = "region_add_tests.rs"]
mod tests;
