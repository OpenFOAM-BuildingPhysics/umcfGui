//! `cargo xtask examples`: runs every example case (`examples/of8`, `examples/of12`) with
//! OpenFOAM and urbanMicroclimateFoam, each on a copy in a temporary directory: `Allprepare`
//! then `Allrun`, as `sh`. A case passes when both scripts succeed within the time limit, the
//! solver's log ends with `End`, it wrote the case's `endTime` and no log has a fatal error.
//!
//! The OpenFOAM environment of each version is `UMCF_OF8_BASHRC` / `UMCF_OF12_BASHRC`, else
//! `$HOME/OpenFOAM-8/etc/bashrc` / `$HOME/OpenFOAM-12/etc/bashrc`; a version without one is
//! skipped with a notice.

// Native-only tooling: the clock and sleeping are fine (the lint guards wasm builds).
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::root;

/// What each case's runner gives back: its place in the jobs, seconds, the outcome.
type Outcome = (usize, f64, Result<(), String>);

/// How long one case may take: they run in seconds, so this only stops a hang.
const LIMIT: Duration = Duration::from_secs(600);

/// The OpenFOAM environment for `version` ("8" or "12"): `Ok(None)` when there is none.
fn bashrc(version: &str) -> Result<Option<PathBuf>, String> {
    let var = format!("UMCF_OF{version}_BASHRC");
    match std::env::var(&var) {
        Ok(p) if !p.is_empty() => {
            let p = PathBuf::from(p);
            return if p.is_file() { Ok(Some(p)) } else { Err(format!("{var}: {} is not a file", p.display())) };
        }
        _ => {}
    }
    let Some(home) = std::env::var_os("HOME") else { return Ok(None) };
    let p = Path::new(&home).join(format!("OpenFOAM-{version}/etc/bashrc"));
    Ok(p.is_file().then_some(p))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// The value of `key` in controlDict text, without a trailing comment: `endTime 7200;` gives
/// "7200".
fn control(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let rest = l.trim_start().strip_prefix(key)?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let value = rest.split("//").next()?.trim().strip_suffix(';')?.trim();
        Some(value.to_string())
    })
}

/// Whether `dir` has a time directory at `time` (by value: `7200`, `7.2e3`).
fn has_time(dir: &Path, time: &str) -> bool {
    let Ok(want) = time.parse::<f64>() else { return dir.join(time).is_dir() };
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    entries.flatten().any(|e| {
        e.path().is_dir() && e.file_name().to_str().and_then(|n| n.parse::<f64>().ok()).is_some_and(|t| t == want)
    })
}

/// Which of `needles` (non-empty) the file at `path` contains, read as a stream in fixed-size
/// chunks: a solver's log can be gigabytes. A match across two chunks is found: the last bytes
/// of one are kept for the next.
fn scan(path: &Path, needles: &[&[u8]]) -> std::io::Result<Vec<bool>> {
    use std::io::Read;
    let keep = needles.iter().map(|n| n.len()).max().unwrap_or(1).saturating_sub(1);
    let mut found = vec![false; needles.len()];
    let mut file = std::fs::File::open(path)?;
    let mut window: Vec<u8> = Vec::with_capacity(keep + (64 << 10));
    let mut chunk = vec![0u8; 64 << 10];
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            return Ok(found);
        }
        window.extend_from_slice(chunk.get(..n).unwrap_or_default());
        for (f, needle) in found.iter_mut().zip(needles) {
            *f = *f || (!needle.is_empty() && window.windows(needle.len()).any(|w| w == *needle));
        }
        let cut = window.len().saturating_sub(keep);
        window.drain(..cut);
    }
}

/// Runs the copy at `dir` with `env`; what went wrong, if anything.
fn run_case(env: &Path, dir: &Path) -> Result<(), String> {
    let script = r#"
. "$1" > /dev/null 2>&1
command -v urbanMicroclimateFoam > /dev/null || { echo "urbanMicroclimateFoam is not on the PATH after sourcing $1"; exit 3; }
cd "$2" || exit 3
if [ -f Allprepare ]; then sh ./Allprepare > log.Allprepare 2>&1 || { echo "Allprepare failed: see log.Allprepare"; exit 1; }; fi
sh ./Allrun > log.Allrun 2>&1 || { echo "Allrun failed: see log.Allrun"; exit 1; }
"#;
    let out_path = dir.join("log.examples");
    let out = std::fs::File::create(&out_path).map_err(|e| format!("{}: {e}", out_path.display()))?;
    let mut command = Command::new("bash");
    // Its own process group, so that a timeout stops the solver under it too. (Ctrl-C then
    // stops xtask but not a case already running: the copy says which.)
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .args(["-c", script, "examples"])
        .arg(env)
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("bash: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| format!("bash: {e}"))? {
            Some(s) => break s,
            None if started.elapsed() > LIMIT => {
                // The whole group: bash, the scripts, the solver. The copy is kept to look at.
                #[cfg(unix)]
                let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("still running after {} s: stopped", LIMIT.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    if !status.success() {
        let said = std::fs::read_to_string(&out_path).unwrap_or_default();
        return Err(if said.trim().is_empty() { format!("failed ({status})") } else { said.trim().to_string() });
    }
    let mut fatal = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        // An entry that can't be read is an error: a skipped log could hide a fatal one.
        let e = e.map_err(|err| format!("{}: {err}", dir.display()))?;
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("log.") {
            let hits = scan(&e.path(), &[b"FOAM FATAL", b"Floating point exception"])
                .map_err(|err| format!("{name}: {err}"))?;
            if hits.contains(&true) {
                fatal.push(name);
            }
        }
    }
    if !fatal.is_empty() {
        return Err(format!("fatal error in {}", fatal.join(", ")));
    }
    // OpenFOAM ends its log with a line `End`.
    let end_line = scan(&dir.join("log.urbanMicroclimateFoam"), &[b"\nEnd\n"])
        .map_err(|e| format!("log.urbanMicroclimateFoam: {e}"))?;
    if !end_line.contains(&true) {
        return Err("the solver did not reach `End`: see log.urbanMicroclimateFoam".into());
    }
    let control_dict = std::fs::read_to_string(dir.join("system/controlDict")).map_err(|e| e.to_string())?;
    let end = control(&control_dict, "endTime").ok_or("system/controlDict: no endTime")?;
    if !has_time(dir, &end) {
        return Err(format!("no time directory {end} (endTime) was written"));
    }
    Ok(())
}

/// Whether `filter` picks the case `job` (`of12/streetCanyon_CFDHAM`): the whole name, a version
/// (`of12`), or a case name in either (`streetCanyon_CFDHAM`), never a part of a name.
fn picks(filter: &str, job: &str) -> bool {
    let f = filter.trim_end_matches('/');
    let (version, name) = job.split_once('/').unwrap_or(("", job));
    f == job || f == version || f == name
}

/// The cases to run: (name, environment, copy). Copies everything first.
fn prepare(base: &Path, filter: Option<&str>) -> Result<Vec<(String, PathBuf, PathBuf)>, String> {
    let mut jobs = Vec::new();
    for version in ["8", "12"] {
        let dir = root().join(format!("examples/of{version}"));
        if !dir.is_dir() {
            continue;
        }
        let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("system/controlDict").is_file())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                filter.is_none_or(|f| picks(f, &format!("of{version}/{name}")))
            })
            .collect();
        cases.sort();
        if cases.is_empty() {
            continue;
        }
        let Some(env) = bashrc(version)? else {
            println!("    OpenFOAM-{version}: SKIPPED (set UMCF_OF{version}_BASHRC to its etc/bashrc)");
            continue;
        };
        for case in cases {
            let name = case.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let copy = base.join(format!("of{version}")).join(&name);
            copy_dir(&case, &copy).map_err(|e| format!("copying {}: {e}", case.display()))?;
            jobs.push((format!("of{version}/{name}"), env.clone(), copy));
        }
    }
    Ok(jobs)
}

pub fn cmd_examples(filter: Option<&str>) -> Result<(), String> {
    println!("==> examples");
    // A directory of its own: made here, so that no earlier run's logs or times are in it.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let base = std::env::temp_dir().join(format!("umcf-examples-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    let jobs = match prepare(&base, filter) {
        Ok(j) if !j.is_empty() => j,
        other => {
            let _ = std::fs::remove_dir_all(&base);
            return Err(match other {
                Err(e) => e,
                Ok(_) => "no example ran: no OpenFOAM environment found (or no case matched)".into(),
            });
        }
    };
    // Each case runs serially on one core: as many at once as there are cores.
    let width = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = Mutex::new(0usize);
    let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..width.min(jobs.len()) {
            s.spawn(|| {
                loop {
                    let k = {
                        let Ok(mut n) = next.lock() else { return };
                        let k = *n;
                        *n += 1;
                        k
                    };
                    let Some((_, env, copy)) = jobs.get(k) else { return };
                    let started = Instant::now();
                    let r = run_case(env, copy);
                    if let Ok(mut out) = results.lock() {
                        out.push((k, started.elapsed().as_secs_f64(), r));
                    }
                }
            });
        }
    });
    let mut results = results.into_inner().map_err(|_| "a runner thread failed".to_string())?;
    results.sort_by_key(|r| r.0);
    let mut failed = 0;
    for (k, (name, _, copy)) in jobs.iter().enumerate() {
        match results.iter().find(|r| r.0 == k) {
            Some((_, secs, Ok(()))) => {
                println!("    ok   {name} ({secs:.0} s)");
                let _ = std::fs::remove_dir_all(copy);
            }
            Some((_, secs, Err(e))) => {
                failed += 1;
                println!("    FAIL {name} ({secs:.0} s): {e} (kept in {})", copy.display());
            }
            None => {
                failed += 1;
                println!("    FAIL {name}: not run (kept in {})", copy.display());
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} of {} examples failed", jobs.len()));
    }
    std::fs::remove_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    println!("examples: {} ran", jobs.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{picks, scan};

    #[test]
    fn scans_across_chunks() {
        let dir = std::env::temp_dir().join(format!("umcf-xtask-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log.x");
        // The needle straddles the 64 KiB boundary.
        let mut bytes = vec![b'.'; (64 << 10) - 4];
        bytes.extend_from_slice(b"FOAM FATAL\nEnd\n");
        std::fs::write(&path, &bytes).unwrap();
        let found = scan(&path, &[b"FOAM FATAL", b"\nEnd\n", b"missing"]).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, [true, true, false]);
    }

    #[test]
    fn filters_pick_names_not_parts() {
        assert!(picks("of12/streetCanyon_CFDHAM", "of12/streetCanyon_CFDHAM"));
        assert!(!picks("of12/streetCanyon_CFDHAM", "of12/streetCanyon_CFDHAM_veg"));
        assert!(picks("of12", "of12/streetCanyon_CFD"));
        assert!(picks("of12/", "of12/streetCanyon_CFD"));
        assert!(!picks("of1", "of12/streetCanyon_CFD"));
        assert!(picks("streetCanyon_CFD", "of8/streetCanyon_CFD"));
        assert!(!picks("streetCanyon", "of8/streetCanyon_CFD"));
    }
}
