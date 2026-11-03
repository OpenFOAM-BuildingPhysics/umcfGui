//! What urbanMicroclimateFoam reads, per OpenFOAM version.
//!
//! [`layout::detect`] works out which OpenFOAM version a case is written for and which regions
//! it has. The solver's catalog of run-time-selectable types will live here too
//! (`cargo xtask catalog`).
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod layout;

pub use layout::{Layout, Note, Regions, Severity, detect};
pub use umcf_core::OfVersion;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not an OpenFOAM case: there is no system/controlDict")]
    NotACase,
    #[error(transparent)]
    Fs(#[from] umcf_core::FsError),
    #[error("{path}: {source}")]
    Dict { path: String, source: umcf_foam_dict::Error },
}
