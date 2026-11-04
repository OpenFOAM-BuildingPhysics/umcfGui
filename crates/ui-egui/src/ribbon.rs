//! The workspaces and their ribbons: which commands go in which tab and group, with their
//! icons. Commands come from the registry; this table only places them. A command with a menu
//! path that the table doesn't place goes in a "More" group of the tab its menu names, so a new
//! command shows up without touching the interface.

use umcf_engine::COMMANDS;
use umcf_render::camera::View;

use crate::icons::Icon;

/// A workspace: a set of ribbon tabs over the same case and selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Setup,
    Run,
    Results,
}

impl Workspace {
    pub const ALL: [Workspace; 3] = [Workspace::Setup, Workspace::Run, Workspace::Results];

    pub fn name(self) -> &'static str {
        match self {
            Workspace::Setup => "Setup",
            Workspace::Run => "Run",
            Workspace::Results => "Results",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Workspace::Setup => Icon::Setup,
            Workspace::Run => Icon::Run,
            Workspace::Results => Icon::Results,
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Workspace::Setup => "Geometry, meshes, boundary conditions and run controls",
            Workspace::Run => "Scripts for the cluster, job files and local runs",
            Workspace::Results => "Probes, slices, time series and residuals",
        }
    }

    /// Whether it has commands yet.
    pub fn available(self) -> bool {
        self == Workspace::Setup
    }

    pub fn tabs(self) -> &'static [Tab] {
        match self {
            Workspace::Setup => SETUP,
            Workspace::Run | Workspace::Results => &[],
        }
    }
}

/// One entry of a group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    /// A command of the registry, by id, with the label shown under its icon.
    Command(&'static str, &'static str, Icon),
    /// A standard view of the 3D view.
    View(View, &'static str, Icon),
    /// Face edges on or off.
    Edges,
    /// Light or dark theme.
    Theme,
}

#[derive(Debug, Clone, Copy)]
pub struct Group {
    pub name: &'static str,
    pub items: &'static [Item],
}

#[derive(Debug, Clone, Copy)]
pub struct Tab {
    pub name: &'static str,
    /// The menu prefix of the registry this tab collects unplaced commands from.
    pub menu: &'static str,
    pub groups: &'static [Group],
    /// Said in the tab when it has little in it yet.
    pub note: Option<&'static str>,
}

const SETUP: &[Tab] = &[
    Tab {
        name: "Case",
        menu: "Case",
        groups: &[
            Group {
                name: "File",
                items: &[
                    Item::Command("case.open", "Open", Icon::Open),
                    Item::Command("case.save", "Save", Icon::Save),
                ],
            },
            Group {
                name: "Inspect",
                items: &[
                    Item::Command("case.info", "Summary", Icon::Info),
                    Item::Command("case.check", "Check", Icon::Check),
                ],
            },
            Group { name: "Configure", items: &[Item::Command("control.set", "Run controls", Icon::Controls)] },
        ],
        note: None,
    },
    Tab {
        name: "Geometry",
        menu: "Geometry",
        groups: &[
            Group {
                name: "Surfaces",
                items: &[
                    Item::Command("geometry.import", "Import STL", Icon::Import),
                    Item::Command("geometry.check", "Check", Icon::SurfaceCheck),
                ],
            },
            Group {
                name: "Modify",
                items: &[
                    Item::Command("geometry.repair", "Repair", Icon::Repair),
                    Item::Command("geometry.scale", "Scale", Icon::Scale),
                ],
            },
        ],
        note: None,
    },
    Tab {
        name: "Mesh",
        menu: "Mesh",
        groups: &[Group { name: "Quality", items: &[Item::Command("mesh.check", "Check mesh", Icon::MeshCheck)] }],
        note: Some("Meshing set-up (blockMesh, snappyHexMesh) comes here"),
    },
    Tab {
        name: "Boundary conditions",
        menu: "Boundary conditions",
        groups: &[
            Group { name: "Conditions", items: &[Item::Command("bc.set", "Set condition", Icon::Condition)] },
            Group { name: "Selection", items: &[Item::Command("select.clear", "Select none", Icon::SelectNone)] },
        ],
        note: Some("Pick patches in the view (Ctrl+click for more), then set their condition"),
    },
    Tab {
        name: "View",
        menu: "View",
        groups: &[
            Group {
                name: "Views",
                items: &[
                    Item::View(View::Iso, "Iso", Icon::ViewIso),
                    Item::View(View::Top, "Top", Icon::ViewTop),
                    Item::View(View::Front, "Front", Icon::ViewFront),
                    Item::View(View::Right, "Right", Icon::ViewRight),
                ],
            },
            Group { name: "Display", items: &[Item::Edges, Item::Theme] },
            Group { name: "Export", items: &[Item::Command("scene.snapshot", "Snapshot", Icon::Snapshot)] },
        ],
        note: None,
    },
];

/// Every command a tab shows: those its groups place, then the registry's other commands whose
/// menu is the tab's (`Geometry/…`), in a group "More".
pub fn groups(tab: &Tab) -> Vec<(&'static str, Vec<Item>)> {
    let mut out: Vec<(&'static str, Vec<Item>)> = tab.groups.iter().map(|g| (g.name, g.items.to_vec())).collect();
    let placed = |id: &str| {
        SETUP
            .iter()
            .flat_map(|t| t.groups)
            .flat_map(|g| g.items)
            .any(|i| matches!(i, Item::Command(c, _, _) if *c == id))
    };
    let more: Vec<Item> = COMMANDS
        .iter()
        .filter(|c| c.menu.split_once('/').is_some_and(|(m, _)| m == tab.menu) && !placed(c.id))
        .map(|c| Item::Command(c.id, c.label, Icon::Command))
        .collect();
    if !more.is_empty() {
        out.push(("More", more));
    }
    out
}

/// The tab of a workspace whose groups place a command.
pub fn tab_of(workspace: Workspace, id: &str) -> Option<usize> {
    workspace
        .tabs()
        .iter()
        .position(|t| t.groups.iter().flat_map(|g| g.items).any(|i| matches!(i, Item::Command(c, _, _) if *c == id)))
}

/// The icon of a command wherever it is shown (history chips, search results).
pub fn icon_of(id: &str) -> Icon {
    SETUP
        .iter()
        .flat_map(|t| t.groups)
        .flat_map(|g| g.items)
        .find_map(|i| match i {
            Item::Command(c, _, icon) if *c == id => Some(*icon),
            _ => None,
        })
        .unwrap_or(match id {
            "edit.undo" => Icon::Undo,
            "edit.redo" => Icon::Redo,
            "dict.set" | "dict.remove" => Icon::Controls,
            "select.pick" | "select.set" => Icon::Patch,
            _ => Icon::Command,
        })
}
