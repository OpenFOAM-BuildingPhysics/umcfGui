//! The command registry.

use crate::CommandSpec;

mod bc;
mod case;
pub(crate) mod display;
mod edit;
mod geometry;
mod mesh;
mod patch;
pub(crate) mod scene;

/// Every command, in menu order.
pub static COMMANDS: &[CommandSpec] = &[
    case::OPEN,
    case::SAVE,
    case::INFO,
    case::CHECK,
    geometry::IMPORT,
    geometry::TREES,
    geometry::CHECK,
    geometry::REPAIR,
    geometry::SCALE,
    mesh::CHECK,
    scene::SNAPSHOT,
    scene::PICK,
    scene::RECT,
    scene::SELECT,
    scene::CLEAR,
    bc::SET,
    patch::SET,
    patch::CREATE,
    edit::CONTROL_SET,
    edit::DICT_SET,
    edit::DICT_REMOVE,
    edit::UNDO,
    edit::REDO,
];
