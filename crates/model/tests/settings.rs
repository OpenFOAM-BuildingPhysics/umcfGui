//! The settings of real cases (`umcf_model::settings`).
//!
//! `UMCF_CASES` (`:`-separated case directories): each case's settings read from its files
//! (every value or a note why not), their `settings` block reads back the same, the whole
//! workflow description of the case (its import) writes and reads back without problems, and
//! the settings written back into the case change nothing (the files are compared in memory;
//! the case on disk is never written).

use umcf_core::DirFs;
use umcf_foam_dict::Document;
use umcf_model::description::Description;
use umcf_model::settings::Settings;
use umcf_model::{Case, OpenOptions};

#[test]
fn real_cases_settings_read_write_and_change_nothing_when_written_back() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!(
            "SKIP real_cases_settings_read_write_and_change_nothing_when_written_back: set UMCF_CASES=<case>:<case>…"
        );
        return;
    };
    let mut failures = Vec::new();
    for dir in cases.split(':').filter(|c| !c.is_empty()) {
        let fs = DirFs::new(dir);
        // Fields aren't needed for the settings.
        let options = OpenOptions { field_bytes: 0, ..OpenOptions::default() };
        let case = match Case::open_with(&fs, options) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{dir}: {e}"));
                continue;
            }
        };
        let (s, notes) = Settings::from_case(&case);
        eprintln!("{dir}: {}", s.write().replace('\n', " ").split_whitespace().collect::<Vec<_>>().join(" "));
        for n in &notes {
            eprintln!("    note: {n}");
        }
        let d = Description { settings: s.clone(), ..Default::default() };
        let (back, problems) = Description::read(&Document::parse(&d.write()).expect("written settings parse"));
        if !problems.is_empty() || back.settings != s {
            failures.push(format!("{dir}: the settings block reads back as {:?}, {problems:?}", back.settings));
        }
        let (imported, _) = Description::import(&case);
        let (back, problems) = Description::read(&Document::parse(&imported.write()).expect("import parses"));
        if !problems.is_empty() || back.settings != imported.settings {
            failures.push(format!("{dir}: the imported description reads back with {problems:?}"));
        }
        let mut written = case.clone();
        match written.apply_settings(&s) {
            Ok(a) => {
                for rel in &a.changed {
                    let (before, after) =
                        (case.dict(rel).map(|d| d.to_string()), written.dict(rel).map(|d| d.to_string()));
                    if before != after {
                        failures.push(format!("{dir}: writing its own settings back changed {rel}"));
                    }
                }
            }
            Err(e) => failures.push(format!("{dir}: apply: {e}")),
        }
        // Other values: the entries they go to change, every other line stays as it was.
        let other = Settings {
            delta_t: s.delta_t.as_ref().map(|_| "123".to_string()),
            end_time: s.end_time.as_ref().map(|_| "4567".to_string()),
            turbulence: s
                .turbulence
                .as_ref()
                .map(|t| if t == "kEpsilon" { "realizableKE" } else { "kEpsilon" }.to_string()),
            temperature_limit: s.temperature_limit.as_ref().map(|_| ("271.5".to_string(), "371.5".to_string())),
            ..Default::default()
        };
        let mut written = case.clone();
        match written.apply_settings(&other) {
            Ok(a) => {
                for rel in &a.changed {
                    let before = case.dict(rel).map(|d| d.to_string()).unwrap_or_default();
                    let after = written.dict(rel).map(|d| d.to_string()).unwrap_or_default();
                    let (b, w): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
                    if b.len() != w.len() {
                        failures.push(format!("{dir}: {rel}: lines added or taken away"));
                        continue;
                    }
                    for (x, y) in b.iter().zip(&w).filter(|(x, y)| x != y) {
                        let new = ["123", "4567", "realizableKE", "kEpsilon", "271.5", "371.5"];
                        if !new.iter().any(|n| y.contains(n))
                            || x.split_whitespace().next() != y.split_whitespace().next()
                        {
                            failures.push(format!("{dir}: {rel}: `{x}` became `{y}`"));
                        }
                    }
                }
                if Settings::from_case(&written).0.delta_t != other.delta_t {
                    failures.push(format!("{dir}: deltaT not read back"));
                }
            }
            Err(e) => failures.push(format!("{dir}: apply other values: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
