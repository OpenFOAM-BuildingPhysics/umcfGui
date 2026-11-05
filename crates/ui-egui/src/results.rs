//! What the results card shows besides diagnostics: a workflow description (`workflow.show`,
//! `workflow.import`), region by region, with the problems in it and the notes of its import.

use serde_json::Value;

/// How a line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Warning,
    Info,
}

/// The lines of a command's workflow `description`, then its `problems` and `notes`; empty when
/// it returned no description (other commands' notes and problems are theirs to show).
pub fn lines(data: &Value) -> Vec<(Kind, String)> {
    let mut out = Vec::new();
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let list = |v: &Value, k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let Some(d) = data.get("description").filter(|d| d.is_object()) else { return out };
    for r in list(d, "regions") {
        let mesh = match r.get("mesh") {
            Some(Value::String(m)) => format!(", mesh {m}"),
            Some(Value::Array(steps)) => {
                let steps: Vec<String> = steps.iter().map(|st| s(st, "step")).collect();
                format!(", mesh {}", steps.join(" "))
            }
            _ => String::new(),
        };
        let steps: Vec<String> = list(&r, "steps").iter().map(|st| s(st, "step")).collect();
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
    for p in list(data, "problems") {
        out.push((Kind::Warning, format!("{}: {}", s(&p, "at"), s(&p, "message"))));
    }
    for n in list(data, "notes") {
        if let Some(n) = n.as_str() {
            out.push((Kind::Info, n.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_description_is_shown_region_by_region() {
        let data = json!({
            "description": { "regions": [
                { "name": "air", "kind": "fluid", "mesh": "blocks", "steps": [{ "step": "createPatch" }],
                  "patches": [
                    { "name": "inlet", "role": "inlet", "partner": null },
                    { "name": "ground", "role": "coupled", "partner": { "region": "street", "patch": "top" } },
                    { "name": "odd", "role": null, "partner": null },
                  ] },
                { "name": "street", "kind": "solid", "mesh": [{ "step": "blockMesh", "dict": null }], "steps": [], "patches": [] },
            ] },
            "problems": [{ "at": "regions/air", "message": "unknown key" }],
            "notes": ["a note"],
        });
        assert_eq!(
            lines(&data),
            vec![
                (Kind::Text, "air (fluid), mesh blocks, steps createPatch".to_string()),
                (Kind::Text, "    inlet: inlet".to_string()),
                (Kind::Text, "    ground: coupled → street/top".to_string()),
                (Kind::Text, "    odd: no role".to_string()),
                (Kind::Text, "street (solid), mesh blockMesh".to_string()),
                (Kind::Warning, "regions/air: unknown key".to_string()),
                (Kind::Info, "a note".to_string()),
            ]
        );
    }

    #[test]
    fn other_results_have_no_lines() {
        assert!(lines(&json!({ "diagnostics": [] })).is_empty());
        assert!(lines(&json!(null)).is_empty());
        assert!(lines(&json!({ "description": "text", "notes": [1] })).is_empty());
        assert!(lines(&json!({ "problems": ["a"], "notes": ["b"] })).is_empty());
    }
}
