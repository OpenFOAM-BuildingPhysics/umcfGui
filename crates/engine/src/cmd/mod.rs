//! The command registry.

use crate::CommandSpec;

mod case;
mod edit;
mod geometry;
mod mesh;

/// Every command, in menu order.
pub static COMMANDS: &[CommandSpec] = &[
    case::OPEN,
    case::SAVE,
    case::INFO,
    case::CHECK,
    geometry::IMPORT,
    geometry::CHECK,
    geometry::REPAIR,
    geometry::SCALE,
    mesh::CHECK,
    edit::CONTROL_SET,
    edit::DICT_SET,
    edit::DICT_REMOVE,
    edit::UNDO,
    edit::REDO,
];
