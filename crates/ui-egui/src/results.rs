//! What the results card shows besides diagnostics: a workflow description (`workflow.show`,
//! `workflow.import`): the problems in it and the notes of its import first, then its settings,
//! then region by region. Each part has a limit of its own, so neither can hide the other.

use serde_json::Value;

/// The most problem and note lines shown, and the most description lines.
pub const MESSAGES: usize = 200;
pub const DESCRIPTION: usize = 300;
/// The most steps listed for one region, and restored fields for one `setFields`.
const ITEMS: usize = 20;

/// The first [`ITEMS`] of `items`, then how many more there are.
fn some(items: impl ExactSizeIterator<Item = String>) -> Vec<String> {
    let more = items.len().saturating_sub(ITEMS);
    let mut out: Vec<String> = items.take(ITEMS).collect();
    if more > 0 {
        out.push(format!("… and {more} more"));
    }
    out
}

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
    let restore = some(list(st, "restore").iter().map(|r| format!("{}→{}", s(r, "backup"), s(r, "field"))));
    if restore.is_empty() { name } else { format!("{name}(restore {})", restore.join(", ")) }
}

/// Keeps the first `limit` lines of `part`, and says how many more there are.
fn capped(out: &mut Vec<(Kind, String)>, part: Vec<(Kind, String)>, limit: usize) {
    let more = part.len().saturating_sub(limit);
    out.extend(part.into_iter().take(limit));
    if more > 0 {
        out.push((Kind::Info, format!("… and {more} more")));
    }
}

/// The lines of a command's workflow `description` with its `problems` and `notes` (at most
/// [`MESSAGES`] of these and [`DESCRIPTION`] of the description, each with a line saying how many
/// more); empty when it returned no description (other commands' notes and problems are theirs
/// to show).
pub fn lines(data: &Value) -> Vec<(Kind, String)> {
    let mut out = Vec::new();
    let Some(d) = data.get("description").filter(|d| d.is_object()) else { return out };
    let mut messages = Vec::new();
    for p in list(data, "problems") {
        messages.push((Kind::Warning, format!("{}: {}", s(&p, "at"), s(&p, "message"))));
    }
    for n in list(data, "notes") {
        if let Some(n) = n.as_str() {
            messages.push((Kind::Info, n.to_string()));
        }
    }
    capped(&mut out, messages, MESSAGES);
    let mut desc = Vec::new();
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
    desc.push((Kind::Text, settings.join(", ")));
    for r in list(d, "regions") {
        let mesh = match r.get("mesh") {
            Some(Value::String(m)) => format!(", mesh {m}"),
            Some(Value::Array(steps)) => {
                format!(", mesh {}", some(steps.iter().map(step)).join(" "))
            }
            _ => String::new(),
        };
        let steps = some(list(&r, "steps").iter().map(step));
        let steps = if steps.is_empty() { String::new() } else { format!(", steps {}", steps.join(" ")) };
        desc.push((Kind::Text, format!("{} ({}){mesh}{steps}", s(&r, "name"), s(&r, "kind"))));
        for p in list(&r, "patches") {
            let role = p.get("role").and_then(Value::as_str).unwrap_or("no role");
            let partner = p
                .get("partner")
                .filter(|q| q.is_object())
                .map(|q| format!(" → {}/{}", s(q, "region"), s(q, "patch")))
                .unwrap_or_default();
            desc.push((Kind::Text, format!("    {}: {role}{partner}", s(&p, "name"))));
        }
    }
    capped(&mut out, desc, DESCRIPTION);
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
    fn many_notes_dont_hide_the_description() {
        let notes: Vec<String> = (0..600).map(|i| format!("note {i}")).collect();
        let restore: Vec<Value> = (0..30).map(|i| json!({ "backup": format!("b{i}"), "field": "T" })).collect();
        let data = json!({
            "description": { "solver": null, "regions": [
                { "name": "air", "kind": "fluid", "steps": [{ "step": "setFields", "restore": restore }], "patches": [] },
            ] },
            "notes": notes,
        });
        let l = lines(&data);
        assert_eq!(l[MESSAGES], (Kind::Info, "… and 400 more".to_string()));
        assert_eq!(l[MESSAGES + 1], (Kind::Text, "no solver run".to_string()));
        assert!(l[MESSAGES + 2].1.ends_with("b19→T, … and 10 more)"), "{:?}", l[MESSAGES + 2]);
        assert_eq!(l.len(), MESSAGES + 3);
    }

    #[test]
    fn many_steps_are_cut_short() {
        let steps: Vec<Value> = (0..25).map(|_| json!({ "step": "topoSet" })).collect();
        let data = json!({ "description": { "solver": null, "regions": [
            { "name": "air", "kind": "fluid", "mesh": steps, "steps": steps, "patches": [] },
        ] } });
        let l = lines(&data);
        let twenty = vec!["topoSet"; 20].join(" ");
        assert_eq!(l[1].1, format!("air (fluid), mesh {twenty} … and 5 more, steps {twenty} … and 5 more"));
    }

    #[test]
    fn other_results_have_no_lines() {
        assert!(lines(&json!({ "diagnostics": [] })).is_empty());
        assert!(lines(&json!(null)).is_empty());
        assert!(lines(&json!({ "description": "text", "notes": [1] })).is_empty());
        assert!(lines(&json!({ "problems": ["a"], "notes": ["b"] })).is_empty());
    }
}
