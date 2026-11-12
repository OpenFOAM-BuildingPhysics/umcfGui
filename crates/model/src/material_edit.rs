//! A material's coefficients edited in its solid's `constant/<region>/transportProperties`
//! (`material.set`): the entries of its dictionary in `buildingMaterials`, one material at a
//! time, nothing else in the file changing.

use umcf_foam_dict::Node;

use crate::{Case, Error, RegionKind, Result};

/// A coefficient: its path in the material's dictionary (one name: `rho`, `wcap`; the solver
/// reads every coefficient there, none in a sub-dictionary) and its value, a number; `None`
/// takes it out.
pub type Coefficient = (Vec<String>, Option<String>);

/// What [`Case::set_material`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaterialEdit {
    /// The file changed (not saved yet); `None` when every value was there already.
    pub changed: Option<String>,
    /// The material's model (`buildingMaterialModel`).
    pub model: Option<String>,
    /// What else to know (a name listed twice, a coefficient taken out that wasn't there).
    pub notes: Vec<String>,
}

fn refuse<T>(message: String) -> Result<T> {
    Err(Error::NewCase(message))
}

/// A word a coefficient's name may be: letters, digits and `_`, starting with a letter.
fn coefficient_name(k: &str) -> bool {
    k.starts_with(|c: char| c.is_ascii_alphabetic()) && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The most materials, and coefficients of a material, [`Case::material_coefficients`] lists.
pub const MAX_LISTED: usize = 1000;

/// A solid's material as its `transportProperties` lists it: its name, its model and its
/// coefficients (the plain entries of its dictionary but its name and model, as their text).
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialCoefficients {
    pub name: String,
    pub model: Option<String>,
    pub coefficients: Vec<(String, String)>,
}

/// The coefficients the solver reads of a material of `model` for `version` (either version's
/// when it isn't known): every material's, then the model's own (the catalog's), each once.
pub fn coefficient_names(version: Option<umcf_core::OfVersion>, model: Option<&str>) -> Vec<&'static str> {
    let versions: Vec<umcf_core::OfVersion> = match version {
        Some(v) => vec![v],
        None => umcf_core::OfVersion::ALL.to_vec(),
    };
    let mut out: Vec<&'static str> = Vec::new();
    for v in &versions {
        for k in crate::settings::required_coefficients(*v) {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    let cat = umcf_schema::catalog::catalog();
    for s in cat.startup_keywords {
        if s.family == "buildingMaterialModel" && Some(s.type_name) == model && !out.contains(&s.name) {
            out.push(s.name);
        }
    }
    out
}

impl Case {
    /// The materials solid region `region` lists in `constant/<region>/transportProperties`'s
    /// `buildingMaterials`, in its order (none without the file or the list; an item that isn't
    /// a dictionary left out).
    pub fn material_coefficients(&self, region: &str) -> Vec<MaterialCoefficients> {
        let rel = format!("constant/{region}/transportProperties");
        let Some(doc) = self.dict(&rel) else { return Vec::new() };
        let Some(list) = doc.root.get("buildingMaterials") else { return Vec::new() };
        let [Node::List { items, .. }] = list.value.as_slice() else { return Vec::new() };
        // At most so many materials, and coefficients a material, of a file that may be anything.
        items
            .iter()
            .take(MAX_LISTED)
            .filter_map(|n| match n {
                Node::Dict { body, .. } => {
                    let text = |k: &str| body.get(k).and_then(|e| e.value_text()).map(|t| t.trim().to_string());
                    let coefficients = body
                        .entries()
                        .filter(|e| !matches!(e.key(), "name" | "buildingMaterialModel") && e.dict().is_none())
                        .filter_map(|e| Some((e.key().to_string(), e.value_text()?.trim().to_string())))
                        .take(MAX_LISTED)
                        .collect();
                    Some(MaterialCoefficients {
                        name: text("name")?,
                        model: text("buildingMaterialModel"),
                        coefficients,
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// Sets (or takes out) the coefficients `coeffs` of material `material` of solid region
    /// `region`, in its dictionary in `constant/<region>/transportProperties`'s
    /// `buildingMaterials`: a coefficient there is changed in place, a new one goes at the end
    /// of the material's dictionary, flat as the solver reads every coefficient (a name the
    /// catalog doesn't list for the model noted). Refused, with nothing changed: a region that
    /// isn't a solid or has no `buildingMaterials` list, a material it doesn't list or lists
    /// twice (the solver stops), one whose dictionary has a macro or a directive (what it gives
    /// can't be told), a name that isn't a plain word, `name` or `buildingMaterialModel` (the
    /// material's name and model aren't coefficients), a value that isn't a number, a
    /// coefficient the solver reads of every material taken out (`rho`, `cap`, `lambda1`,
    /// `lambda2`), a value where the material has a dictionary.
    /// [`Case::save`] writes it.
    pub fn set_material(&mut self, region: &str, material: &str, coeffs: &[Coefficient]) -> Result<MaterialEdit> {
        let mut next = self.clone();
        let e = next.set_material_to(region, material, coeffs)?;
        *self = next;
        Ok(e)
    }

    fn set_material_to(&mut self, region: &str, material: &str, coeffs: &[Coefficient]) -> Result<MaterialEdit> {
        match self.region(region) {
            Some(r) if r.kind == RegionKind::Solid => {}
            Some(_) => return refuse(format!("region {region} isn't a solid: its materials are a solid's")),
            None => return refuse(format!("the case has no region {region}")),
        }
        if coeffs.is_empty() {
            return refuse("give a coefficient".to_string());
        }
        // A version not known: those of either (the solver stops without one of them).
        let required: Vec<&str> = match self.layout.version {
            Some(v) => crate::settings::required_coefficients(v),
            None => {
                let mut r: Vec<&str> = [umcf_core::OfVersion::V8, umcf_core::OfVersion::V12]
                    .into_iter()
                    .flat_map(crate::settings::required_coefficients)
                    .collect();
                r.sort_unstable();
                r.dedup();
                r
            }
        };
        for (at, v) in coeffs {
            let shown = at.join("/");
            let Some(name) = (match at.as_slice() {
                [name] if coefficient_name(name) => Some(name),
                _ => None,
            }) else {
                return refuse(format!(
                    "{shown}: a coefficient is a plain name in the material's dictionary (the solver reads them there, as VanGenuchten's n and wcap; none in a sub-dictionary)"
                ));
            };
            if matches!(name.as_str(), "name" | "buildingMaterialModel") {
                return refuse(format!(
                    "{shown} isn't a coefficient: the material's name and model are workflow.material's"
                ));
            }
            match v {
                Some(v) if v.parse::<f64>().map_or(true, |x| !x.is_finite()) => {
                    return refuse(format!("{shown}: {v} is not a number"));
                }
                None if required.iter().any(|r| r == name) => {
                    return refuse(format!(
                        "{shown}: the solver reads it of every material ({}): it can't be taken out",
                        required.join(", ")
                    ));
                }
                _ => {}
            }
            if coeffs.iter().filter(|(a, _)| a == at).count() > 1 {
                return refuse(format!("{shown} is given twice"));
            }
        }
        let rel = format!("constant/{region}/transportProperties");
        if self.dict(&rel).is_none() {
            return refuse(format!("{rel} isn't there or doesn't load"));
        }
        let mut notes = Vec::new();
        let doc = self.dict_mut(&rel)?;
        let Some(list) = doc.root.get_mut("buildingMaterials") else {
            return refuse(format!("{rel} has no buildingMaterials list"));
        };
        let [Node::List { items, .. }] = list.value.as_mut_slice() else {
            return refuse(format!("{rel}: buildingMaterials isn't a list of dictionaries"));
        };
        // The materials named so: the solver gives each its cell zone, and stops on a second.
        let named: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, n)| match n {
                Node::Dict { body, .. } => body.get("name").and_then(|e| e.value_text()).as_deref() == Some(material),
                _ => false,
            })
            .map(|(i, _)| i)
            .collect();
        let Some(&at) = named.first() else {
            return refuse(format!("{rel} lists no material {material}"));
        };
        if named.len() > 1 {
            return refuse(format!(
                "{rel} lists material {material} {} times: the solver stops on a cell zone given two materials (mend the file first)",
                named.len()
            ));
        }
        let not_a_dict = || refuse(format!("{rel}: material {material} isn't a dictionary"));
        let Some(Node::Dict { body, .. }) = items.get(at) else {
            return not_a_dict();
        };
        if body.items.iter().any(|i| !matches!(i, umcf_foam_dict::Item::Entry(_))) {
            return refuse(format!(
                "{rel}, material {material}: its dictionary has a macro or a directive: what it gives can't be told (edit it by hand)"
            ));
        }
        let model = body.get("buildingMaterialModel").and_then(|e| e.value_text()).map(|t| t.trim().to_string());
        // What the solver reads of a material of this model: every material's, and the model's
        // own (the catalog's). Another name is written, with a note.
        let cat = umcf_schema::catalog::catalog();
        let known = |k: &str| {
            required.contains(&k)
                || cat.startup_keywords.iter().any(|s| {
                    s.family == "buildingMaterialModel" && Some(s.type_name) == model.as_deref() && s.name == k
                })
        };
        let mut changed = false;
        for (path, v) in coeffs {
            let p: Vec<&str> = path.iter().map(String::as_str).collect();
            let shown = path.join("/");
            let Some(Node::Dict { body, .. }) = items.get(at) else {
                return not_a_dict();
            };
            let there = body.get_path(&p);
            if there.is_some_and(|e| e.dict().is_some()) {
                return refuse(format!("{rel}, material {material}: {shown} is a dictionary, not a coefficient"));
            }
            match v {
                Some(v) => {
                    if there.and_then(|e| e.value_text()).as_deref().map(str::trim) == Some(v.as_str()) {
                        continue;
                    }
                    if there.is_none() && !known(&shown) {
                        notes.push(format!(
                            "{shown}: not a coefficient the solver's catalog lists for {} (a typo?): written all the same",
                            model.as_deref().unwrap_or("its model")
                        ));
                    }
                    let Some(item) = items.get_mut(at) else { return not_a_dict() };
                    item.dict_set(&p, v).map_err(|source| Error::Dict { path: rel.clone(), source })?;
                    changed = true;
                }
                None => {
                    let Some(item) = items.get_mut(at) else { return not_a_dict() };
                    let removed = item.dict_remove(&p).map_err(|source| Error::Dict { path: rel.clone(), source })?;
                    if removed {
                        changed = true;
                    } else {
                        notes.push(format!("{shown}: material {material} has none: nothing taken out"));
                    }
                }
            }
        }
        Ok(MaterialEdit { changed: changed.then_some(rel), model, notes })
    }
}

#[cfg(test)]
#[path = "material_edit_tests.rs"]
mod tests;
