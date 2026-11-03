//! Shared basics for every umcfGui crate: case file access ([`CaseFs`]) and the OpenFOAM
//! versions umcfGui supports ([`OfVersion`]).
//!
//! Case access goes through [`CaseFs`] so that layers above work on a directory ([`DirFs`]),
//! in memory ([`MemFs`], for tests and the web viewer) or anything else, and stay wasm-clean.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// The OpenFOAM-org versions urbanMicroclimateFoam is ported to and umcfGui supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OfVersion {
    V8,
    V12,
}

impl OfVersion {
    pub const ALL: [OfVersion; 2] = [OfVersion::V8, OfVersion::V12];

    pub fn label(self) -> &'static str {
        match self {
            OfVersion::V8 => "OpenFOAM-8",
            OfVersion::V12 => "OpenFOAM-12",
        }
    }
}

impl std::fmt::Display for OfVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path} is {size} bytes, more than the {limit}-byte limit")]
    TooLarge { path: String, size: u64, limit: u64 },
    #[error("`{0}` is not a path inside the case")]
    BadPath(String),
}

/// Read access to a case. Paths are relative to the case root and use `/`
/// (`system/controlDict`, `constant/air/polyMesh/boundary`).
pub trait CaseFs {
    /// The file's bytes, `Ok(None)` if there is no such file. Files bigger than `limit` are an
    /// error, so a stray 10 GB file can't exhaust memory.
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError>;
    fn is_file(&self, rel: &str) -> bool;
    fn is_dir(&self, rel: &str) -> bool;
    /// Names of the entries directly inside `rel` (files and directories), sorted. Empty if it
    /// is not a directory.
    fn list(&self, rel: &str) -> Vec<String>;
}

/// Rejects absolute paths and `..`, so a path taken from a dictionary can't leave the case.
fn check_rel(rel: &str) -> Result<(), FsError> {
    let ok = Path::new(rel).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if ok { Ok(()) } else { Err(FsError::BadPath(rel.to_string())) }
}

/// A case directory on disk.
#[derive(Debug, Clone)]
pub struct DirFs {
    root: PathBuf,
}

impl DirFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        DirFs { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, rel: &str) -> Option<PathBuf> {
        check_rel(rel).ok().map(|()| self.root.join(rel))
    }
}

impl CaseFs for DirFs {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        let path = self.path(rel).ok_or_else(|| FsError::BadPath(rel.to_string()))?;
        let meta = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(FsError::Io { path: rel.to_string(), source }),
        };
        if meta.len() > limit {
            return Err(FsError::TooLarge { path: rel.to_string(), size: meta.len(), limit });
        }
        std::fs::read(&path).map(Some).map_err(|source| FsError::Io { path: rel.to_string(), source })
    }

    fn is_file(&self, rel: &str) -> bool {
        self.path(rel).is_some_and(|p| p.is_file())
    }

    fn is_dir(&self, rel: &str) -> bool {
        self.path(rel).is_some_and(|p| p.is_dir())
    }

    fn list(&self, rel: &str) -> Vec<String> {
        let Some(path) = self.path(rel) else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(path) else { return Vec::new() };
        let mut names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }
}

/// A case held in memory: `path → bytes`. Directories exist implicitly.
#[derive(Debug, Clone, Default)]
pub struct MemFs {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemFs {
    pub fn new() -> Self {
        MemFs::default()
    }

    /// Adds a file (builder style).
    pub fn with(mut self, rel: &str, contents: impl Into<Vec<u8>>) -> Self {
        self.insert(rel, contents);
        self
    }

    pub fn insert(&mut self, rel: &str, contents: impl Into<Vec<u8>>) {
        self.files.insert(rel.trim_matches('/').to_string(), contents.into());
    }
}

impl CaseFs for MemFs {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        check_rel(rel)?;
        match self.files.get(rel) {
            Some(b) if b.len() as u64 > limit => {
                Err(FsError::TooLarge { path: rel.to_string(), size: b.len() as u64, limit })
            }
            other => Ok(other.cloned()),
        }
    }

    fn is_file(&self, rel: &str) -> bool {
        self.files.contains_key(rel)
    }

    fn is_dir(&self, rel: &str) -> bool {
        let prefix = format!("{}/", rel.trim_matches('/'));
        rel.is_empty() || self.files.keys().any(|k| k.starts_with(&prefix))
    }

    fn list(&self, rel: &str) -> Vec<String> {
        let rel = rel.trim_matches('/');
        let prefix = if rel.is_empty() { String::new() } else { format!("{rel}/") };
        let mut names: Vec<String> = self
            .files
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter_map(|rest| rest.split('/').next())
            .map(str::to_string)
            .collect();
        names.dedup();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memfs_files_and_dirs() {
        let fs = MemFs::new().with("system/controlDict", "a 1;").with("system/air/fvSchemes", "").with("0/air/U", "");
        assert!(fs.is_file("system/controlDict"));
        assert!(fs.is_dir("system") && fs.is_dir("system/air") && !fs.is_dir("constant"));
        assert_eq!(fs.list(""), ["0", "system"]);
        assert_eq!(fs.list("system"), ["air", "controlDict"]);
        assert_eq!(fs.read("system/controlDict", 100).unwrap().as_deref(), Some(&b"a 1;"[..]));
        assert!(fs.read("system/nope", 100).unwrap().is_none());
        assert!(matches!(fs.read("system/controlDict", 2), Err(FsError::TooLarge { .. })));
    }

    #[test]
    fn paths_cannot_leave_the_case() {
        let fs = MemFs::new();
        assert!(matches!(fs.read("../etc/passwd", 100), Err(FsError::BadPath(_))));
        assert!(matches!(fs.read("/etc/passwd", 100), Err(FsError::BadPath(_))));
        let d = DirFs::new("/tmp");
        assert!(matches!(d.read("../etc/passwd", 100), Err(FsError::BadPath(_))));
        assert!(!d.is_file("../etc/passwd"));
        assert!(d.list("..").is_empty());
    }

    #[test]
    fn dirfs_reads_missing_as_none() {
        let d = DirFs::new(env!("CARGO_MANIFEST_DIR"));
        assert!(d.is_file("Cargo.toml"));
        assert!(d.read("no-such-file", 10).unwrap().is_none());
        assert!(d.read("src", 10).unwrap().is_none());
        assert!(d.list("").contains(&"src".to_string()));
        assert!(matches!(d.read("Cargo.toml", 3), Err(FsError::TooLarge { .. })));
    }
}
