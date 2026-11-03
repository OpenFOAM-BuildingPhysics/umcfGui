//! Read a case's scripts into the workflow pattern and print the scripts generated from it.
//!
//! `cargo run -p umcf-model --example workflow -- <case> [--write]`: with `--write`, the
//! generated `Allprepare`, `Allrun` and `Allclean` replace the case's. Each replaced script is
//! first copied to the first free `<name>.bak`, `<name>.1.bak`, `<name>.2.bak`, … (never
//! overwritten), then the new text is written atomically (a temporary file renamed over the
//! script). A development tool until the CLI has this.

use umcf_core::{CaseFs, CaseWrite, DirFs};
use umcf_model::{Case, OpenOptions};

/// The most backups of one script kept side by side.
const MAX_BACKUPS: usize = 1000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("usage: workflow <case> [--write]");
        std::process::exit(2);
    };
    let write = args.iter().any(|a| a == "--write");
    let mut fs = DirFs::new(dir);
    let case = Case::open_with(&fs, OpenOptions { field_bytes: 0, ..Default::default() })?;
    let Some(version) = case.layout.version else {
        eprintln!("{dir}: the OpenFOAM version can't be told");
        std::process::exit(1);
    };
    let wf = case.workflow();
    for u in &wf.unmatched {
        eprintln!("{} {}:{}: {}", if u.redundant { "not needed" } else { "doesn't fit" }, u.script, u.line, u.what);
    }
    let scripts = wf.generate(version)?;
    for (name, text) in scripts {
        if !write {
            println!("==== {name}\n{text}");
            continue;
        }
        if let Some(old) = fs.read(name, u64::MAX)? {
            let backup = (0..MAX_BACKUPS)
                .map(|k| if k == 0 { format!("{name}.bak") } else { format!("{name}.{k}.bak") })
                .find(|b| !fs.is_file(b) && !fs.is_dir(b))
                .ok_or_else(|| format!("{name}: {MAX_BACKUPS} backups already, remove some"))?;
            fs.write(&backup, &old)?;
            eprintln!("kept {dir}/{backup}");
        }
        fs.write(name, text.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(fs.root().join(name), std::fs::Permissions::from_mode(0o755))?;
        }
        eprintln!("wrote {dir}/{name}");
    }
    Ok(())
}
