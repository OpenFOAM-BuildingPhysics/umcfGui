//! Workspace tooling: `cargo xtask <command>`.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod catalog;
mod examples;
mod layers;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  ci        fmt --check, clippy -D warnings (default and --no-default-features), tests, layers,
            assets, wasm, deny (stops at the first failure)
  layers    enforce the crate dependency layering (docs/architecture.md)
  assets    check that every file under assets/ and docs/images/ has a row in ATTRIBUTION.md
  wasm      cargo check --target wasm32-unknown-unknown for the L0-L3 crates
  catalog [--check]
            scan solver/ (pinned commit and of-org_v8.0) for solver types and keywords; write
            crates/schema/src/catalog/generated.rs and docs/solver-coverage.md (--check: fail if
            they are out of date; solver/ must be checked out: git submodule update --init)
  docs      API reference with warnings as errors, every docs/ page in the zensical.toml nav, and
            the site in strict mode (skipped with a notice if Zensical is not installed)
  deny      cargo deny check licenses (skipped with a notice if cargo-deny is not installed)
  examples [<filter>]
            run every example case (examples/of8, examples/of12) with OpenFOAM and
            urbanMicroclimateFoam, each on a temporary copy; <filter> picks cases by path. The
            environments are UMCF_OF8_BASHRC and UMCF_OF12_BASHRC, else ~/OpenFOAM-<v>/etc/bashrc
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("ci") => cmd_ci(),
        Some("layers") => cmd_layers(),
        Some("assets") => cmd_assets(),
        Some("wasm") => cmd_wasm(),
        Some("docs") => cmd_docs(),
        Some("catalog") => match args.get(1).map(String::as_str) {
            None => cmd_catalog(false),
            Some("--check") if args.len() == 2 => cmd_catalog(true),
            Some(_) => {
                let bad = args.iter().skip(1).find(|a| *a != "--check").map_or("--check", String::as_str);
                Err(format!("catalog: unexpected argument `{bad}` (usage: cargo xtask catalog [--check])"))
            }
        },
        Some("deny") => cmd_deny(),
        Some("examples") => match args.get(1..).unwrap_or_default() {
            [] => examples::cmd_examples(None),
            [f] => examples::cmd_examples(Some(f)),
            _ => Err("examples: one filter at most (usage: cargo xtask examples [<filter>])".into()),
        },
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_path_buf()
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(step: &str, mut cmd: Command) -> Result<(), String> {
    println!("==> {step}");
    let status = cmd.status().map_err(|e| format!("{step}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{step} failed ({status})")) }
}

fn cmd_ci() -> Result<(), String> {
    let mut c = cargo();
    c.args(["fmt", "--all", "--check"]);
    run("fmt", c)?;
    for extra in [&[][..], &["--no-default-features"][..]] {
        let mut c = cargo();
        c.args(["clippy", "--workspace", "--all-targets"]).args(extra).args(["--", "-D", "warnings"]);
        run(&format!("clippy {}", extra.join(" ")), c)?;
    }
    let mut c = cargo();
    c.args(["test", "--workspace"]);
    run("test", c)?;
    let mut c = cargo();
    c.args(["test", "--workspace", "--no-default-features"]);
    run("test --no-default-features", c)?;
    cmd_layers()?;
    cmd_catalog(true)?;
    cmd_assets()?;
    cmd_wasm()?;
    cmd_docs()?;
    cmd_deny()?;
    println!("ci: all gates passed");
    Ok(())
}

fn metadata() -> Result<serde_json::Value, String> {
    let out = cargo()
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))
}

fn cmd_layers() -> Result<(), String> {
    println!("==> layers");
    let crates = layers::from_metadata(&metadata()?)?;
    let violations = layers::check(&crates);
    if violations.is_empty() {
        println!("layers: {} crates OK", crates.len());
        return Ok(());
    }
    for v in &violations {
        eprintln!("  {v}");
    }
    Err(format!("layers: {} violation(s)", violations.len()))
}

fn cmd_wasm() -> Result<(), String> {
    let crates = layers::from_metadata(&metadata()?)?;
    let pkgs: Vec<&str> = crates
        .iter()
        .filter(|c| matches!(layers::classify(&c.name), Some(layers::Class::Layer(l)) if l <= layers::WASM_MAX_LAYER))
        .map(|c| c.name.as_str())
        .collect();
    if pkgs.is_empty() {
        println!("==> wasm: no L0-L{} crates yet", layers::WASM_MAX_LAYER);
        return Ok(());
    }
    let mut c = cargo();
    c.args(["check", "--target", "wasm32-unknown-unknown"]);
    for p in &pkgs {
        c.args(["-p", p]);
    }
    run(&format!("wasm ({})", pkgs.join(", ")), c)
}

fn cmd_catalog(check: bool) -> Result<(), String> {
    println!("==> catalog{}", if check { " --check" } else { "" });
    let root = root();
    let solver = root.join("solver");
    // The submodule is part of the repository: without it there is nothing to check, and a
    // skipped check would let `cargo xtask ci` pass locally and fail on CI.
    if !solver.join("urbanMicroclimateFoam.C").is_file() {
        return Err("catalog: solver/ is not checked out: run `git submodule update --init`".into());
    }
    let pat = catalog::Patterns::new()?;
    let v12 = catalog::scan(&pat, &catalog::load(&solver, "HEAD")?)?;
    let v8_files = catalog::load(&solver, catalog::V8_REV)
        .map_err(|e| format!("{e}\n  (is the tag missing? git -C solver fetch --tags)"))?;
    let v8 = catalog::scan(&pat, &v8_files)?;
    let mut merged = catalog::merge(vec![(catalog::Ver::V8, v8), (catalog::Ver::V12, v12)]);
    merged.solver_commit = catalog::git(&solver, &["rev-parse", "HEAD"])?.trim().to_string();
    merged.v8_commit =
        catalog::git(&solver, &["rev-parse", &format!("{}^{{commit}}", catalog::V8_REV)])?.trim().to_string();
    let outputs =
        [(catalog::GENERATED_RS, catalog::render_rs(&merged)), (catalog::COVERAGE_MD, catalog::render_md(&merged))];
    for (path, content) in outputs {
        let full = root.join(path);
        let current = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{path}: {e}")),
        };
        if check {
            if current != content {
                return Err(format!("catalog: {path} does not match solver/: run `cargo xtask catalog`"));
            }
        } else if current != content {
            if let Some(dir) = full.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&full, content).map_err(|e| format!("{path}: {e}"))?;
            println!("catalog: wrote {path}");
        }
    }
    println!("catalog: up to date with solver/ at {}", merged.solver_commit.get(..7).unwrap_or(""));
    Ok(())
}

fn cmd_docs() -> Result<(), String> {
    let mut c = cargo();
    c.args(["doc", "--workspace", "--no-deps", "--quiet"]).env("RUSTDOCFLAGS", "-D warnings");
    run("docs: API reference", c)?;

    println!("==> docs: nav");
    let root = root();
    let config = std::fs::read_to_string(root.join("zensical.toml")).map_err(|e| format!("zensical.toml: {e}"))?;
    let mut pages = Vec::new();
    walk(&root, &root.join("docs"), &mut pages)?;
    let missing: Vec<&String> = pages
        .iter()
        .filter(|p| p.ends_with(".md"))
        .filter(|p| !config.contains(&format!("\"{}\"", p.trim_start_matches("docs/"))))
        .collect();
    if !missing.is_empty() {
        for m in &missing {
            eprintln!("  not in the zensical.toml nav: {m}");
        }
        return Err(format!("docs: {} page(s) missing from the nav", missing.len()));
    }

    let venv = root.join(".venv/bin/zensical");
    let zensical = if venv.is_file() {
        Some(venv.into_os_string())
    } else if Command::new("zensical").arg("--version").output().is_ok_and(|o| o.status.success()) {
        Some("zensical".into())
    } else {
        None
    };
    if zensical.is_none() && strict() {
        return Err("docs: Zensical is not installed (required on CI)".into());
    }
    let Some(zensical) = zensical else {
        println!("==> docs: site SKIPPED (Zensical not installed: see docs/contributing.md)");
        return Ok(());
    };
    let mut c = Command::new(zensical);
    c.current_dir(&root).args(["build", "--strict", "--clean"]);
    run("docs: site (strict)", c)
}

/// On CI (`CI` is set, as GitHub Actions does) a missing tool fails the gate instead of skipping it.
fn strict() -> bool {
    std::env::var_os("CI").is_some_and(|v| !v.is_empty() && v != "false")
}

fn cmd_deny() -> Result<(), String> {
    let installed = cargo().args(["deny", "--version"]).output().is_ok_and(|o| o.status.success());
    if !installed && strict() {
        return Err("deny: cargo-deny is not installed (required on CI)".into());
    }
    if !installed {
        println!("==> deny: SKIPPED (cargo-deny not installed: `cargo install cargo-deny`)");
        return Ok(());
    }
    let mut c = cargo();
    c.args(["deny", "check", "licenses"]);
    run("deny", c)
}

fn cmd_assets() -> Result<(), String> {
    println!("==> assets");
    let root = root();
    let attribution =
        std::fs::read_to_string(root.join("ATTRIBUTION.md")).map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let patterns: Vec<String> = attribution
        .lines()
        .filter_map(|l| l.strip_prefix('|'))
        .filter_map(|l| l.split('|').next())
        .map(|c| c.trim().trim_matches('`').to_string())
        .filter(|c| !c.is_empty() && c != "Path" && !c.starts_with("---"))
        .collect();
    let mut files = Vec::new();
    for dir in ["assets", "docs/images"] {
        walk(&root, &root.join(dir), &mut files)?;
    }
    let missing: Vec<&String> = files.iter().filter(|f| !patterns.iter().any(|p| glob_match(p, f))).collect();
    if missing.is_empty() {
        println!("assets: {} file(s) attributed", files.len());
        return Ok(());
    }
    for m in &missing {
        eprintln!("  no ATTRIBUTION.md row: {m}");
    }
    Err(format!("assets: {} unattributed file(s)", missing.len()))
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for e in entries {
        let p = e.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if p.is_dir() {
            walk(root, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// `*` matches any run of characters (including `/`); everything else matches literally.
fn glob_match(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else { return pattern == text };
    let Some(mut rest) = text.strip_prefix(first) else { return false };
    let parts: Vec<&str> = parts.collect();
    for (i, part) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(pos) => rest = rest.get(pos + part.len()..).unwrap_or(""),
            None => return false,
        }
    }
    rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn globs() {
        assert!(glob_match("assets/icons/*.svg", "assets/icons/run.svg"));
        assert!(glob_match("assets/*", "assets/a/b.png"));
        assert!(glob_match("docs/images/x.png", "docs/images/x.png"));
        assert!(!glob_match("assets/*.svg", "assets/a.png"));
        assert!(!glob_match("docs/images/x.png", "docs/images/y.png"));
    }
}
