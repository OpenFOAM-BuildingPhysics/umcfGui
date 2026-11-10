//! `workflow.*`: the case's workflow description, `system/umcfWorkflow`.

use serde_json::{Value, json};
use umcf_model::RegionKind;
use umcf_model::description::{self, Description, PATH};
use umcf_model::description_edit::{EditReport, PatchEdit};
use umcf_model::settings::Settings;
use umcf_model::workflow::{MeshBuild, RegionStep, RootStep};

use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const SHOW: CommandSpec = CommandSpec {
    id: "workflow.show",
    label: "Workflow description",
    menu: "Case/Workflow description",
    shortcut: None,
    params: "scripts: true to also return the scripts workflow.scripts would write from it (scripts, version; scriptsError when none can be: no system/umcfWorkflow of the case's own, a description with problems, no version, a name unsafe in a script), without writing them. Returns the case's workflow description: system/umcfWorkflow as read (source \"file\", with the problems in it), or, when the case has none, as workflow.import would write it from the scripts and meshes (source \"scripts\", with what the import couldn't tell; \"unreadable\" when the file is there but isn't a dictionary, \"disk\" when it is on disk but not in the open case: written or undone since it was opened). Each region: kind (fluid, solid, vegetation), mesh (\"blocks\", or the steps that build it in constant/polyMesh), steps, patches with their role (inlet, outlet, side, top, ground, coupled, canopy, interior, bottom, sky) and partner, and a solid's materials (name: its cell zone; model: its buildingMaterialModel).",
    enabled: needs_case,
    run: show,
    journal: false,
    undoable: false,
    reads_only: true,
};

fn show(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(SHOW.id, p, &["scripts"])?;
    let with_scripts = p.bool("scripts")?.unwrap_or(false);
    let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let elsewhere = if case.description().is_none() { on_disk(case, fs) } else { OnDisk::None };
    let (d, source, problems, notes) = match case.description() {
        Some((d, problems)) => (d, "file", problems, Vec::new()),
        None => {
            let (d, notes) = Description::import(case);
            let source = match elsewhere {
                OnDisk::None => "scripts",
                OnDisk::Unreadable => "unreadable",
                OnDisk::NotOpen => "disk",
            };
            (d, source, Vec::new(), notes)
        }
    };
    let message = match source {
        "file" => format!("{PATH}: {}, {}", count(d.regions.len(), "region"), count(problems.len(), "problem")),
        "unreadable" => format!(
            "{PATH} is there but doesn't load (not a dictionary, a directory, or over the case's dictionary size limit): as the scripts and meshes have it, {}",
            count(d.regions.len(), "region")
        ),
        "disk" => format!(
            "{PATH} is on disk but not in the open case (written or undone since the case was opened; edit.redo or opening the case again brings it in): as the scripts and meshes have it, {}",
            count(d.regions.len(), "region")
        ),
        _ => format!(
            "no {PATH}: as the scripts and meshes have it, {}, {}",
            count(d.regions.len(), "region"),
            count(notes.len(), "note")
        ),
    };
    let mut data = json!({
        "file": PATH,
        "source": source,
        "description": to_json(&d),
        "problems": problems.iter().map(|p| json!({ "at": p.at, "message": p.message })).collect::<Vec<_>>(),
        "notes": notes,
    });
    if with_scripts && let Some(o) = data.as_object_mut() {
        // The scripts workflow.scripts would write from this description; nothing is written.
        match d.version.or(case.layout.version) {
            // Not a description of the case's own: workflow.scripts writes none until there is.
            _ if source != "file" => {
                o.insert(
                    "scriptsError".into(),
                    json!(match source {
                        "unreadable" => format!("{PATH} is there but doesn't load: workflow.scripts writes none until it is mended"),
                        "disk" => format!("{PATH} is on disk but not in the open case: workflow.scripts writes none until edit.redo or opening the case again brings it in"),
                        _ => format!("the case has no {PATH}: workflow.scripts writes none until workflow.import writes one"),
                    }),
                );
            }
            _ if !problems.is_empty() => {
                o.insert(
                    "scriptsError".into(),
                    json!("the description has problems: workflow.scripts writes none from it until they are mended"),
                );
            }
            None => {
                o.insert(
                    "scriptsError".into(),
                    json!("no OpenFOAM version: the description names none, and the case's can't be told"),
                );
            }
            Some(v) => match d.scripts(case, v) {
                Ok(written) => {
                    o.insert("version".into(), json!(v.to_string()));
                    let texts: Vec<Value> = written.iter().map(|(n, t)| json!({ "file": n, "text": t })).collect();
                    o.insert("scripts".into(), Value::Array(texts));
                }
                Err(e) => {
                    o.insert("scriptsError".into(), json!(format!("`{}` isn't safe in a script", e.0)));
                }
            },
        }
    }
    Ok(Outcome::new(message, data))
}

pub(super) const IMPORT: CommandSpec = CommandSpec {
    id: "workflow.import",
    label: "Write workflow description",
    menu: "Case/Write workflow description",
    shortcut: None,
    params: "replace: true to write over the case's system/umcfWorkflow (its comments and hand edits are not kept). Writes the workflow description from the case: its scripts (Allprepare, Allrun) read into the workflow, the region kinds, each patch's role and partner as its mesh and velocity condition tell, each solid's materials as its constant/<region>/transportProperties lists them (buildingMaterials), and the components the case's files switch on (a fluid's grassProperties, a limitTemperature entry in a fluid's fvOptions, fvConstraints or fvModels, in constant/ or system/, blending on in controlDict), and the settings its files have (deltaT, endTime, writeInterval, the fluids' RAS model, the temperature limit's bounds, blendingCoeffs: a macro, or fluids that differ, noted and left out), and the material library from the solids' materials (each model's first material whose coefficients are numbers or dictionaries of them; a model whose materials differ noted once, a material with a macro or directive among its coefficients noted and left out). What it couldn't tell (a patch without a role, a region the scripts don't mesh, a step that doesn't fit, a material or its model given by a macro, a material listed more than once) is listed in notes. case.save writes it. A system/umcfWorkflow that is a symbolic link is not written over.",
    enabled: needs_case,
    run: import,
    journal: true,
    undoable: true,
    reads_only: false,
};

fn import(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(IMPORT.id, p, &["replace"])?;
    let replace = p.bool("replace")?.unwrap_or(false);
    let o = s.opened_mut()?;
    let elsewhere = if o.case.description().is_none() { on_disk(&o.case, o.fs.as_ref()) } else { OnDisk::None };
    let had = o.case.description().is_some();
    let (d, notes) = Description::import(&o.case);
    if d.regions.is_empty() {
        return Err(Error::Failed(format!("{}: the case has no regions to describe", IMPORT.id)));
    }
    o.case.put_description(o.fs.as_ref(), &d.write(), replace).map_err(|e| match e {
        umcf_model::Error::Exists(_) => {
            Error::Failed(format!("{PATH} is there already: pass \"replace\": true to write over it"))
        }
        umcf_model::Error::IsDict(_) => Error::Failed(format!(
            "{PATH} is a link to another of the case's dictionaries: it isn't written over"
        )),
        umcf_model::Error::IsLink(_) => {
            Error::Failed(format!("{PATH} is a symbolic link (to a field, another file or nothing): it isn't written over"))
        }
        umcf_model::Error::NotLoaded { .. } => match elsewhere {
            OnDisk::NotOpen => Error::Failed(format!(
                "{PATH} is on disk but not in the open case (written or undone since the case was opened): edit.redo or open the case again first"
            )),
            _ => Error::Failed(format!(
                "{PATH} is there but doesn't load (not a dictionary, a directory, or over the case's dictionary size limit): mend or remove it first (it isn't written over)"
            )),
        },
        e => e.into(),
    })?;
    // Written over, the old file's comments and what the case doesn't tell are gone.
    let over = if had {
        " (over the one there: its comments and hand edits are gone; edit.undo brings them back)"
    } else {
        ""
    };
    let message = format!(
        "{PATH}: written from the case{over}, {}, {}",
        count(d.regions.len(), "region"),
        count(notes.len(), "note")
    );
    Ok(Outcome::new(message, json!({ "file": PATH, "description": to_json(&d), "notes": notes })))
}

pub(super) const REGION: CommandSpec = CommandSpec {
    id: "workflow.region",
    label: "Describe a region",
    menu: "Case/Workflow/Describe a region",
    shortcut: None,
    params: "region: its name; kind: fluid, solid or vegetation (adds the region to system/umcfWorkflow, or gives it that kind; a solid with materials stays one), or remove: true (takes it out; what still names it, coupled patches and the radiation, is listed). Edits the description only, entry by entry: its comments and layout stay. The case's own files follow once they are written from it. A name is one plain OpenFOAM word. workflow.import writes a description first when the case has none.",
    enabled: needs_case,
    run: region,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// `workflow.region`: adds a region, gives it a kind, or takes it out.
fn region(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(REGION.id, p, &["region", "kind", "remove"])?;
    let region = p.required_str("region")?;
    let kind = p.str("kind")?;
    let remove = p.bool("remove")?.unwrap_or(false);
    let o = s.opened_mut()?;
    let (report, message) = match (kind, remove) {
        (Some(_), true) | (None, false) => {
            return Err(Error::Params(format!("{}: give kind (fluid, solid, vegetation) or remove: true", REGION.id)));
        }
        (None, true) => {
            let r = o.case.undescribe_region(region)?;
            let also = if r.dangling.is_empty() {
                String::new()
            } else {
                format!("; still naming it: {}", r.dangling.join(", "))
            };
            (r, format!("{PATH}: region {region} taken out{also}"))
        }
        (Some(k), false) => {
            let kind = match k {
                "fluid" => RegionKind::Fluid,
                "solid" => RegionKind::Solid,
                "vegetation" => RegionKind::Vegetation,
                _ => return Err(Error::Params(format!("{}: kind is fluid, solid or vegetation", REGION.id))),
            };
            let r = o.case.describe_region(region, kind)?;
            let done = if r.added { "added" } else { "now" };
            (r, format!("{PATH}: region {region} {done}, {k}"))
        }
    };
    edited(s, message, &report)
}

pub(super) const PATCH: CommandSpec = CommandSpec {
    id: "workflow.patch",
    label: "Describe a patch",
    menu: "Case/Workflow/Describe a patch",
    shortcut: None,
    params: "region, patch: the region of system/umcfWorkflow and its patch (a patch the region doesn't list is added); role: inlet, outlet, side, top, ground, coupled, canopy, interior, bottom or sky (null takes it away); partner: {\"region\", \"patch\"} it is coupled to (null takes it away); name: a new name for the patch (the partners that named it follow). At least one of them. Edits the description only, entry by entry: its comments and layout stay. The case's own files (the mesh, fields, dictionaries) follow once they are written from it; patch.set renames a patch in the case.",
    enabled: needs_case,
    run: patch,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// `workflow.patch`: a patch's role, partner and name.
fn patch(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(PATCH.id, p, &["region", "patch", "role", "partner", "name"])?;
    let region = p.required_str("region")?;
    let patch = p.required_str("patch")?;
    let mut edit = PatchEdit::default();
    match p.value("role") {
        None => {}
        Some(Value::Null) => edit.role = Some(None),
        Some(Value::String(r)) => match description::Role::parse(r) {
            Some(role) => edit.role = Some(Some(role)),
            None => {
                let names: Vec<&str> = description::Role::ALL.iter().map(|r| r.name()).collect();
                return Err(Error::Params(format!("{}: role is one of {}, or null", PATCH.id, names.join(", "))));
            }
        },
        Some(_) => return Err(Error::Params(format!("{}: role is a name, or null", PATCH.id))),
    }
    match p.value("partner") {
        None => {}
        Some(Value::Null) => edit.partner = Some(None),
        Some(v) => {
            let field = |k: &str| v.get(k).and_then(Value::as_str);
            match (field("region"), field("patch"), v.as_object().map(serde_json::Map::len)) {
                (Some(r), Some(q), Some(2)) => edit.partner = Some(Some((r.to_string(), q.to_string()))),
                _ => {
                    return Err(Error::Params(format!(
                        "{}: partner is {{\"region\": …, \"patch\": …}}, or null",
                        PATCH.id
                    )));
                }
            }
        }
    }
    edit.rename = p.str("name")?.map(str::to_string);
    let o = s.opened_mut()?;
    let r = o.case.describe_patch(region, patch, &edit)?;
    let shown = edit.rename.as_deref().unwrap_or(patch);
    let mut message = format!("{PATH}: patch {shown} of region {region} {}", if r.added { "added" } else { "changed" });
    if let Some(to) = &edit.rename {
        message = format!("{PATH}: patch {patch} of region {region} is now {to}");
        if !r.followed.is_empty() {
            message.push_str(&format!("; partners that followed: {}", r.followed.join(", ")));
        }
    }
    edited(s, message, &r)
}

pub(super) const MATERIAL: CommandSpec = CommandSpec {
    id: "workflow.material",
    label: "Describe a material",
    menu: "Case/Workflow/Describe a material",
    shortcut: None,
    params: "region: a solid region of system/umcfWorkflow; material: its name, the cell zone built of it (added when the region doesn't list it); model: its buildingMaterialModel (null takes it away), or remove: true (takes the material out). Edits the description only, entry by entry: its comments and layout stay; the region's transportProperties follows once it is written from it.",
    enabled: needs_case,
    run: material,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// `workflow.material`: a solid's material and its model, or taking it out.
fn material(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(MATERIAL.id, p, &["region", "material", "model", "remove"])?;
    let region = p.required_str("region")?;
    let material = p.required_str("material")?;
    let remove = p.bool("remove")?.unwrap_or(false);
    let model = match p.value("model") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(m)) => Some(Some(m.as_str())),
        Some(_) => return Err(Error::Params(format!("{}: model is a name, or null", MATERIAL.id))),
    };
    if remove && model.is_some() {
        return Err(Error::Params(format!("{}: give model or remove: true, not both", MATERIAL.id)));
    }
    let o = s.opened_mut()?;
    let (r, message) = if remove {
        (
            o.case.undescribe_material(region, material)?,
            format!("{PATH}: material {material} of region {region} taken out"),
        )
    } else {
        let r = o.case.describe_material(region, material, model)?;
        let done = if r.added { "added" } else { "changed" };
        (r, format!("{PATH}: material {material} of region {region} {done}"))
    };
    edited(s, message, &r)
}

pub(super) const SETTINGS: CommandSpec = CommandSpec {
    id: "workflow.settings",
    label: "Choose settings",
    menu: "Case/Workflow/Choose settings",
    shortcut: None,
    params: "Any of: deltaT, endTime, writeInterval (numbers above 0; endTime may be 0); turbulence: the RAS model of the simulationType RAS fluids, one of OpenFOAM's for the case's version (kEpsilon, realizableKE, kOmegaSST, …) or the solver's (porousrealizableKE), or the one the case's fluids have already (a model of its own library); any name when the case's version isn't known; temperatureLimit: [min, max] in kelvin; blending: {\"dampingThickness\": 60, \"alphaCoeffU\": 0.3, \"alphaCoeffT\": 0.3}; materials: the material library, {\"HamstadConcrete\": {\"rho\": 1950, \"cap\": 850, \"lambda1\": 1.3, \"lambda2\": 0}, …} (each a material model of the solver for the version, its coefficients numbers or objects of numbers, at least those the solver reads of every material: rho, cap, lambda1, lambda2), what new materials of each model start from (case.new, region.add; the case's materials keep theirs). numerics: the solver numerics per region kind, {\"fluid\": {\"SIMPLE\": {\"nNonOrthogonalCorrectors\": 1}, \"relaxationFactors\": {\"equations\": {\"(h|e)\": 0.3}}}, \"solid\": …, \"vegetation\": …}: entries of system/<region>/fvSolution under solvers, SIMPLE, PIMPLE, PISO and relaxationFactors (three levels at most; numbers or plain words; field names or patterns as keys, brackets balanced; an entry given twice in the description's block refused), written into every region of that kind and new ones, only where each is an entry of the file (never over a dictionary nor through a value; never a field's dictionary the file's solvers hasn't, which is added by hand, nor a dictionary the file hasn't next to a directive or macro where it would go; a field's entry right under solvers is a dictionary; what isn't written is noted; values come back as strings, as written); read from a case as what all regions of a kind share, left out from a case of another OpenFOAM version; with from, those given go over its entry by entry. from: another case's directory (relative to the open case's), its settings read from its files, read only, those given here put over them; the models its solids have must be the solver's for the open case's version. The others stay as they are. Each goes into the case's files where the case has it (controlDict; each fluid's momentumTransport RAS model; the min and max of each fluid's limitTemperature entries, in their limitTemperatureCoeffs when they have one; controlDict's blendingCoeffs), nothing else in them changing, and into the settings block of system/umcfWorkflow when the case has one. A temperature limit or blending coefficients in a case without that component are kept in the description only, with a note. A setting given twice in the description's block is refused. writeInterval counts in the case's writeControl unit. case.save writes it all.",
    enabled: needs_case,
    run: settings,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// The settings given as command parameters (`deltaT`, …, `temperatureLimit`, `blending`),
/// checked as the description's `settings` block is.
pub(super) fn settings_params(id: &str, p: &Value) -> Result<Settings, Error> {
    let bad = |m: String| Error::Params(format!("{id}: {m}"));
    let Some(map) = p.as_object() else {
        return Err(bad("the settings are an object: {\"deltaT\": 3600, …}".into()));
    };
    let text_of = |v: &Value| -> Option<String> {
        match v {
            Value::Number(n) => Some(n.to_string()),
            Value::String(s) if !s.contains(['{', '}', '(', ')', ';', '"', '/', '$', '#']) && !s.trim().is_empty() => {
                Some(s.trim().to_string())
            }
            _ => None,
        }
    };
    let mut body = String::new();
    for (k, v) in map {
        if k == "from" {
            return Err(bad("from is workflow.settings' own: another case's settings aren't taken here".into()));
        }
        let value = match (k.as_str(), v) {
            ("numerics", Value::Object(kinds)) => {
                // { "fluid": { "relaxationFactors": { "fields": { "p_rgh": 0.3 } } } }: nested
                // objects down to numbers or plain words; keys may be patterns ("(h|e)").
                fn nest(
                    o: &serde_json::Map<String, Value>,
                    at: &str,
                    depth: usize,
                    text_of: &dyn Fn(&Value) -> Option<String>,
                ) -> Result<String, String> {
                    if depth > 5 {
                        return Err(format!("{at} is nested too deep"));
                    }
                    let mut b = String::from("{");
                    for (k, v) in o {
                        let plain = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.');
                        if !umcf_model::settings::numerics_key(k) {
                            return Err(format!("{at}/{k} is not a field's name or pattern"));
                        }
                        let key = if k.bytes().all(plain) { k.clone() } else { format!("\"{k}\"") };
                        match v {
                            Value::Object(inner) => b.push_str(&format!(
                                " {key} {}",
                                nest(inner, &format!("{at}/{k}"), depth + 1, text_of)?
                            )),
                            v => {
                                let t = text_of(v).ok_or_else(|| format!("{at}/{k} is a number or a plain word"))?;
                                b.push_str(&format!(" {key} {t};"));
                            }
                        }
                    }
                    b.push_str(" }");
                    Ok(b)
                }
                nest(kinds, "numerics", 0, &text_of).map_err(bad)?
            }
            ("materials", Value::Object(models)) => {
                // { "Soil": { "rho": 1150, "VanGenuchtenCoeffs": { "n": 2 } } }
                let word = |w: &str| !w.is_empty() && w.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
                let mut b = String::from("{");
                for (model, coeffs) in models {
                    let Value::Object(coeffs) = coeffs else {
                        return Err(bad(format!("materials/{model} is an object of coefficients")));
                    };
                    if !word(model) {
                        return Err(bad(format!("materials/{model} is not a material model's name")));
                    }
                    b.push_str(&format!(" {model} {{"));
                    for (ck, cv) in coeffs {
                        if !word(ck) {
                            return Err(bad(format!("materials/{model}/{ck} is not a coefficient's name")));
                        }
                        match cv {
                            Value::Object(inner) => {
                                b.push_str(&format!(" {ck} {{"));
                                for (ik, iv) in inner {
                                    let t = text_of(iv).filter(|_| word(ik));
                                    let t = t.ok_or_else(|| bad(format!("materials/{model}/{ck}/{ik} is a number")))?;
                                    b.push_str(&format!(" {ik} {t};"));
                                }
                                b.push_str(" }");
                            }
                            v => {
                                let t = text_of(v).ok_or_else(|| bad(format!("materials/{model}/{ck} is a number")))?;
                                b.push_str(&format!(" {ck} {t};"));
                            }
                        }
                    }
                    b.push_str(" }");
                }
                b.push_str(" }");
                b
            }
            ("temperatureLimit", Value::Array(a)) => {
                let t: Option<Vec<String>> = a.iter().map(text_of).collect();
                format!("( {} )", t.ok_or_else(|| bad("temperatureLimit is [min, max]".into()))?.join(" "))
            }
            ("blending", Value::Object(o)) => {
                let mut b = String::from("{");
                for (ck, cv) in o {
                    let t = text_of(cv).filter(|_| ck.bytes().all(|c| c.is_ascii_alphanumeric()));
                    let t = t.ok_or_else(|| bad(format!("blending/{ck} is a number")))?;
                    b.push_str(&format!(" {ck} {t};"));
                }
                b.push_str(" }");
                b
            }
            (_, v) => text_of(v).ok_or_else(|| bad(format!("{k} is a number or a name")))?,
        };
        if !k.bytes().all(|c| c.is_ascii_alphanumeric()) {
            return Err(bad(format!("{k} is not a setting")));
        }
        let end = if value.starts_with('{') { "" } else { ";" };
        body.push_str(&format!("{k} {value}{end}\n"));
    }
    let doc = umcf_foam_dict::Document::parse(&body).map_err(|e| bad(e.to_string()))?;
    let mut problems = Vec::new();
    let s = Settings::read(&doc.root, &mut |at: &str, m: String| {
        problems.push(format!("{} {m}", at.trim_start_matches("settings/")))
    });
    if !problems.is_empty() {
        return Err(bad(problems.join("; ")));
    }
    Ok(s)
}

/// `workflow.settings`: settings chosen, into the files and the description.
fn settings(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    // `from` is read here, the rest as any settings are.
    let mut given = p.clone();
    if let Some(o) = given.as_object_mut() {
        o.remove("from");
    }
    let mut chosen = settings_params(SETTINGS.id, &given)?;
    // Another case's settings, read only: those given here come first.
    let mut from_notes = Vec::new();
    if let Some(from) = p.get("from") {
        let Some(dir) = from.as_str().filter(|d| !d.trim().is_empty()) else {
            return Err(Error::Params(format!("{}: from is the path of a case", SETTINGS.id)));
        };
        // A relative path is the open case's neighbour: `../other` from the case's directory.
        let dir = match s.location() {
            Some(here) if std::path::Path::new(dir).is_relative() => {
                std::path::Path::new(here).join(dir).to_string_lossy().into_owned()
            }
            _ => dir.to_string(),
        };
        let dir = dir.as_str();
        let fs = umcf_core::DirFs::new(dir);
        let options = umcf_model::OpenOptions { field_bytes: 0, ..umcf_model::OpenOptions::default() };
        let other = umcf_model::Case::open_with(&fs, options)
            .map_err(|e| Error::Failed(format!("{}: from {dir}: {e}", SETTINGS.id)))?;
        let (mut theirs, mut notes) = Settings::from_case(&other);
        // Numerics are what differs from the configuration of the other case's version: the
        // paths of one version's fvSolution needn't be the other's.
        let here = s.opened_mut()?.case.layout.version;
        if !theirs.numerics.is_empty() && other.layout.version != here {
            notes.push(format!(
                "its numerics are left out: its OpenFOAM version isn't the open case's ({})",
                here.map_or("not known", |v| v.label())
            ));
            theirs.numerics.clear();
        }
        if theirs.is_empty() {
            return Err(Error::Failed(format!(
                "{}: from {dir}: the case has none of the settings to take{}",
                SETTINGS.id,
                notes.iter().map(|n| format!("; {n}")).collect::<String>()
            )));
        }
        from_notes = notes.into_iter().map(|n| format!("{dir}: {n}")).collect();
        chosen = merged(theirs, chosen);
    }
    if chosen.is_empty() {
        return Err(Error::Params(format!(
            "{}: give a setting (deltaT, endTime, writeInterval, turbulence, temperatureLimit, blending, materials, numerics) or from (another case)",
            SETTINGS.id
        )));
    }
    let o = s.opened_mut()?;
    let c = o.case.choose_settings(&chosen)?;
    let mut names: Vec<&str> = chosen.entries().iter().map(|(k, _)| *k).collect();
    if !chosen.materials.is_empty() {
        names.push("materials");
    }
    if !chosen.numerics.is_empty() {
        names.push("numerics");
    }
    let mut message = format!("settings chosen: {}", names.join(", "));
    if !c.applied.changed.is_empty() {
        message.push_str(&format!("; written into {}", c.applied.changed.join(", ")));
    }
    if c.described {
        message.push_str(&format!("; in {PATH}"));
    }
    let notes: Vec<String> = from_notes.into_iter().chain(c.applied.notes.iter().cloned()).collect();
    for n in &notes {
        message.push_str(&format!("; {n}"));
    }
    Ok(Outcome::new(
        message,
        json!({ "settings": settings_json(&chosen), "changed": c.applied.changed, "described": c.described, "notes": notes }),
    ))
}

/// `base` with what `over` chooses put over it (a material model of `over` replaces `base`'s;
/// numerics entry by entry).
fn merged(mut base: Settings, over: Settings) -> Settings {
    macro_rules! take {
        ($($f:ident),*) => { $( if over.$f.is_some() { base.$f = over.$f; } )* };
    }
    take!(delta_t, end_time, write_interval, turbulence, temperature_limit, blending);
    for (model, coeffs) in over.materials {
        match base.materials.iter_mut().find(|(m, _)| *m == model) {
            Some(slot) => slot.1 = coeffs,
            None => base.materials.push((model, coeffs)),
        }
    }
    umcf_model::settings::merge_numerics(&mut base.numerics, &over.numerics);
    base
}

/// The settings as JSON, as `workflow.settings` takes them.
pub fn settings_json(s: &Settings) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in [
        ("deltaT", &s.delta_t),
        ("endTime", &s.end_time),
        ("writeInterval", &s.write_interval),
        ("turbulence", &s.turbulence),
    ] {
        if let Some(v) = v {
            m.insert(k.into(), json!(v));
        }
    }
    if let Some((a, b)) = &s.temperature_limit {
        m.insert("temperatureLimit".into(), json!([a, b]));
    }
    if let Some(c) = &s.blending {
        let o: serde_json::Map<String, Value> =
            umcf_model::settings::BLENDING_KEYS.iter().zip(c).map(|(k, v)| ((*k).to_string(), json!(v))).collect();
        m.insert("blending".into(), Value::Object(o));
    }
    if !s.materials.is_empty() {
        let mut models = serde_json::Map::new();
        for (model, coeffs) in &s.materials {
            let mut c = serde_json::Map::new();
            for (k, v) in coeffs {
                let value = match umcf_foam_dict::Document::parse(&format!(
                    "x {v}{}",
                    if v.starts_with('{') { "" } else { ";" }
                )) {
                    Ok(d) if v.starts_with('{') => Value::Object(
                        d.get(&["x"])
                            .and_then(|e| e.dict())
                            .map(|b| {
                                b.entries()
                                    .filter_map(|e| Some((e.key().to_string(), json!(e.value_text()?))))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ),
                    _ => json!(v),
                };
                c.insert(k.clone(), value);
            }
            models.insert(model.clone(), Value::Object(c));
        }
        m.insert("materials".into(), Value::Object(models));
    }
    if !s.numerics.is_empty() {
        let mut kinds = serde_json::Map::new();
        for (kind, leaves) in &s.numerics {
            fn put(node: &mut serde_json::Map<String, Value>, at: &[String], v: &str) {
                match at {
                    [] => {}
                    [last] => {
                        node.insert(last.clone(), json!(v));
                    }
                    [first, rest @ ..] => {
                        let next = node.entry(first.clone()).or_insert_with(|| Value::Object(serde_json::Map::new()));
                        if let Value::Object(o) = next {
                            put(o, rest, v);
                        }
                    }
                }
            }
            let mut root = serde_json::Map::new();
            for (at, v) in leaves {
                put(&mut root, at, v);
            }
            kinds.insert(kind.clone(), Value::Object(root));
        }
        m.insert("numerics".into(), Value::Object(kinds));
    }
    Value::Object(m)
}

pub(super) const SCRIPTS: CommandSpec = CommandSpec {
    id: "workflow.scripts",
    label: "Write the scripts",
    menu: "Case/Workflow/Write the scripts",
    shortcut: None,
    params: "replace: true to write over scripts that are there and differ (case.save keeps a backup of each; a script that is a symbolic link isn't written over). Writes Allprepare, Allrun and Allclean from system/umcfWorkflow, for its OpenFOAM version (the case's when it names none): every region's mesh and steps, the radiation pre-processing and the solver, as the description has them; the dictionaries Allrun copies into processor*/ are the case's. A description with problems is refused (workflow.show lists them), and so is a name that isn't safe in a script. case.save writes them, executable; the case check reads them as written. workflow.show with scripts: true returns them without writing.",
    enabled: needs_case,
    run: scripts,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// `workflow.scripts`: the scripts written from the description.
fn scripts(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(SCRIPTS.id, p, &["replace"])?;
    let replace = p.bool("replace")?.unwrap_or(false);
    let o = s.opened_mut()?;
    let Some((d, problems)) = o.case.description() else {
        return Err(Error::Failed(format!(
            "the case has no {PATH} (or it doesn't load): workflow.import writes one from the case first"
        )));
    };
    if let Some(first) = problems.first() {
        return Err(Error::Failed(format!(
            "{PATH} has {} ({}: {}): mend it first, as the scripts would leave out what can't be read (workflow.show lists them)",
            count(problems.len(), "problem"),
            first.at,
            first.message
        )));
    }
    let Some(version) = d.version.or(o.case.layout.version) else {
        return Err(Error::Failed(format!(
            "{PATH} names no OpenFOAM version, and the case's can't be told: set version 8 or 12"
        )));
    };
    let written = d.scripts(&o.case, version).map_err(|e| {
        Error::Failed(format!("{}: `{}` isn't safe in a script: no script is written", SCRIPTS.id, e.0))
    })?;
    let texts: Vec<Value> = written.iter().map(|(n, t)| json!({ "file": n, "text": t })).collect();
    // What is there now, put by a command or on disk.
    let mut changed = Vec::new();
    let mut differ = Vec::new();
    for (name, text) in &written {
        match o.case.file(o.fs.as_ref(), name)? {
            Some(old) if old.as_slice() == text.as_bytes() => {}
            Some(_) => {
                differ.push(*name);
                changed.push(*name);
            }
            None => changed.push(*name),
        }
    }
    if !differ.is_empty() && !replace {
        return Err(Error::Failed(format!(
            "{} {} there and {}: pass \"replace\": true to write over {} (case.save keeps a backup of each)",
            differ.join(", "),
            if differ.len() == 1 { "is" } else { "are" },
            if differ.len() == 1 { "differs" } else { "differ" },
            if differ.len() == 1 { "it" } else { "them" }
        )));
    }
    for (name, text) in &written {
        if changed.contains(name) {
            o.case.put_script(o.fs.as_ref(), name, text.clone().into_bytes()).map_err(|e| match e {
                umcf_model::Error::IsLink(_) => Error::Failed(format!(
                    "{name} is a symbolic link (to another file or nothing): it isn't written over"
                )),
                e => e.into(),
            })?;
        }
    }
    let message = if changed.is_empty() {
        format!("the scripts are as {PATH} has them already, for {version}: nothing to write")
    } else {
        format!("{}: written from {PATH}, for {version}", changed.join(", "))
    };
    Ok(Outcome::new(message, json!({ "version": version.to_string(), "changed": changed, "scripts": texts })))
}

/// The outcome of an edit: the message, and the description as it is now.
fn edited(s: &Session, message: String, r: &EditReport) -> Result<Outcome, Error> {
    let (d, problems) = s.case().and_then(umcf_model::Case::description).unwrap_or_default();
    // What the file has the reader can't read, the edit's or not: shown with it.
    let problems: Vec<Value> = problems.iter().map(|p| json!({ "at": p.at, "message": p.message })).collect();
    let message = if problems.is_empty() {
        message
    } else {
        format!("{message} ({} in the file)", count(problems.len(), "problem"))
    };
    Ok(Outcome::new(
        message,
        json!({ "file": PATH, "description": to_json(&d), "problems": problems, "followed": r.followed, "dangling": r.dangling }),
    ))
}

/// What is on disk at the description's path when the open case holds none.
enum OnDisk {
    None,
    /// Not a dictionary.
    Unreadable,
    /// A dictionary the open case doesn't hold (taken back by an undo after a save).
    NotOpen,
}

fn on_disk(case: &umcf_model::Case, fs: &dyn umcf_core::CaseFs) -> OnDisk {
    // A directory there isn't a dictionary either (a read of it finds no file).
    if fs.is_dir(PATH) {
        return OnDisk::Unreadable;
    }
    // Read as the open loaded its dictionaries: one too large for that limit didn't load either.
    match fs.read(PATH, case.options().dict_bytes) {
        Ok(Some(bytes)) => match umcf_foam_dict::Document::parse_bytes(&bytes) {
            Ok(_) => OnDisk::NotOpen,
            Err(_) => OnDisk::Unreadable,
        },
        Ok(None) => OnDisk::None,
        Err(_) => OnDisk::Unreadable,
    }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// The description as JSON, as `workflow.show` returns it.
pub(crate) fn to_json(d: &Description) -> Value {
    let regions: Vec<Value> = d
        .regions
        .iter()
        .map(|r| {
            let kind = match r.kind {
                RegionKind::Solid => "solid",
                RegionKind::Vegetation => "vegetation",
                RegionKind::Fluid | RegionKind::Single => "fluid",
            };
            let mesh = match &r.mesh {
                None => Value::Null,
                Some(MeshBuild::Block) => json!("blocks"),
                Some(MeshBuild::Root(steps)) => steps
                    .iter()
                    .map(|st| match st {
                        RootStep::BlockMesh(d) => json!({ "step": "blockMesh", "dict": d }),
                        RootStep::CreatePatch => json!({ "step": "createPatch" }),
                        RootStep::SnappyHexMesh(d) => json!({ "step": "snappyHexMesh", "dict": d }),
                    })
                    .collect(),
            };
            let steps: Vec<Value> = r
                .steps
                .iter()
                .map(|st| match st {
                    RegionStep::CreatePatch => json!({ "step": "createPatch" }),
                    RegionStep::ChangeDictionary => json!({ "step": "changeDictionary" }),
                    RegionStep::Sets { setset_batch: None } => json!({ "step": "topoSet" }),
                    RegionStep::Sets { setset_batch: Some(b) } => json!({ "step": "setSet", "batch": b }),
                    RegionStep::SetFields { restore } => json!({
                        "step": "setFields",
                        "restore": restore.iter().map(|(b, f)| json!({ "backup": b, "field": f })).collect::<Vec<_>>(),
                    }),
                })
                .collect();
            let patches: Vec<Value> = r
                .patches
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name,
                        "role": p.role.map(description::Role::name),
                        "partner": p.partner.as_ref().map(|(r, q)| json!({ "region": r, "patch": q })),
                    })
                })
                .collect();
            let materials: Vec<Value> =
                r.materials.iter().map(|m| json!({ "name": m.name, "model": m.model })).collect();
            json!({ "name": r.name, "kind": kind, "mesh": mesh, "steps": steps, "patches": patches, "materials": materials })
        })
        .collect();
    json!({
        "version": d.version.map(|v| v.label()),
        "components": d.components.words().into_iter().filter(|(_, on)| *on).map(|(w, _)| w).collect::<Vec<_>>(),
        "settings": settings_json(&d.settings),
        "regions": regions,
        "features": d.features,
        "radiation": d.radiation.as_ref().map(|r| json!({
            "region": r.region,
            "lai": r.lai_region,
            "viewFactors": r.view_factors,
            "solarRayTracing": r.solar_ray_tracing,
        })),
        "solver": d.solver,
        "reconstruct": d.reconstruct,
    })
}
