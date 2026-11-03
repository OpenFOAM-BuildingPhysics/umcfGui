//! Opening real cases.
//!
//! `UMCF_CASES` (`:`-separated case directories): every case opens, every listed region is
//! there, every region with a mesh has its patches, every field of the start time reads (or
//! is noted as too large). Then `endTime` is edited and saved into an in-memory overlay (the
//! case on disk is never written): exactly that entry changes, and the rest of the file is
//! byte-identical.

// Native-only test: timing with the clock is fine (the lint guards wasm builds).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::time::Instant;

use umcf_core::{CaseFs, CaseWrite, DirFs, FsError};
use umcf_model::{CONTROL_DICT, Case};

/// Reads from the case on disk, writes to memory.
struct Overlay {
    disk: DirFs,
    written: BTreeMap<String, Vec<u8>>,
}

impl CaseFs for Overlay {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        match self.written.get(rel) {
            Some(b) => Ok(Some(b.clone())),
            None => self.disk.read(rel, limit),
        }
    }
    fn is_file(&self, rel: &str) -> bool {
        self.written.contains_key(rel) || self.disk.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.disk.is_dir(rel)
    }
    fn list(&self, rel: &str) -> Result<Vec<String>, FsError> {
        self.disk.list(rel)
    }
}

impl CaseWrite for Overlay {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), FsError> {
        self.written.insert(rel.to_string(), bytes.to_vec());
        Ok(())
    }
}

#[test]
fn reference_cases_open_and_save() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_cases_open_and_save: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    let mut failures = Vec::new();
    for dir in cases.split(':').filter(|c| !c.is_empty()) {
        let started = Instant::now();
        let mut fs = Overlay { disk: DirFs::new(dir), written: BTreeMap::new() };
        let case = match Case::open(&fs) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{dir}: {e}"));
                continue;
            }
        };
        let listed: Vec<&str> = case.layout.regions.all().collect();
        let opened: Vec<&str> = case.regions.iter().map(|r| r.name.as_str()).filter(|r| !r.is_empty()).collect();
        if listed != opened {
            failures.push(format!("{dir}: regions {opened:?}, layout lists {listed:?}"));
        }
        let fields: usize = case.regions.iter().map(|r| r.fields.len()).sum();
        let with_bcs: usize = case.regions.iter().flat_map(|r| &r.fields).filter(|f| f.bcs.is_some()).count();
        eprintln!(
            "{dir}: {:?} {:?}, start {:?}, {} regions ({} with a mesh), {fields} fields ({with_bcs} with BCs), {} dictionaries, {} notes, {:.1}s",
            case.layout.kind,
            case.layout.version,
            case.start.as_ref().map(|t| t.name.as_str()),
            case.regions.len(),
            case.regions.iter().filter(|r| r.patches.is_some()).count(),
            case.dict_paths().count(),
            case.notes.len(),
            started.elapsed().as_secs_f64()
        );
        for n in &case.notes {
            eprintln!("    {:?} {}: {}", n.severity, n.path, n.message);
        }

        // Edit and save: one entry changes, nothing else.
        let mut edited = case.clone();
        let Some(before) = edited.dict(CONTROL_DICT).map(|d| d.to_string()) else {
            failures.push(format!("{dir}: controlDict not loaded"));
            continue;
        };
        if let Err(e) = edited.set(CONTROL_DICT, &["endTime"], "123456789") {
            failures.push(format!("{dir}: set: {e}"));
            continue;
        }
        match edited.save(&mut fs, Some(".umcfgui/backup/test")) {
            Ok(saved) if saved == [CONTROL_DICT] => {}
            Ok(saved) => failures.push(format!("{dir}: saved {saved:?}")),
            Err(e) => failures.push(format!("{dir}: save: {e}")),
        }
        let after = fs.written.get(CONTROL_DICT).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
        let old_end = case.controls.end_time.clone().unwrap_or_default();
        let diff: Vec<(&str, &str)> = before.lines().zip(after.lines()).filter(|(a, b)| a != b).collect();
        let one_line = diff.len() == 1
            && diff[0].0.replacen(&old_end, "123456789", 1) == diff[0].1
            && before.lines().count() == after.lines().count();
        if !one_line {
            failures.push(format!("{dir}: the edit changed {diff:?}"));
        }
        let backup = fs.written.get(".umcfgui/backup/test/system/controlDict");
        if backup.map(Vec::as_slice) != Some(before.as_bytes()) {
            failures.push(format!("{dir}: backup is not the original controlDict"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
