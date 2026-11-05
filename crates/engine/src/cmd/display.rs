//! How the scene's layers are drawn: the `display` parameter of `scene.snapshot`, `select.pick`
//! and `select.rect`, a list of entries each giving a region's mesh, one of its patches or patch
//! groups, a surface or one of its regions a mode (surface, wireframe, points, hidden) and an
//! opacity. Later entries win over earlier ones, field by field.

use std::collections::HashMap;

use serde_json::{Map, Value, json};
use umcf_geom::stl;
use umcf_render::raster::{Display, Mode};
use umcf_render::{Scene, Source};

use super::geometry::{limits, surface_path};
use crate::{Error, Session};

/// What an entry applies to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Layer {
    /// A region's mesh (`""` in a single-region case).
    Region(String),
    Patch {
        region: String,
        patch: String,
    },
    /// The patches of a region in a patch group (`inGroups`).
    Group {
        region: String,
        group: String,
    },
    /// An STL surface (case-relative).
    Surface(String),
    /// A region of a surface, as OpenFOAM names it.
    Solid {
        file: String,
        solid: String,
    },
}

/// One entry: a layer and what it sets (`None`: as before).
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayEntry {
    pub layer: Layer,
    pub mode: Option<Mode>,
    pub opacity: Option<f32>,
}

/// The most entries a `display` takes.
const MAX_ENTRIES: usize = 1000;

impl DisplayEntry {
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        match &self.layer {
            Layer::Region(r) => {
                o.insert("region".into(), json!(r));
            }
            Layer::Patch { region, patch } => {
                o.insert("region".into(), json!(region));
                o.insert("patch".into(), json!(patch));
            }
            Layer::Group { region, group } => {
                o.insert("region".into(), json!(region));
                o.insert("group".into(), json!(group));
            }
            Layer::Surface(f) => {
                o.insert("surface".into(), json!(f));
            }
            Layer::Solid { file, solid } => {
                o.insert("surface".into(), json!(file));
                o.insert("solid".into(), json!(solid));
            }
        }
        if let Some(m) = self.mode {
            o.insert("mode".into(), json!(m.name()));
        }
        if let Some(x) = self.opacity {
            o.insert("opacity".into(), json!(x));
        }
        Value::Object(o)
    }

    /// Whether this entry applies to class `class` of `object`.
    fn applies(&self, object: &umcf_render::Object, class: usize) -> bool {
        let name = object.classes.get(class).map(String::as_str);
        match (&self.layer, &object.source) {
            (Layer::Region(r), Source::Mesh { region }) => r == region,
            (Layer::Patch { region, patch }, Source::Mesh { region: r }) => region == r && name == Some(patch),
            (Layer::Group { region, group }, Source::Mesh { region: r }) => {
                region == r && object.groups.get(class).is_some_and(|g| g.contains(group))
            }
            (Layer::Surface(f), Source::Surface { file }) => f == file,
            (Layer::Solid { file, solid }, Source::Surface { file: f }) => file == f && name == Some(solid),
            _ => false,
        }
    }
}

/// The entries of a `display` parameter, as written; the commands check that what they name is
/// in the case.
pub fn parse(id: &str, v: &Value) -> Result<Vec<DisplayEntry>, Error> {
    let list = v.as_array().ok_or_else(|| Error::Params(format!("{id}: `display` is a list")))?;
    if list.len() > MAX_ENTRIES {
        return Err(Error::Params(format!("{id}: more than {MAX_ENTRIES} `display` entries")));
    }
    let mut out = Vec::with_capacity(list.len());
    for e in list {
        let bad = || {
            Error::Params(format!(
                "{id}: a `display` entry is {{\"region\"[, \"patch\" or \"group\"]}} or {{\"surface\"[, \"solid\"]}} with \"mode\" (surface, wireframe, points, hidden) and/or \"opacity\" (0 to 1), not {e}"
            ))
        };
        let o = e.as_object().ok_or_else(bad)?;
        const KEYS: [&str; 7] = ["region", "patch", "group", "surface", "solid", "mode", "opacity"];
        if o.keys().any(|k| !KEYS.contains(&k.as_str())) {
            return Err(bad());
        }
        let text = |k: &str| match o.get(k) {
            None => Ok(None),
            Some(v) => v.as_str().map(|s| Some(s.to_string())).ok_or_else(bad),
        };
        let layer = match (text("region")?, text("patch")?, text("group")?, text("surface")?, text("solid")?) {
            (Some(r), None, None, None, None) => Layer::Region(r),
            (Some(region), Some(patch), None, None, None) => Layer::Patch { region, patch },
            (Some(region), None, Some(group), None, None) => Layer::Group { region, group },
            (None, None, None, Some(f), None) => Layer::Surface(surface_path(id, &f, false)?),
            (None, None, None, Some(f), Some(solid)) => Layer::Solid { file: surface_path(id, &f, false)?, solid },
            _ => return Err(bad()),
        };
        let mode = match text("mode")? {
            None => None,
            Some(m) => Some(Mode::parse(&m).ok_or_else(bad)?),
        };
        let opacity = match o.get("opacity") {
            None => None,
            Some(x) => Some(x.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(bad)? as f32),
        };
        if mode.is_none() && opacity.is_none() {
            return Err(bad());
        }
        out.push(DisplayEntry { layer, mode, opacity });
    }
    Ok(out)
}

/// Whether what `entries` name is in the open case: each region with a mesh, patch, patch group,
/// surface and surface region. A surface `scene` shows is checked against it; another is read.
pub(super) fn check(s: &Session, id: &str, entries: &[DisplayEntry], scene: &Scene) -> Result<(), Error> {
    let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let patches = |region: &str| {
        let r = case
            .regions
            .iter()
            .find(|r| r.name == region)
            .ok_or_else(|| Error::Params(format!("{id}: no region `{region}`")))?;
        r.patches.as_ref().ok_or_else(|| Error::Params(format!("{id}: region `{region}` has no mesh")))
    };
    let mut solids: HashMap<String, Vec<String>> = HashMap::new();
    for e in entries {
        match &e.layer {
            Layer::Region(r) => {
                patches(r)?;
            }
            Layer::Patch { region, patch } => {
                if !patches(region)?.iter().any(|p| p.name == *patch) {
                    return Err(Error::Params(format!("{id}: region `{region}` has no patch `{patch}`")));
                }
            }
            Layer::Group { region, group } => {
                if !patches(region)?.iter().any(|p| p.groups.contains(group)) {
                    return Err(Error::Params(format!("{id}: region `{region}` has no patch in group `{group}`")));
                }
            }
            Layer::Surface(f) | Layer::Solid { file: f, .. } => {
                let shown = scene.objects.iter().find(|o| matches!(&o.source, Source::Surface { file } if file == f));
                if let Some(o) = shown
                    && !solids.contains_key(f)
                {
                    solids.insert(f.clone(), o.classes.clone());
                }
                if !solids.contains_key(f) {
                    let bytes =
                        case.file(fs, f)?.ok_or_else(|| Error::Params(format!("{id}: there is no surface {f}")))?;
                    let stl = stl::read(&bytes, &limits()).map_err(|e| Error::Failed(format!("{id}: {f}: {e}")))?;
                    solids.insert(f.clone(), stl.regions().0);
                }
                if let Layer::Solid { file, solid } = &e.layer
                    && !solids.get(file).is_some_and(|names| names.contains(solid))
                {
                    return Err(Error::Params(format!("{id}: {file} has no region `{solid}`")));
                }
            }
        }
    }
    Ok(())
}

/// How each class of `scene` is drawn after `entries`, in order, for the renderer's
/// [`umcf_render::raster::Style::display`]: only the classes they change are in it.
pub fn style(scene: &Scene, entries: &[DisplayEntry]) -> HashMap<(usize, u32), Display> {
    let mut out: HashMap<(usize, u32), Display> = HashMap::new();
    if entries.is_empty() {
        return out;
    }
    for (k, o) in scene.objects.iter().enumerate() {
        for c in 0..o.classes.len() {
            let Ok(class) = u32::try_from(c) else { break };
            let mut d: Option<Display> = None;
            for e in entries.iter().filter(|e| e.applies(o, c)) {
                let x = d.get_or_insert_with(Display::default);
                if let Some(m) = e.mode {
                    x.mode = m;
                }
                if let Some(a) = e.opacity {
                    x.opacity = a;
                }
            }
            if let Some(d) = d {
                out.insert((k, class), d);
            }
        }
    }
    out
}
