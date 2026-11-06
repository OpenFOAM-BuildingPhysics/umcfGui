//! `cargo xtask make-examples [--check]`: makes `examples/of8` and `examples/of12` from the
//! street canyon cases of the urbanMicroclimateFoam tutorials, at two pinned commits, made
//! small enough to run in seconds (`examples/README.md` lists every change). `--check` makes
//! them in a temporary directory and fails unless `examples/` is the same, byte for byte and
//! with the same executable bits.
//!
//! The tutorials come from `UMCF_TUTORIALS` (a clone that has both commits; it is only read),
//! else they are fetched from GitHub into a temporary directory. Needs `git` and `tar`.

// Native-only tooling: the clock is fine (the lint guards wasm builds).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use regex::Regex;

use crate::examples::copy_dir;
use crate::root;

const TUTORIALS: &str = "https://github.com/OpenFOAM-BuildingPhysics/urbanMicroclimateFoam-tutorials.git";

/// The tutorial commit each version's examples are made from. Not the tags `of-org_v8.0` and
/// `of-org_v12.0`: those moved to later commits, whose cases differ.
const PINS: [(&str, &str); 2] = [
    ("of8", "64ae5d2cf2f7003d78d767f373c283b8e930a081"), // 2024-05-08, "Adapt tutorials for openfoam-org v8.0"
    ("of12", "b8135bae77231ddda1ab7e899253323d5c85612d"), // 2026-03-23, "Remove nearest sampleMode ..."
];

/// The weather and solar tables, shifted to start at 11:00 so that the two steps have the sun up.
const TABLES: [&str; 5] =
    ["0/air/Tambient", "0/air/wambient", "constant/IDN", "constant/Idif", "constant/sunPosVector"];
const SHIFT: f64 = 39600.0;
const DAY: f64 = 86400.0;

/// `pattern` compiled; an error names it.
fn re(pattern: &str) -> Result<Regex, String> {
    Regex::new(pattern).map_err(|e| format!("regex {pattern}: {e}"))
}

/// The text of the file at `path`.
fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes `text` to the file at `path`, replacing it.
fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The entries of `dir` that a shell's `dir/*` lists (not hidden ones), sorted; an entry that
/// can't be read is an error, never skipped.
fn entries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        if !e.file_name().to_string_lossy().starts_with('.') {
            out.push(e.path());
        }
    }
    out.sort();
    Ok(out)
}

/// Removes the file at `path`; one that isn't there is fine.
fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
        _ => Ok(()),
    }
}

/// Line endings made `\n` (`\r\n` and a lone `\r` too), as a rewritten file has them.
fn lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `f` applied to each line, every byte between them kept (a `\r`, a missing last newline).
fn each_line(text: &str, mut f: impl FnMut(&str) -> String) -> String {
    text.split('\n').map(&mut f).collect::<Vec<_>>().join("\n")
}

/// The first match of `pattern` in each line replaced by `with` (`${1}` for a group).
fn sub_lines(text: &str, pattern: &Regex, with: &str) -> String {
    each_line(text, |l| pattern.replacen(l, 1, with).into_owned())
}

/// The same on bytes, for the files only edited line by line: they are kept byte for byte,
/// whatever their encoding.
fn re_bytes(pattern: &str) -> Result<regex::bytes::Regex, String> {
    regex::bytes::Regex::new(pattern).map_err(|e| format!("regex {pattern}: {e}"))
}

/// The bytes of the file at `path`.
fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes `bytes` to the file at `path`, replacing it.
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// `f` applied to each line of `text`, the bytes between them kept.
fn each_line_bytes(text: &[u8], mut f: impl FnMut(&[u8]) -> Vec<u8>) -> Vec<u8> {
    text.split(|b| *b == b'\n').map(&mut f).collect::<Vec<_>>().join(&b"\n"[..])
}

/// The first match of `pattern` in each line of `text` replaced by `with` (`${1}` for a group).
fn sub_lines_bytes(text: &[u8], pattern: &regex::bytes::Regex, with: &str) -> Vec<u8> {
    each_line_bytes(text, |l| pattern.replacen(l, 1, with.as_bytes()).into_owned())
}

/// Cells along an edge of length `length` that had `cells`: through a wall or the street (a
/// solid, at most 2.5 m) the graded resolution is kept, at most 20; elsewhere 3 to 6, one per
/// 10 m, so the solids have the air's cells along them and each coupled face has one partner.
fn cell_count(length: f64, solid: bool, cells: u64) -> u64 {
    if solid && length <= 2.5 {
        return cells.min(20);
    }
    // Halves round to even, as the generator this replaces did.
    let along = if length > 30.0 { (length / 10.0).round_ties_even() } else { 3.0 };
    // A finite, small count: 3 to 6.
    along.clamp(3.0, 6.0) as u64
}

/// A region's `blockMeshDict` with test-sized cell counts; the air's blocks uniform.
fn block_mesh(text: &str, solid: bool) -> Result<String, String> {
    let text = lf(text);
    let vertices = re(r"(?s)vertices\s*\((.*?)\n\s*\);")?
        .captures(&text)
        .and_then(|c| c.get(1))
        .ok_or("no vertices list")?
        .as_str();
    let num = r"[-+0-9.eE]+";
    let mut points = Vec::new();
    for c in re(&format!(r"\(\s*({num})\s+({num})\s+({num})\s*\)"))?.captures_iter(vertices) {
        let mut p = [0.0f64; 3];
        for (k, v) in p.iter_mut().enumerate() {
            let s = c.get(k + 1).map_or("", |m| m.as_str());
            *v = s.parse().map_err(|_| format!("vertex coordinate `{s}`"))?;
        }
        points.push(p);
    }
    let hex =
        re(r"hex\s*\(([\d\s]+)\)(\s*)\(([\d\s]+)\)\s*(simpleGrading\s*\((?:[^()]|\((?:[^()]|\([^()]*\))*\))*\))")?;
    let mut failed = None;
    let out = hex.replace_all(&text, |c: &regex::Captures| {
        let group = |k| c.get(k).map_or("", |m| m.as_str());
        let made = (|| -> Result<String, String> {
            let ints = |s: &str| -> Result<Vec<u64>, String> {
                s.split_whitespace().map(|t| t.parse().map_err(|_| format!("label `{t}`"))).collect()
            };
            let labels = ints(group(1))?;
            let cells = ints(group(3))?;
            let point = |k: usize| -> Result<[f64; 3], String> {
                let label = labels.get(k).ok_or("a hex with fewer than 8 points")?;
                let i = usize::try_from(*label).map_err(|_| "a point label out of range")?;
                points.get(i).copied().ok_or_else(|| format!("point {label} is not in the vertices"))
            };
            let origin = point(0)?;
            let mut counts = Vec::new();
            for (axis, corner) in [1, 3, 4].into_iter().enumerate() {
                let p = point(corner)?;
                // The edges of the tutorials' blocks are along the axes, where this is exact; on
                // an oblique edge it can differ from Python's math.dist in the last bit.
                let length = origin.iter().zip(p).map(|(a, b)| (a - b) * (a - b)).sum::<f64>().sqrt();
                let had = *cells.get(axis).ok_or("a hex with fewer than 3 cell counts")?;
                counts.push(cell_count(length, solid, had).to_string());
            }
            let grading = if solid { group(4).to_string() } else { "simpleGrading (1 1 1)".to_string() };
            Ok(format!("hex ({}){}({}) {grading}", group(1), group(2), counts.join(" ")))
        })();
        made.unwrap_or_else(|e| {
            failed.get_or_insert(e);
            c.get(0).map_or("", |m| m.as_str()).to_string()
        })
    });
    match failed {
        Some(e) => Err(e),
        None => Ok(out.into_owned()),
    }
}

/// `controlDict` for the short run: `endTime 7200`, at most 30 fluid iterations, the probes
/// written at every step.
fn control_dict(text: &str) -> Result<String, String> {
    let mut s = re(r"(\nendTime\s+)[^;]+;")?.replace_all(&lf(text), "${1}7200;").into_owned();
    if !s.contains("maxFluidIteration") {
        s = re(r"(\nwriteFormat)")?.replacen(&s, 1, "\nmaxFluidIteration 30;${1}").into_owned();
    }
    let s = sub_lines(&s, &re(r"^(maxFluidIteration\s+)[0-9]+;")?, "${1}30;");
    // From a line with `probes` to the next line with `}`, as often as it comes.
    let interval = re(r"(writeInterval\s+)10;")?;
    let mut inside = false;
    Ok(each_line(&s, |l| {
        let out =
            if inside || l.contains("probes") { interval.replacen(l, 1, "${1}1;").into_owned() } else { l.to_string() };
        inside = if inside { !l.contains('}') } else { l.contains("probes") };
        out
    }))
}

/// A daily table started at 11:00: one day repeated is rotated, several days are cut.
fn shift_table(text: &str, name: &str) -> Result<String, String> {
    let row = re(r"^\s*\(\s*(\d+(?:\.\d*)?)\s+(.*)\)\s*$")?;
    let text = lf(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut rows = Vec::new();
    for (k, l) in lines.iter().enumerate() {
        if let Some(c) = row.captures(l) {
            let t = c.get(1).map_or("", |m| m.as_str());
            let t: f64 = t.parse().map_err(|_| format!("{name}: time `{t}`"))?;
            rows.push((k, t, c.get(2).map_or("", |m| m.as_str()).trim_end().to_string()));
        }
    }
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Err(format!("{name}: no table rows"));
    };
    if first.1 != 0.0 || last.1 < DAY || !rows.iter().any(|r| r.1 == SHIFT) {
        return Err(format!("{name}: not a table from 0 over a day or more"));
    }
    let (first, last) = (first.0, last.0);
    let mut moved: Vec<(f64, String)> = if rows.last().is_some_and(|r| r.1 == DAY) {
        let mut m: Vec<(f64, String)> =
            rows.iter().filter(|r| r.1 < DAY).map(|r| ((r.1 - SHIFT).rem_euclid(DAY), r.2.clone())).collect();
        m.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let start = m.first().map(|r| r.1.clone()).unwrap_or_default();
        m.push((DAY, start));
        m
    } else {
        rows.iter().filter(|r| r.1 >= SHIFT).map(|r| (r.1 - SHIFT, r.2.clone())).collect()
    };
    // Whole seconds, as written: the times are whole.
    let out: Vec<String> = moved.drain(..).map(|(t, v)| format!("( {} {v} )", t.trunc() as i64)).collect();
    let mut all: Vec<String> = lines.get(..first).unwrap_or_default().iter().map(|l| l.to_string()).collect();
    all.extend(out);
    all.extend(lines.get(last + 1..).unwrap_or_default().iter().map(|l| l.to_string()));
    Ok(all.join("\n"))
}

/// The air's `fvSolution` with lower relaxation, so the coarse mesh's first iterations stay
/// stable.
fn fv_solution(text: &[u8]) -> Result<Vec<u8>, String> {
    let s = sub_lines_bytes(text, &re_bytes(r#"("\(h\|e\)"\s+)0\.9;"#)?, "${1}0.5;");
    let s = sub_lines_bytes(&s, &re_bytes(r"(^\s+U\s+)0\.7;")?, "${1}0.5;");
    Ok(sub_lines_bytes(&s, &re_bytes(r"(p_rgh\s+)0\.5;")?, "${1}0.3;"))
}

/// `Allrun` serial: no decomposing or reconstructing, `runApplication` for `runParallel`.
fn serial(text: &str) -> Result<String, String> {
    let parallel = re(r"decomposePar|reconstructPar|NPROCS=|Decomposing regions|Reconstructing latest")?;
    let text = lf(text);
    let kept: Vec<&str> = text.split('\n').filter(|l| !parallel.is_match(l)).collect();
    Ok(kept.join("\n").replace("runParallel -np $NPROCS ", "runApplication "))
}

/// A script stopping at its first failing step: `set -e` before its first `cd `.
fn stop_on_error(text: &[u8]) -> Vec<u8> {
    let mut done = false;
    each_line_bytes(text, |l| match l.strip_prefix(b"cd ") {
        Some(rest) if !done => {
            done = true;
            [&b"set -e\ncd "[..], rest].concat()
        }
        _ => l.to_vec(),
    })
}

/// One case made small, in place.
fn make_case(case: &Path, version: &str) -> Result<(), String> {
    let system = case.join("system");
    // `system/*/blockMeshDict`.
    let dicts: Vec<PathBuf> =
        entries(&system)?.into_iter().map(|d| d.join("blockMeshDict")).filter(|p| p.is_file()).collect();
    for d in dicts {
        let solid = d.parent().and_then(|p| p.file_name()).is_some_and(|n| n != "air");
        let text = block_mesh(&read(&d)?, solid).map_err(|e| format!("{}: {e}", d.display()))?;
        write(&d, &text)?;
    }
    let cd = system.join("controlDict");
    write(&cd, &control_dict(&read(&cd)?)?)?;
    for t in TABLES {
        let p = case.join(t);
        if p.exists() {
            write(&p, &shift_table(&read(&p)?, &p.display().to_string())?)?;
        }
    }
    let fv = system.join("air/fvSolution");
    write_bytes(&fv, &fv_solution(&read_bytes(&fv)?)?)?;
    let allrun = case.join("Allrun");
    write(&allrun, &serial(&read(&allrun)?)?)?;
    if version == "of12" {
        // The regions are in system/controlDict.
        remove(&case.join("constant/regionProperties"))?;
    }
    remove(&case.join("reconstructScript"))?;
    for script in ["Allprepare", "Allrun"] {
        let p = case.join(script);
        if p.is_file() {
            write_bytes(&p, &stop_on_error(&read_bytes(&p)?))?;
        }
    }
    let prepare = case.join("Allprepare");
    if prepare.is_file() {
        // What splitMeshRegions may not write.
        let rm = re_bytes(r"^rm -r (system/vegetation/)")?;
        write_bytes(&prepare, &sub_lines_bytes(&read_bytes(&prepare)?, &rm, "rm -rf ${1}"))?;
    }
    // Each wall its own room temperature table (the tutorials cross them).
    let tamb = re_bytes(r#"(Tamb\s+"\$FOAM_CASE/0/)[a-z]+(/T_room.dat")"#)?;
    for wall in ["leeward", "windward"] {
        let ts = case.join(format!("0/{wall}/Ts"));
        if ts.is_file() {
            write_bytes(&ts, &sub_lines_bytes(&read_bytes(&ts)?, &tamb, &format!("${{1}}{wall}${{2}}")))?;
        }
    }
    if version == "of12" {
        // OpenFOAM-12's faceAgglomerate reads system/viewFactorsDict unless given the region's;
        // the other copies are not read, nor setSet's batch files (no setSet in OpenFOAM-12).
        let agglomerate = re(r"^runApplication faceAgglomerate -region ([a-z]*)$")?;
        let text = read(&allrun)?;
        let regions: Vec<String> = text
            .split('\n')
            // An empty name is no region (the shell's `[ -n "$r" ]`).
            .filter_map(|l| agglomerate.captures(l).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()))
            .filter(|r| !r.is_empty())
            .collect();
        match regions.as_slice() {
            [] => {}
            [r] => {
                let plain = format!("runApplication faceAgglomerate -region {r}");
                let given = format!("{plain} -dict system/{r}/viewFactorsDict");
                write(&allrun, &each_line(&text, |l| if l == plain { given.clone() } else { l.to_string() }))?;
                remove(&system.join("viewFactorsDict"))?;
                remove(&case.join(format!("constant/{r}/viewFactorsDict")))?;
            }
            _ => return Err(format!("{}: faceAgglomerate runs for more than one region", allrun.display())),
        }
        // `system/*/setset.batch`.
        for dir in entries(&system)?.into_iter().filter(|d| d.is_dir()) {
            remove(&dir.join("setset.batch"))?;
        }
    }
    Ok(())
}

/// Runs `git -C <dir> <args>`; its own messages go to the terminal.
fn git(args: &[&str], dir: &Path) -> Result<(), String> {
    let status = Command::new("git").arg("-C").arg(dir).args(args).status().map_err(|e| format!("git: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("git {} failed ({status})", args.join(" "))) }
}

/// A repository with both pinned commits: `UMCF_TUTORIALS`, else a shallow fetch into `scratch`.
fn tutorials(scratch: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("UMCF_TUTORIALS").filter(|v| !v.is_empty()) {
        let dir = PathBuf::from(dir);
        for (_, commit) in PINS {
            git(&["cat-file", "-e", &format!("{commit}^{{commit}}")], &dir).map_err(|e| {
                format!(
                    "UMCF_TUTORIALS ({}): commit {commit}: {e} (if the clone lacks it: `git -C {0} fetch origin`)",
                    dir.display()
                )
            })?;
        }
        return Ok(dir);
    }
    let repo = scratch.join("tutorials.git");
    std::fs::create_dir_all(&repo).map_err(|e| format!("{}: {e}", repo.display()))?;
    git(&["init", "-q", "--bare"], &repo)?;
    for (_, commit) in PINS {
        println!("    fetching tutorials {}", commit.get(..7).unwrap_or(commit));
        git(&["fetch", "-q", "--depth", "1", TUTORIALS, commit], &repo)?;
    }
    Ok(repo)
}

/// The street canyon cases of `commit`, unpacked into `out`.
fn unpack(repo: &Path, commit: &str, out: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut archive = Command::new("git")
        .arg("-C")
        .arg(repo)
        // The files as committed, whatever this machine's line ending settings.
        .args(["-c", "core.autocrlf=false", "archive", "--format=tar", commit, "--", "streetCanyon_*"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git archive: {e}"))?;
    let stdout = archive.stdout.take().ok_or("git archive: no output")?;
    let tar = Command::new("tar").arg("-x").arg("-f").arg("-").arg("-C").arg(out).stdin(stdout).status();
    // A tar that failed stops git archive with SIGPIPE: then tar's error is the one; a git that
    // failed on its own (a bad commit) comes first, as tar then only sees no archive.
    let archived = archive.wait().map_err(|e| format!("git archive: {e}"))?;
    let tar = tar.map_err(|e| format!("tar: {e}"))?;
    #[cfg(unix)]
    let piped = {
        use std::os::unix::process::ExitStatusExt;
        archived.signal() == Some(13)
    };
    #[cfg(not(unix))]
    let piped = false;
    if !archived.success() && !piped {
        return Err(format!("git archive {commit} failed ({archived})"));
    }
    if !tar.success() {
        return Err(format!("tar failed ({tar})"));
    }
    if !archived.success() {
        return Err(format!("git archive {commit} failed ({archived})"));
    }
    // No links: a file is written where it is, never through a link to somewhere else.
    files(out).map(|_| ())
}

/// Every file under `dir` by its path relative to it: its bytes and whether it is executable.
fn files(dir: &Path) -> Result<BTreeMap<PathBuf, (Vec<u8>, bool)>, String> {
    /// Adds the files under `dir` to `out`, by their path relative to `base`.
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, bool)>) -> Result<(), String> {
        for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = e.path();
            let kind = e.file_type().map_err(|e| format!("{}: {e}", path.display()))?;
            if kind.is_symlink() {
                return Err(format!("{}: a symbolic link (the examples have none)", path.display()));
            }
            if kind.is_dir() {
                walk(base, &path, out)?;
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            #[cfg(unix)]
            let exec = {
                use std::os::unix::fs::PermissionsExt;
                e.metadata().map_err(|e| format!("{}: {e}", path.display()))?.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let exec = false;
            let rel = path.strip_prefix(base).map_err(|e| e.to_string())?.to_path_buf();
            out.insert(rel, (bytes, exec));
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{}: a symbolic link (the examples have none)", dir.display()));
    }
    if dir.is_dir() {
        walk(dir, dir, &mut out)?;
    }
    Ok(out)
}

/// What differs between the made cases and `examples/`, one line each.
fn differences(made: &Path, examples: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (version, _) in PINS {
        let a = files(&made.join(version))?;
        let b = files(&examples.join(version))?;
        for (path, (bytes, exec)) in &a {
            let name = Path::new(version).join(path).display().to_string();
            match b.get(path) {
                None => out.push(format!("{name}: missing in examples/")),
                Some((other, _)) if other != bytes => out.push(format!("{name}: differs")),
                Some((_, other_exec)) if other_exec != exec => out.push(format!("{name}: executable bit differs")),
                Some(_) => {}
            }
        }
        for path in b.keys().filter(|p| !a.contains_key(*p)) {
            out.push(format!("{}: not made by the generator", Path::new(version).join(path).display()));
        }
    }
    Ok(out)
}

/// Write mode replaces only what git has: untracked, ignored or changed files there would be
/// lost.
fn examples_as_committed() -> Result<(), String> {
    let status = Command::new("git")
        .arg("-C")
        .arg(root())
        .args(["status", "--porcelain", "--ignored", "--", "examples/of8", "examples/of12"])
        .output()
        .map_err(|e| format!("git status: {e}"))?;
    if !status.status.success() {
        return Err(format!("git status failed ({})", status.status));
    }
    let changed = String::from_utf8_lossy(&status.stdout);
    if !changed.trim().is_empty() {
        return Err(format!(
            "examples/ has files git doesn't have as committed; commit, move or remove them first:\n{}",
            changed.trim_end()
        ));
    }
    Ok(())
}

/// Makes the examples in `scratch`, then checks `examples/` against them or writes them there.
fn make(scratch: &Path, check: bool) -> Result<(), String> {
    if !check {
        examples_as_committed()?;
    }
    let repo = tutorials(scratch)?;
    let made = scratch.join("examples");
    for (version, commit) in PINS {
        let out = made.join(version);
        unpack(&repo, commit, &out)?;
        let cases = entries(&out)?;
        if cases.is_empty() {
            return Err(format!("tutorials {commit}: no streetCanyon_* case"));
        }
        for case in &cases {
            make_case(case, version).map_err(|e| {
                let name = case.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                format!("{version}/{name}: {e}")
            })?;
        }
    }
    let examples = root().join("examples");
    if check {
        let diffs = differences(&made, &examples)?;
        if diffs.is_empty() {
            println!("make-examples: examples/ is what the generator makes");
            return Ok(());
        }
        for d in diffs.iter().take(20) {
            eprintln!("  {d}");
        }
        if diffs.len() > 20 {
            eprintln!("  … and {} more", diffs.len() - 20);
        }
        return Err(format!("{} files of examples/ differ from what `cargo xtask make-examples` makes", diffs.len()));
    }
    for (version, _) in PINS {
        let target = examples.join(version);
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        copy_dir(&made.join(version), &target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    println!("make-examples: wrote examples/of8 and examples/of12");
    Ok(())
}

/// `cargo xtask make-examples [--check]`.
pub fn cmd_make_examples(check: bool) -> Result<(), String> {
    println!("==> make-examples{}", if check { " --check" } else { "" });
    // A new directory of its own, only this user's: never one that is there already.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let scratch = std::env::temp_dir().join(format!("umcf-make-examples-{}-{stamp}", std::process::id()));
    let mut dir = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dir.mode(0o700);
    }
    dir.create(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let result = make(&scratch, check);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_round_to_even() {
        // 45 m: 4.5 cells rounds to 4, as Python's round() did; 55 m to 6.
        assert_eq!(cell_count(45.0, false, 10), 4);
        assert_eq!(cell_count(55.0, false, 10), 6);
        assert_eq!(cell_count(25.0, false, 10), 3);
        assert_eq!(cell_count(200.0, false, 10), 6);
        assert_eq!(cell_count(0.3, true, 40), 20);
        assert_eq!(cell_count(0.3, true, 12), 12);
    }

    #[test]
    fn hex_blocks_get_new_counts() {
        let dict = "vertices\n(\n    (0 0 0)\n    (50 0 0)\n    (50 20 0)\n    (0 20 0)\n    (0 0 1)\n    (50 0 1)\n    (50 20 1)\n    (0 20 1)\n);\nblocks\n(\n    hex (0 1 2 3 4 5 6 7) (50 20 10) simpleGrading (1 ((0.5 0.5 2) (0.5 0.5 0.5)) 1)\n);\n";
        let air = block_mesh(dict, false).unwrap();
        assert!(air.contains("hex (0 1 2 3 4 5 6 7) (5 3 3) simpleGrading (1 1 1)"), "{air}");
        let solid = block_mesh(dict, true).unwrap();
        assert!(solid.contains("(5 3 10) simpleGrading (1 ((0.5 0.5 2) (0.5 0.5 0.5)) 1)"), "{solid}");
    }

    #[test]
    fn probes_write_every_step_only_inside_their_block() {
        let text = "\nendTime 86400;\nwriteFormat ascii;\nwriteInterval 10;\nprobes\n{\n    writeInterval 10;\n}\nother\n{\n    writeInterval 10;\n}\n";
        let out = control_dict(text).unwrap();
        assert!(
            out.contains("\nendTime 7200;\nmaxFluidIteration 30;\nwriteFormat ascii;\nwriteInterval 10;\n"),
            "{out}"
        );
        assert!(out.contains("probes\n{\n    writeInterval 1;\n}\nother\n{\n    writeInterval 10;"), "{out}");
    }

    #[test]
    fn a_day_is_rotated_and_several_are_cut() {
        let day = "x\n(\n(0 1)\n(39600 2)\n(43200\t3)\n(86400 1)\n);\n";
        assert_eq!(shift_table(day, "t").unwrap(), "x\n(\n( 0 2 )\n( 3600 3 )\n( 46800 1 )\n( 86400 2 )\n);\n");
        let days = "(\n(0 1)\n(39600 2)\n(172800 3)\n)";
        assert_eq!(shift_table(days, "t").unwrap(), "(\n( 0 2 )\n( 133200 3 )\n)");
        assert!(shift_table("(\n(0 1)\n(3600 2)\n)", "t").is_err());
    }

    #[test]
    fn scripts_stop_at_the_first_error_and_run_serially() {
        let allrun = "#!/bin/sh\ncd ${0%/*} || exit 1\nNPROCS=4\ndecomposePar -allRegions\nrunParallel -np $NPROCS urbanMicroclimateFoam\ncd x\n";
        let out = stop_on_error(serial(allrun).unwrap().as_bytes());
        assert_eq!(out, b"#!/bin/sh\nset -e\ncd ${0%/*} || exit 1\nrunApplication urbanMicroclimateFoam\ncd x\n");
        assert_eq!(stop_on_error(b"#!/bin/sh\necho\n"), b"#!/bin/sh\necho\n");
    }

    #[test]
    fn line_edits_keep_every_other_byte() {
        // CRLF and a Latin-1 byte (\xb0) stay: only the relaxation factors change.
        let fv = b"relaxationFactors\r\n{\r\n    \"(h|e)\"  0.9;\r\n    U       0.7; // \xb0C\r\n    p_rgh   0.5;\r\n}";
        let out = fv_solution(fv).unwrap();
        assert_eq!(
            out,
            b"relaxationFactors\r\n{\r\n    \"(h|e)\"  0.5;\r\n    U       0.5; // \xb0C\r\n    p_rgh   0.3;\r\n}"
        );
    }

    #[test]
    fn a_probes_line_with_its_brace_still_opens_the_range() {
        // As sed's /probes/,/\}/: the end is looked for from the next line on.
        let out = control_dict("\nendTime 1;\nmaxFluidIteration 30;\nprobes { writeInterval 10; }\nwriteInterval 10;\n}\nwriteInterval 10;\n").unwrap();
        assert!(out.ends_with("probes { writeInterval 1; }\nwriteInterval 1;\n}\nwriteInterval 10;\n"), "{out}");
    }

    #[test]
    fn equal_times_sort_by_their_values() {
        let day = "(\n(0 1)\n(39600 b)\n(39600 a)\n(86400 1)\n)";
        assert_eq!(shift_table(day, "t").unwrap(), "(\n( 0 a )\n( 0 b )\n( 46800 1 )\n( 86400 a )\n)");
    }
}
