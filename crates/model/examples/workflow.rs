//! Read a case's scripts into the workflow pattern and print the scripts generated from it.
//!
//! `cargo run -p umcf-model --example workflow -- <case> [--write]`: with `--write`, the
//! generated `Allprepare`, `Allrun` and `Allclean` replace the case's (the old ones are kept
//! as `<name>.bak`). A development tool until the CLI has this.

use umcf_core::DirFs;
use umcf_model::{Case, OpenOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("usage: workflow <case> [--write]");
        std::process::exit(2);
    };
    let write = args.iter().any(|a| a == "--write");
    let case = Case::open_with(&DirFs::new(dir), OpenOptions { field_bytes: 0, ..Default::default() })?;
    let Some(version) = case.layout.version else {
        eprintln!("{dir}: the OpenFOAM version can't be told");
        std::process::exit(1);
    };
    let wf = case.workflow();
    for u in &wf.unmatched {
        eprintln!("{} {}:{}: {}", if u.redundant { "not needed" } else { "doesn't fit" }, u.script, u.line, u.what);
    }
    for (name, text) in wf.generate(version) {
        if write {
            let path = format!("{dir}/{name}");
            if std::path::Path::new(&path).exists() {
                std::fs::rename(&path, format!("{path}.bak"))?;
            }
            std::fs::write(&path, &text)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
            eprintln!("wrote {path}");
        } else {
            println!("==== {name}\n{text}");
        }
    }
    Ok(())
}
