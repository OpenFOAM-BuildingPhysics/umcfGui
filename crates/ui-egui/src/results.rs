//! What the results card shows besides diagnostics: a workflow description (`workflow.show`,
//! `workflow.import`): the problems in it and the notes of its import first (so a long
//! description can't push them past the card's limit), then its settings, then region by region.

use serde_json::Value;

/// How a line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Warning,
    Info,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn list(v: &Value, k: &str) -> Vec<Value> {
    v.get(k).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// A step with what it is given: `blockMesh(blockMeshDict.air)`, `setSet(batch)`,
/// `setFields(restore 0/T.orig→T)`.
fn step(st: &Value) -> String {
    let name = s(st, "step");
    let arg = ["dict", "batch"].iter().find_map(|k| st.get(*k).and_then(Value::as_str));
    if let Some(a) = arg {
        return format!("{name}({a})");
    }
    let restore: Vec<String> =
        list(st, "restore").iter().map(|r| format!("{}→{}", s(r, "backup"), s(r, "field"))).collect();
    if restore.is_empty() { name } else { format!("{name}(restore {})", restore.join(", ")) }
}

/// The lines of a command's workflow `description` with its `problems` and `notes`; empty when it
/// returned no description (other commands' notes and problems are theirs to show).
pub fn lines(data: &Value) -> Vec<(Kind, String)> {
    let mut out = Vec::new();
    let Some(d) = data.get("description").filter(|d| d.is_object()) else { return out };
    for p in list(data, "problems") {
        out.push((Kind::Warning, format!("{}: {}", s(&p, "at"), s(&p, "message"))));
    }
    for n in list(data, "notes") {
        if let Some(n) = n.as_str() {
            out.push((Kind::Info, n.to_string()));
        }
    }
    let mut settings = Vec::new();
    if let Some(v) = d.get("version").and_then(Value::as_str) {
        settings.push(format!("for {v}"));
    }
    if d.get("features").and_then(Value::as_bool) == Some(true) {
        settings.push("surfaceFeatures".to_string());
    }
    if let Some(r) = d.get("radiation").filter(|r| r.is_object()) {
        let mut what = Vec::new();
        if r.get("viewFactors").and_then(Value::as_bool) == Some(true) {
            what.push("view factors".to_string());
        }
        if r.get("solarRayTracing").and_then(Value::as_bool) == Some(true) {
            what.push("solar ray tracing".to_string());
        }
        if let Some(l) = r.get("lai").and_then(Value::as_str) {
            what.push(format!("LAI on {l}"));
        }
        let what = if what.is_empty() { String::new() } else { format!(" ({})", what.join(", ")) };
        settings.push(format!("radiation on {}{what}", s(r, "region")));
    }
    match d.get("solver").and_then(Value::as_str) {
        Some(sv) => settings.push(format!("solver {sv}")),
        None => settings.push("no solver run".to_string()),
    }
    if d.get("reconstruct").and_then(Value::as_bool) == Some(true) {
        settings.push("reconstructed".to_string());
    }
    out.push((Kind::Text, settings.join(", ")));
    for r in list(d, "regions") {
        let mesh = match r.get("mesh") {
            Some(Value::String(m)) => format!(", mesh {m}"),
            Some(Value::Array(steps)) => {
                format!(", mesh {}", steps.iter().map(step).collect::<Vec<_>>().join(" "))
            }
            _ => String::new(),
        };
        let steps: Vec<String> = list(&r, "steps").iter().map(step).collect();
        let steps = if steps.is_empty() { String::new() } else { format!(", steps {}", steps.join(" ")) };
        out.push((Kind::Text, format!("{} ({}){mesh}{steps}", s(&r, "name"), s(&r, "kind"))));
        for p in list(&r, "patches") {
            let role = p.get("role").and_then(Value::as_str).unwrap_or("no role");
            let partner = p
                .get("partner")
                .filter(|q| q.is_object())
                .map(|q| format!(" → {}/{}", s(q, "region"), s(q, "patch")))
                .unwrap_or_default();
            out.push((Kind::Text, format!("    {}: {role}{partner}", s(&p, "name"))));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_description_is_shown_problems_first_then_settings_then_regions() {
        let data = json!({
            "description": {
                "version": "OpenFOAM-8", "features": true, "solver": "urbanMicroclimateFoam", "reconstruct": true,
                "radiation": { "region": "street", "lai": "air", "viewFactors": true, "solarRayTracing": false },
                "regions": [
                    { "name": "air", "kind": "fluid", "mesh": "blocks",
                      "steps": [
                        { "step": "createPatch" },
                        { "step": "setSet", "batch": "system/air/setSet.batch" },
                        { "step": "setFields", "restore": [{ "backup": "0/air/T.orig", "field": "T" }] },
                      ],
                      "patches": [
                        { "name": "inlet", "role": "inlet", "partner": null },
                        { "name": "ground", "role": "coupled", "partner": { "region": "street", "patch": "top" } },
                        { "name": "odd", "role": null, "partner": null },
                      ] },
                    { "name": "street", "kind": "solid", "mesh": [{ "step": "blockMesh", "dict": "blockMeshDict.street" }],
                      "steps": [], "patches": [] },
                ],
            },
            "problems": [{ "at": "regions/air", "message": "unknown key" }],
            "notes": ["a note"],
        });
        assert_eq!(
            lines(&data),
            vec![
                (Kind::Warning, "regions/air: unknown key".to_string()),
                (Kind::Info, "a note".to_string()),
                (
                    Kind::Text,
                    "for OpenFOAM-8, surfaceFeatures, radiation on street (view factors, LAI on air), solver urbanMicroclimateFoam, reconstructed"
                        .to_string()
                ),
                (
                    Kind::Text,
                    "air (fluid), mesh blocks, steps createPatch setSet(system/air/setSet.batch) setFields(restore 0/air/T.orig→T)"
                        .to_string()
                ),
                (Kind::Text, "    inlet: inlet".to_string()),
                (Kind::Text, "    ground: coupled → street/top".to_string()),
                (Kind::Text, "    odd: no role".to_string()),
                (Kind::Text, "street (solid), mesh blockMesh(blockMeshDict.street)".to_string()),
            ]
        );
    }

    #[test]
    fn an_empty_description_still_shows() {
        let data = json!({ "description": { "regions": [], "solver": null }, "problems": [], "notes": [] });
        assert_eq!(lines(&data), vec![(Kind::Text, "no solver run".to_string())]);
    }

    #[test]
    fn other_results_have_no_lines() {
        assert!(lines(&json!({ "diagnostics": [] })).is_empty());
        assert!(lines(&json!(null)).is_empty());
        assert!(lines(&json!({ "description": "text", "notes": [1] })).is_empty());
        assert!(lines(&json!({ "problems": ["a"], "notes": ["b"] })).is_empty());
    }
}
