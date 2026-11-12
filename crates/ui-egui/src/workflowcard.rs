//! The workflow description's cards (`workflow.region`, `workflow.patch`, `workflow.material`,
//! `workflow.component`, `mesh.split`): each frame, the names their fields offer (the description's regions,
//! a region's patches and materials, the material models of the solver, the air's patches), and
//! what they describe filled in when it is chosen (a region's kind, a patch's role and partner,
//! a material's model).

use umcf_model::RegionKind;
use umcf_model::description::Description;

use crate::App;
use crate::forms::{FormState, PARTNER_PATCHES};

/// The cards this fills.
pub(crate) const CARDS: [&str; 9] = [
    "workflow.region",
    "workflow.patch",
    "workflow.material",
    "workflow.component",
    "material.set",
    "case.new",
    "region.add",
    "workflow.settings",
    "mesh.split",
];

/// What a card is filled for: the region, and the patch or material, it describes.
pub(crate) fn fill_key(id: &str, f: &FormState) -> String {
    let v = |k: &str| f.values.get(k).cloned().unwrap_or_default();
    match id {
        "workflow.patch" => format!("{}/{}", v("region"), v("patch")),
        "workflow.material" | "material.set" => format!("{}/{}", v("region"), v("material")),
        "workflow.component" => v("component"),
        // Nothing the case says fills them.
        "case.new" | "region.add" | "workflow.settings" => String::new(),
        _ => v("region"),
    }
}

fn kind_name(k: RegionKind) -> &'static str {
    match k {
        RegionKind::Fluid => "fluid",
        RegionKind::Solid => "solid",
        RegionKind::Vegetation => "vegetation",
        RegionKind::Single => "",
    }
}

/// The patches of region `r` of the description.
fn patches_of(d: &Description, r: &str) -> Vec<String> {
    d.region(r).map(|r| r.patches.iter().map(|p| p.name.clone()).collect()).unwrap_or_default()
}

/// What the description says of the patch or material the card names: its role and partner, or
/// its model, as the fields hold them (empty for one it doesn't give); `None` when the
/// description doesn't have it.
fn described(
    id: &str,
    case: Option<&umcf_model::Case>,
    d: Option<&Description>,
    region: &str,
    f: &FormState,
) -> Option<Vec<(&'static str, String)>> {
    let value = |k: &str| f.values.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
    // A material's coefficients, as its solid's transportProperties has them.
    if id == "material.set" {
        let m = case?.material_coefficients(region).into_iter().find(|m| m.name == value("material"))?;
        let rows: Vec<String> = m
            .coefficients
            .iter()
            .filter(|(k, v)| !k.contains('|') && !v.contains('|'))
            .map(|(k, v)| format!("{k}|{v}"))
            .collect();
        return Some(vec![("coefficients", rows.join("\n"))]);
    }
    let r = d?.region(region)?;
    match id {
        "workflow.region" => Some(vec![("kind", kind_name(r.kind).to_string())]),
        "workflow.patch" => {
            let p = r.patches.iter().find(|p| p.name == value("patch"))?;
            let role = p.role.map(|x| x.name().to_string()).unwrap_or_default();
            // A name with the parts' separator can't be shown as a partner's two parts.
            let partner = p
                .partner
                .as_ref()
                .filter(|(a, b)| !a.contains('|') && !b.contains('|'))
                .map(|(a, b)| format!("{a}|{b}"))
                .unwrap_or_default();
            Some(vec![("role", role), ("partner", partner)])
        }
        "workflow.material" => {
            let m = r.materials.iter().find(|m| m.name == value("material"))?;
            Some(vec![("model", m.model.clone().unwrap_or_default())])
        }
        "mesh.split" => match &r.mesh {
            // The copied region's own dictionary left empty: it follows *Split from*.
            Some(umcf_model::workflow::MeshBuild::Split(sp)) => Some(vec![
                ("of", sp.of.clone()),
                ("zone", sp.zone.clone().unwrap_or_default()),
                (
                    "dict",
                    if sp.dict == umcf_model::workflow::Split::default_dict(&sp.of) {
                        String::new()
                    } else {
                        sp.dict.clone()
                    },
                ),
            ]),
            _ => None,
        },
        _ => None,
    }
}

/// The fields a patch or material card fills from the description.
fn card_fields(id: &str) -> &'static [&'static str] {
    match id {
        "workflow.region" => &["kind"],
        "workflow.patch" => &["role", "partner"],
        "workflow.material" => &["model"],
        "material.set" => &["coefficients"],
        "mesh.split" => &["of", "zone", "dict"],
        _ => &[],
    }
}

/// The patches the air's grass grows on, as `constant/air/grassProperties` names them (its
/// model's coefficients, `grassPatches`); none without the file.
fn grass_patches(case: Option<&umcf_model::Case>) -> std::collections::BTreeSet<String> {
    let Some(doc) = case.and_then(|c| c.dict("constant/air/grassProperties")) else { return Default::default() };
    // As the command takes the model: the file's, else the first of the solver's whose
    // coefficients it has (for the case's version, or either when it isn't known).
    let cat = umcf_schema::catalog::catalog();
    let models: Vec<&str> = match case.and_then(|c| c.layout.version) {
        Some(v) => cat.choices("grassModel", v),
        None => umcf_core::OfVersion::ALL.iter().flat_map(|v| cat.choices("grassModel", *v)).collect(),
    };
    let model = match doc.value(&["grassModel"]) {
        Some(m) if m != "none" => Some(m),
        _ => doc.root.entries().find_map(|e| {
            e.key().strip_suffix("Coeffs").filter(|m| e.dict().is_some() && models.contains(m)).map(str::to_string)
        }),
    };
    let Some(model) = model else { return Default::default() };
    let list = [model.as_str(), "Coeffs"].concat();
    doc.get(&[list.as_str(), "grassPatches"])
        .and_then(|e| e.value_text())
        .map(|t| t.replace(['(', ')'], " ").split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

impl App {
    /// After a command, on a region, patch or material card typed in: the fields not typed in follow
    /// what the description says now, and what they were filled with moves to it, so OK
    /// doesn't put back the old (an undo, a command run elsewhere).
    pub(crate) fn workflow_rebase(&mut self) {
        let Some(id) = self.dialog.as_ref().map(|d| d.id) else { return };
        if id == "workflow.component" {
            // The grass's ticks not changed follow the case's file; what they were filled with
            // moves to it.
            let now = grass_patches(self.session.case());
            let Some(f) = self.dialog.as_mut().and_then(|d| d.form.as_mut()).filter(|f| !f.json) else { return };
            if f.values.get("component").map(|c| c.trim()) == Some("grass") {
                if f.lists.get("patches") == f.initial_lists.get("patches") {
                    f.lists.insert("patches", now.clone());
                }
                f.initial_lists.insert("patches", now);
            }
            return;
        }
        if !matches!(id, "workflow.region" | "workflow.patch" | "workflow.material" | "material.set" | "mesh.split") {
            return;
        }
        let case = self.session.case();
        let description = case.and_then(|c| c.description()).map(|(d, _)| d);
        let Some(f) = self.dialog.as_mut().and_then(|d| d.form.as_mut()).filter(|f| !f.json) else { return };
        let region = f.values.get("region").map(|v| v.trim().to_string()).unwrap_or_default();
        // Gone from the description (an undo of adding it): what the form holds is all new.
        let Some(now) = described(id, case, description.as_ref(), &region, f) else {
            // What was filled in from it and not changed since goes with it.
            for k in card_fields(id) {
                if !f.extra.contains_key(*k) && f.values.get(k) == f.initial.get(k) {
                    f.values.insert(k, String::new());
                }
                f.initial.insert(k, String::new());
            }
            return;
        };
        for (k, v) in now {
            let was = f.initial.get(k).cloned().unwrap_or_default();
            let value = f.values.get(k).cloned().unwrap_or_default();
            if !f.extra.contains_key(k) && value == was {
                f.values.insert(k, v.clone());
            } else if k == "coefficients" && !f.extra.contains_key(k) {
                // A table: the rows not typed in follow the case, row by row.
                f.values.insert(k, crate::forms::merge_rows(&value, &was, &v, 2));
            }
            f.initial.insert(k, v);
        }
    }

    /// Each frame while a workflow card shows its form: what its fields offer, and the fields
    /// filled from the description when what they describe was chosen.
    pub(crate) fn workflow_card_frame(&mut self) {
        let Some(id) = self.dialog.as_ref().filter(|d| d.form.as_ref().is_some_and(|f| !f.json)).map(|d| d.id) else {
            return;
        };
        if !CARDS.contains(&id) {
            return;
        }
        let Some(spec) = crate::forms::spec(id) else { return };
        let case = self.session.case();
        let description = case.and_then(|c| c.description()).map(|(d, _)| d);
        let version = case.and_then(|c| c.layout.version);
        let Some(f) = self.dialog.as_mut().and_then(|d| d.form.as_mut()) else { return };
        let value = |f: &FormState, k: &str| f.values.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
        let mut options = std::collections::BTreeMap::new();
        if let Some(d) = &description {
            let regions: Vec<String> = d.regions.iter().map(|r| r.name.clone()).collect();
            let region = value(f, "region");
            match id {
                "workflow.region" => {
                    options.insert("region", regions);
                }
                "workflow.patch" => {
                    options.insert("patch", patches_of(d, &region));
                    let partner = f.values.get("partner").and_then(|p| p.split('|').next()).unwrap_or("").trim();
                    options.insert(PARTNER_PATCHES, patches_of(d, partner));
                    options.insert("partner", regions);
                }
                "mesh.split" => {
                    // Only the regions listed before it (their meshes are built first), and the
                    // zones the dictionary makes (the one typed, else the copied region's).
                    let before: Vec<String> =
                        d.regions.iter().take_while(|r| r.name != region).map(|r| r.name.clone()).collect();
                    let of = value(f, "of");
                    let dict = match value(f, "dict") {
                        d if d.is_empty() => umcf_model::workflow::Split::default_dict(&of),
                        d => d,
                    };
                    let zones = case.and_then(|c| c.dict(&dict)).map(umcf_model::workflow::zones_made);
                    // None for the first region: the field's hint says why.
                    options.insert("of", before);
                    options.insert("zone", zones.unwrap_or_default());
                }
                "workflow.material" => {
                    let solids =
                        d.regions.iter().filter(|r| r.kind == RegionKind::Solid).map(|r| r.name.clone()).collect();
                    options.insert("region", solids);
                    let materials = d
                        .region(&region)
                        .map(|r| r.materials.iter().map(|m| m.name.clone()).collect())
                        .unwrap_or_default();
                    options.insert("material", materials);
                }
                _ => {}
            }
        }
        if id == "workflow.material" {
            let cat = umcf_schema::catalog::catalog();
            let mut models: Vec<String> = match version {
                Some(v) => cat.choices("buildingMaterialModel", v).into_iter().map(str::to_string).collect(),
                None => umcf_core::OfVersion::ALL
                    .iter()
                    .flat_map(|v| cat.choices("buildingMaterialModel", *v))
                    .map(str::to_string)
                    .collect(),
            };
            models.sort();
            models.dedup();
            options.insert("model", models);
        }
        match id {
            "material.set" => {
                // The case's solids, the chosen one's materials, and the coefficients the solver
                // reads of the chosen material's model.
                let region = value(f, "region");
                let solids = case
                    .map(|c| c.regions.iter().filter(|r| r.kind == RegionKind::Solid).map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                let materials = case.map(|c| c.material_coefficients(&region)).unwrap_or_default();
                let chosen = materials.iter().find(|m| m.name == value(f, "material"));
                let names =
                    umcf_model::material_edit::coefficient_names(version, chosen.and_then(|m| m.model.as_deref()));
                options.insert("coefficients", names.into_iter().map(str::to_string).collect());
                options.insert("material", materials.iter().map(|m| m.name.clone()).collect());
                options.insert("region", solids);
            }
            "case.new" => {
                // The radiation goes on a fluid or the vegetation of the table.
                let rows = f.values.get("regions").cloned().unwrap_or_default();
                let names = rows
                    .split('\n')
                    .filter_map(|r| {
                        let mut c = r.split('|').map(str::trim);
                        let (name, kind) = (c.next()?, c.next().unwrap_or(""));
                        (!name.is_empty() && matches!(kind, "fluid" | "vegetation")).then(|| name.to_string())
                    })
                    .collect();
                options.insert("radiation", names);
            }
            "region.add" => {
                let cat = umcf_schema::catalog::catalog();
                let mut models: Vec<String> = match version {
                    Some(v) => cat.choices("buildingMaterialModel", v).into_iter().map(str::to_string).collect(),
                    None => umcf_core::OfVersion::ALL
                        .iter()
                        .flat_map(|v| cat.choices("buildingMaterialModel", *v))
                        .map(str::to_string)
                        .collect(),
                };
                models.sort();
                models.dedup();
                options.insert("materials", models);
            }
            "workflow.settings" => {
                let mut models: Vec<String> = match version {
                    Some(v) => umcf_model::settings::ras_models(v).into_iter().map(str::to_string).collect(),
                    None => umcf_core::OfVersion::ALL
                        .iter()
                        .flat_map(|v| umcf_model::settings::ras_models(*v))
                        .map(str::to_string)
                        .collect(),
                };
                models.sort();
                models.dedup();
                options.insert("turbulence", models);
            }
            _ => {}
        }
        if id == "workflow.component" {
            // The air's patches: its mesh's, and those the description gives it.
            let mut names: Vec<String> = case
                .and_then(|c| c.regions.iter().find(|r| r.name == "air"))
                .and_then(|r| r.patches.as_ref())
                .map(|p| p.iter().map(|p| p.name.clone()).collect())
                .unwrap_or_default();
            for n in description.as_ref().map(|d| patches_of(d, "air")).unwrap_or_default() {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            options.insert("patches", names);
        }
        f.options = options;
        // Filled from the description when what the card describes was chosen (what was typed
        // for another goes): what is left as filled is what OK leaves out.
        let key = fill_key(id, f);
        if f.filled_for.as_deref() == Some(key.as_str()) {
            return;
        }
        f.filled_for = Some(key);
        let region = value(f, "region");
        match id {
            "workflow.component" => {
                // The grass's patches as the case has them (what OK replaces), ticked.
                let ticked: std::collections::BTreeSet<String> =
                    if value(f, "component") == "grass" { grass_patches(case) } else { Default::default() };
                f.lists.insert("patches", ticked.clone());
                f.initial_lists.insert("patches", ticked);
            }
            _ => {
                // What the description says of the region, patch or material: shown when it
                // has it; a name it doesn't have (a new one, or one being typed) keeps what was
                // typed, and loses what another's filled in.
                let found = described(id, case, description.as_ref(), &region, f);
                for k in card_fields(id) {
                    match found.as_ref().and_then(|v| v.iter().find(|(x, _)| x == k)) {
                        Some((_, v)) => {
                            f.values.insert(k, v.clone());
                            f.extra.remove(*k);
                            f.initial.insert(k, v.clone());
                        }
                        None => {
                            if f.values.get(k) == f.initial.get(k) {
                                f.values.insert(k, String::new());
                            }
                            f.initial.insert(k, String::new());
                        }
                    }
                }
            }
        }
        // Parameters from *Edit as JSON* for what it now describes: laid over it.
        if let Some(v) = f.refill.take()
            && f.fill(spec, &v).is_err()
        {
            f.values = f.initial.clone();
        }
    }
}
