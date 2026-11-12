//! The command registry.

use crate::CommandSpec;

mod bc;
mod case;
pub(crate) mod display;
mod edit;
mod geometry;
mod material;
mod mesh;
pub(crate) mod mesh_block;
pub(crate) mod mesh_box;
mod patch;
mod region;
pub(crate) mod scene;
pub(crate) mod workflow;

/// Every command, in menu order.
pub static COMMANDS: &[CommandSpec] = &[
    case::NEW,
    case::OPEN,
    case::FIELDS,
    case::SAVE,
    case::INFO,
    region::ADD,
    material::SET,
    workflow::SHOW,
    workflow::IMPORT,
    workflow::REGION,
    workflow::PATCH,
    workflow::MATERIAL,
    workflow::SETTINGS,
    workflow::COMPONENT,
    workflow::SCRIPTS,
    case::CHECK,
    geometry::IMPORT,
    geometry::TREES,
    geometry::CHECK,
    geometry::REPAIR,
    geometry::SCALE,
    mesh::CHECK,
    mesh_block::BLOCK,
    mesh_box::BOX,
    scene::SNAPSHOT,
    scene::PICK,
    scene::RECT,
    scene::BRUSH,
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
