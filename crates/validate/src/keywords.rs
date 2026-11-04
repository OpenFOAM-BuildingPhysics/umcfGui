//! `keyword.missing`: the required keywords of the model selected in a region's dictionary.
//! The catalog lists those a type reads whenever the solver builds it (see
//! `docs/solver-coverage.md`, "Keywords read at start"); the ones read from the type's
//! coefficients (`coeffs_`, the `<Type>Coeffs` dictionary) are checked here.

use umcf_foam_dict::{Dict, Item, unquote};
use umcf_model::Case;
use umcf_schema::CaseKind;
use umcf_schema::catalog::catalog;

use crate::fields::{in_version, kind_name, selected};
use crate::{Diagnostic, Level};

/// The dictionary each family's type is chosen in, and its selector.
fn chooser(
    base: &str,
    dict_file: Option<&'static str>,
    selector: Option<&'static str>,
) -> Option<(&'static str, &'static str)> {
    match (dict_file, selector) {
        (Some(f), Some(s)) => Some((f, s)),
        _ if base == "radiationModel" => Some(("radiationProperties", "radiationModel")),
        _ => None,
    }
}

/// A dictionary whose entries can come from elsewhere (`#include`, `$macro`) or match by
/// pattern (`"(a|b)"`): a key not written in it may still be found.
fn open_ended(d: &Dict) -> bool {
    d.items.iter().any(|i| match i {
        Item::Directive { .. } | Item::Macro { .. } => true,
        Item::Entry(e) => {
            let k = e.key.text.as_str();
            k.starts_with('#') || k.starts_with('$') || k.starts_with('"')
        }
        Item::Value(_) => false,
    })
}

pub(crate) fn check(case: &Case, out: &mut Vec<Diagnostic>) {
    if case.layout.kind != CaseKind::Solver {
        return;
    }
    let cat = catalog();
    let v = case.layout.version;
    for r in &case.regions {
        let Some(kind) = kind_name(r.kind) else { continue };
        if r.name.is_empty() {
            continue;
        }
        let builds =
            |class: &str| cat.builds.iter().any(|b| b.region == kind && b.class == class && in_version(b.versions, v));
        for fam in cat.families.iter().filter(|f| builds(f.base)) {
            let Some((file, key)) = chooser(fam.base, fam.dict_file, fam.selector) else { continue };
            let Some(chosen) = selected(case, &r.name, file, &[key]) else { continue };
            let Some(t) = cat.find(fam.base, &chosen) else { continue };
            let rel = format!("constant/{}/{file}", r.name);
            let Some(doc) = case.dict(&rel) else { continue };
            if open_ended(&doc.root) {
                continue;
            }
            let coeffs = format!("{}Coeffs", t.name);
            // Written twice, the blocks merge (OpenFOAM's default `#inputMode merge`). Missing
            // altogether: the type reads an empty dictionary (subOrEmptyDict).
            let blocks: Vec<_> = doc.root.entries().filter(|e| unquote(&e.key.text) == coeffs).collect();
            let mut dicts: Vec<&Dict> = Vec::new();
            for e in &blocks {
                match e.dict() {
                    Some(d) if !open_ended(d) => dicts.push(d),
                    // `$macro` or something else: what it holds can't be told here.
                    _ => break,
                }
            }
            if dicts.len() != blocks.len() {
                continue;
            }
            let dict = (!dicts.is_empty()).then_some(&dicts);
            let needed = cat.startup_keywords.iter().filter(|k| {
                k.family == fam.base && k.type_name == t.name && k.receiver == "coeffs_" && in_version(k.versions, v)
            });
            for k in needed {
                if dict.is_some_and(|ds| ds.iter().any(|d| d.entries().any(|e| unquote(&e.key.text) == k.name))) {
                    continue;
                }
                let place =
                    if dict.is_some() { format!("`{coeffs}`") } else { format!("`{coeffs}` (not in the file)") };
                out.push(
                    Diagnostic::new(
                        Level::Error,
                        "keyword.missing",
                        &rel,
                        format!(
                            "{} `{}` needs `{}` in {place}: the solver reads it when it builds the model ({}), and stops without it",
                            fam.base, t.name, k.name, t.source
                        ),
                    )
                    .region(&r.name),
                );
            }
        }
    }
}
