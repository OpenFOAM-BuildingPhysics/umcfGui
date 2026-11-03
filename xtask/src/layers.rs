//! `cargo xtask layers`: the crate dependency layering (docs/architecture.md, AGENTS.md §4).
//!
//! The rule engine works on a small model ([`Crate`], [`Dep`]) so it can be unit-tested;
//! [`from_metadata`] builds that model from `cargo metadata`.

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Layer(u8),
    /// Build tooling: not checked.
    Exempt,
}

/// The layer table. Names are package names without the `umcf-` prefix. A crate that is not
/// listed here is a violation: register it before adding it.
pub const TABLE: &[(&str, Class)] = &[
    ("core", Class::Layer(0)),
    ("geom", Class::Layer(0)),
    ("foam-dict", Class::Layer(1)),
    ("foam-mesh", Class::Layer(1)),
    ("foam-field", Class::Layer(1)),
    ("schema", Class::Layer(2)),
    ("model", Class::Layer(2)),
    ("validate", Class::Layer(3)),
    ("post", Class::Layer(3)),
    ("render", Class::Layer(3)),
    ("runner", Class::Layer(4)),
    ("engine", Class::Layer(5)),
    ("ui-egui", Class::Layer(6)),
    ("mcp", Class::Layer(6)),
    ("gui", Class::Layer(7)),
    ("cli", Class::Layer(7)),
    ("xtask", Class::Exempt),
];

/// Orderings within a layer: in each chain an earlier crate may be used by a later one.
pub const INTRA_LAYER_ORDER: &[&[&str]] =
    &[&["core", "geom"], &["foam-dict", "foam-mesh", "foam-field"], &["schema", "model"], &["post", "render"]];

/// Format crates may depend only on these workspace crates, so OpenFOAM syntax never pulls in
/// UMCF knowledge.
pub const FORMAT_ALLOWED: &[(&str, &[&str])] = &[
    ("foam-dict", &["core", "geom"]),
    ("foam-mesh", &["core", "geom", "foam-dict"]),
    ("foam-field", &["core", "geom", "foam-dict", "foam-mesh"]),
];

/// External crates that make up a UI toolkit. A trailing `*` is a prefix match.
pub const UI_CRATES: &[&str] = &["egui*", "eframe", "winit", "rfd", "wgpu*"];

/// First layer allowed to use UI crates.
pub const UI_MIN_LAYER: u8 = 6;

/// Crates that must build for wasm32 (L0–L3; `runner` at L4 spawns processes).
pub const WASM_MAX_LAYER: u8 = 3;

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("umcf-").unwrap_or(pkg)
}

pub fn classify(pkg: &str) -> Option<Class> {
    let s = short_name(pkg);
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

fn intra_layer_allowed(from: &str, to: &str) -> bool {
    let (from, to) = (short_name(from), short_name(to));
    INTRA_LAYER_ORDER.iter().any(|chain| {
        match (chain.iter().position(|n| *n == from), chain.iter().position(|n| *n == to)) {
            (Some(f), Some(t)) => t < f,
            _ => false,
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    /// The dependency is a workspace member.
    pub workspace: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered { krate: String },
    Upward { krate: String, layer: u8, dep: String, dep_layer: u8 },
    SameLayer { krate: String, dep: String, layer: u8 },
    UiBelowL6 { krate: String, dep: String, layer: u8 },
    FormatDep { krate: String, dep: String },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(f, "{krate} is not in the layer table (xtask/src/layers.rs TABLE)")
            }
            Violation::Upward { krate, layer, dep, dep_layer } => {
                write!(f, "{krate} (L{layer}) depends on {dep} (L{dep_layer}), a higher layer")
            }
            Violation::SameLayer { krate, dep, layer } => {
                write!(f, "{krate} depends on {dep}, both L{layer}, and no INTRA_LAYER_ORDER chain allows it")
            }
            Violation::UiBelowL6 { krate, dep, layer } => {
                write!(
                    f,
                    "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L{UI_MIN_LAYER}+"
                )
            }
            Violation::FormatDep { krate, dep } => {
                write!(f, "format crate {krate} depends on {dep}; FORMAT_ALLOWED does not list it")
            }
        }
    }
}

/// Every rule, applied to every crate.
pub fn check(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        let layer = match classify(&c.name) {
            None => {
                out.push(Violation::Unregistered { krate: c.name.clone() });
                continue;
            }
            Some(Class::Exempt) => continue,
            Some(Class::Layer(l)) => l,
        };
        // Apps sit on top of everything.
        if layer == 7 {
            continue;
        }
        for d in c.deps.iter().filter(|d| d.kind != DepKind::Dev) {
            if d.workspace {
                let Some(Class::Layer(dl)) = classify(&d.name) else { continue };
                if dl > layer {
                    out.push(Violation::Upward { krate: c.name.clone(), layer, dep: d.name.clone(), dep_layer: dl });
                } else if dl == layer && !intra_layer_allowed(&c.name, &d.name) {
                    out.push(Violation::SameLayer { krate: c.name.clone(), dep: d.name.clone(), layer });
                }
                if let Some((_, allowed)) = FORMAT_ALLOWED.iter().find(|(n, _)| *n == short_name(&c.name))
                    && !allowed.contains(&short_name(&d.name))
                {
                    out.push(Violation::FormatDep { krate: c.name.clone(), dep: d.name.clone() });
                }
            } else if layer < UI_MIN_LAYER && is_ui_crate(&d.name) {
                out.push(Violation::UiBelowL6 { krate: c.name.clone(), dep: d.name.clone(), layer });
            }
        }
    }
    out
}

/// Builds the model from `cargo metadata --format-version 1 --no-deps` output.
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let packages = meta.get("packages").and_then(Value::as_array).ok_or("cargo metadata: no `packages` array")?;
    let names: Vec<&str> = packages.iter().filter_map(|p| p.get("name").and_then(Value::as_str)).collect();
    let mut out = Vec::new();
    for p in packages {
        let name = p.get("name").and_then(Value::as_str).ok_or("cargo metadata: package without a name")?;
        let mut deps = Vec::new();
        for d in p.get("dependencies").and_then(Value::as_array).into_iter().flatten() {
            let Some(dn) = d.get("name").and_then(Value::as_str) else { continue };
            let kind = match d.get("kind").and_then(Value::as_str) {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            deps.push(Dep { name: dn.to_string(), kind, workspace: names.contains(&dn) });
        }
        out.push(Crate { name: name.to_string(), deps });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn krate(name: &str, deps: &[(&str, bool)]) -> Crate {
        Crate {
            name: name.into(),
            deps: deps.iter().map(|(n, ws)| Dep { name: (*n).into(), kind: DepKind::Normal, workspace: *ws }).collect(),
        }
    }

    #[test]
    fn downward_deps_pass() {
        let v =
            check(&[krate("umcf-model", &[("umcf-foam-dict", true), ("umcf-schema", true), ("serde_json", false)])]);
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn upward_dep_fails() {
        let v = check(&[krate("umcf-foam-dict", &[("umcf-engine", true)])]);
        assert!(v.iter().any(|x| matches!(x, Violation::Upward { dep_layer: 5, .. })), "{v:?}");
    }

    #[test]
    fn same_layer_needs_a_chain() {
        assert!(check(&[krate("umcf-foam-field", &[("umcf-foam-dict", true)])]).is_empty());
        let v = check(&[krate("umcf-foam-dict", &[("umcf-foam-field", true)])]);
        assert!(v.iter().any(|x| matches!(x, Violation::SameLayer { .. })), "{v:?}");
        let v = check(&[krate("umcf-validate", &[("umcf-post", true)])]);
        assert!(matches!(v[..], [Violation::SameLayer { .. }]), "{v:?}");
    }

    #[test]
    fn ui_crates_only_from_l6() {
        for dep in ["egui", "egui_extras", "eframe", "winit", "rfd", "wgpu"] {
            let v = check(&[krate("umcf-engine", &[(dep, false)])]);
            assert!(matches!(v[..], [Violation::UiBelowL6 { layer: 5, .. }]), "{dep}");
        }
        assert!(check(&[krate("umcf-ui-egui", &[("egui", false), ("umcf-engine", true)])]).is_empty());
    }

    #[test]
    fn format_crates_stay_narrow() {
        // L0 is below foam-dict, but schema is not, and model is not allowed either way.
        let v = check(&[krate("umcf-foam-mesh", &[("umcf-schema", true)])]);
        assert!(v.iter().any(|x| matches!(x, Violation::FormatDep { .. })), "{v:?}");
    }

    #[test]
    fn unregistered_crate_fails_and_apps_are_free() {
        assert!(matches!(check(&[krate("umcf-whatever", &[])])[..], [Violation::Unregistered { .. }]));
        assert!(check(&[krate("umcf-gui", &[("umcf-ui-egui", true), ("eframe", false)])]).is_empty());
    }

    #[test]
    fn dev_deps_are_ignored() {
        let c = Crate {
            name: "umcf-foam-dict".into(),
            deps: vec![Dep { name: "umcf-engine".into(), kind: DepKind::Dev, workspace: true }],
        };
        assert!(check(&[c]).is_empty());
    }
}
