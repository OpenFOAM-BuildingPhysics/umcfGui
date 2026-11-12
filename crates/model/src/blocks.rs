//! Block meshes in the workflow description: a region's `blocks` entry (gaps row 5), the grid of
//! blocks its `blockMeshDict` is written from, and the import of a `blockMeshDict` that is such
//! a grid.
//!
//! ```c++
//! blocks
//! {
//!     // Stations along each axis; between two stations: (cells grading), the grading a
//!     // number or blockMesh's sections ((fraction cells-fraction expansion) …).
//!     x       ( 0 (7 0.0074) 200 (10 ((0.5 0.5 13.9) (0.5 0.5 0.072))) 250 (5 1) 590 );
//!     y       ( 0 (6 1) 100 (5 1) 150 );
//!     z       ( 0 (3 1) 10 );
//!     missing ( (1 1 0) );                    // blocks (i j k) left out: the buildings
//!     patches { west inlet; east outlet; south side; north side; ground ground; top top;
//!               missing buildings; }          // each side's patch
//!     faces   { air_to_street ( (2 1 0 ground) ); }   // single block faces given another
//!     noFaces ( street );                     // patches declared without faces
//! }
//! ```
//!
//! `west`/`east` are x min/max, `south`/`north` y min/max, `ground`/`top` z min/max, `missing`
//! the faces around the missing blocks. The patch named `defaultFaces` is blockMesh's own: its
//! faces are left to `defaultPatch` (type `empty`), as the reference cases leave their
//! buildings' faces for a root `createPatch` to rename. A patch's type follows its role in the
//! region's `patches` ([`patch_type`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use umcf_foam_dict::{Dict, Document, Item, Node, unquote};

use crate::description::{RegionDesc, Role, atom, entry_name, list, single, twice, unescaped, word};
use crate::workflow::{MeshBuild, RootStep};
use crate::{Case, RegionKind};

/// The sides on the ground and on top: those across the axis the gravity is along (the region's
/// `constant/<region>/g`, else `constant/g`; z down without either), the ground the one it
/// points to.
/// The third value: whether a `g` was read (else z is taken as up).
pub fn ground_and_top(case: &Case, region: &str) -> (Side, Side, bool) {
    let g = [format!("constant/{region}/g"), "constant/g".to_string()].iter().find_map(|rel| {
        let e = case.dict(rel)?.get(&["value"])?;
        let [umcf_foam_dict::Node::List { items, .. }] = e.value.as_slice() else { return None };
        // Three items, or no gravity: counted before anything is read of them.
        if items.len() != 3 {
            return None;
        }
        // Exactly three numbers, all finite, one at least not 0: else no gravity is read.
        let v: Option<Vec<f64>> = items
            .iter()
            .map(|n| match n {
                umcf_foam_dict::Node::Atom(t) => t.text.parse::<f64>().ok().filter(|x| x.is_finite()),
                _ => None,
            })
            .collect();
        match v?.as_slice() {
            [x, y, z] if [x, y, z].iter().any(|c| **c != 0.0) => Some([*x, *y, *z]),
            _ => None,
        }
    });
    let read = g.is_some();
    let g = g.unwrap_or([0.0, 0.0, -1.0]);
    let axis = (0..3).max_by(|a, b| g[*a].abs().total_cmp(&g[*b].abs())).unwrap_or(2);
    let [low, high] = [[Side::West, Side::East], [Side::South, Side::North], [Side::Ground, Side::Top]][axis];
    if g[axis] < 0.0 { (low, high, read) } else { (high, low, read) }
}

/// The line every file umcfGui generates from the workflow carries.
pub const GENERATED: &str = "Written by umcfGui from the case workflow; edit the workflow, not this file.";

/// blockMesh's patch for the faces no patch lists.
pub const DEFAULT_FACES: &str = "defaultFaces";

/// The most blocks a grid may have (the generated dictionary lists each one).
pub const MAX_BLOCKS: usize = 20_000;
/// The most stations along one axis.
pub const MAX_STATIONS: usize = 10_000;
/// The most sections of one grading.
pub const MAX_SECTIONS: usize = 1_000;
/// The most vertices an imported dictionary may list.
const MAX_VERTICES: usize = 1_000_000;

const AXES: [&str; 3] = ["x", "y", "z"];

/// How the cells of a span are spread: one expansion ratio (last cell over first), or
/// blockMesh's sections `((fraction cells-fraction expansion) …)`.
#[derive(Debug, Clone, PartialEq)]
pub enum Grading {
    Ratio(f64),
    Sections(Vec<[f64; 3]>),
}

/// What lies between two stations: its number of cells and their grading.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub cells: u32,
    pub grading: Grading,
}

/// The stations along one axis and the spans between them (`spans.len() + 1 ==
/// stations.len()` in a grid read or imported; [`BlockGrid::problems`] says when not).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Axis {
    pub stations: Vec<f64>,
    pub spans: Vec<Span>,
}

/// A side of the grid (the first six) or of a block; `Missing`: the faces around the missing
/// blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    West,
    East,
    South,
    North,
    Ground,
    Top,
    Missing,
}

impl Side {
    pub const ALL: [Side; 7] =
        [Side::West, Side::East, Side::South, Side::North, Side::Ground, Side::Top, Side::Missing];

    pub fn name(self) -> &'static str {
        match self {
            Side::West => "west",
            Side::East => "east",
            Side::South => "south",
            Side::North => "north",
            Side::Ground => "ground",
            Side::Top => "top",
            Side::Missing => "missing",
        }
    }

    pub fn parse(s: &str) -> Option<Side> {
        Side::ALL.into_iter().find(|x| x.name() == s)
    }

    /// The axis a face of this side is across, and whether it faces the axis' end; `None` for
    /// `Missing`.
    pub fn axis(self) -> Option<(usize, bool)> {
        match self {
            Side::West => Some((0, false)),
            Side::East => Some((0, true)),
            Side::South => Some((1, false)),
            Side::North => Some((1, true)),
            Side::Ground => Some((2, false)),
            Side::Top => Some((2, true)),
            Side::Missing => None,
        }
    }

    fn of(axis: usize, end: bool) -> Side {
        match (axis, end) {
            (0, false) => Side::West,
            (0, true) => Side::East,
            (1, false) => Side::South,
            (1, true) => Side::North,
            (_, false) => Side::Ground,
            (_, true) => Side::Top,
        }
    }
}

/// A face of a block: the block `(i j k)` and the side of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockFace {
    pub block: [usize; 3],
    /// West to top; never `Missing`.
    pub side: Side,
}

/// A region's grid of blocks, as its `blocks` entry has it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockGrid {
    pub axes: [Axis; 3],
    /// The blocks `(i j k)` left out, in the order given.
    pub missing: Vec<[usize; 3]>,
    /// Each side's patch, in the order given.
    pub sides: Vec<(Side, String)>,
    /// Single faces given to another patch than their side's, by patch, in the order given.
    pub faces: Vec<(String, Vec<BlockFace>)>,
    /// Patches declared without faces (for the steps after blockMesh to fill).
    pub no_faces: Vec<String>,
    /// Patch types given explicitly (`symmetryPlane`, `slip`, …), over what the patch's role
    /// gives ([`patch_type`]); the import fills it for every patch whose type the role
    /// wouldn't give, so the dictionary is written again with the types it had.
    pub types: Vec<(String, String)>,
}

/// A `blockMeshDict` read as a grid: the grid, and the type of each patch it lists, in its
/// order.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub grid: BlockGrid,
    pub types: Vec<(String, String)>,
}

/// The blockMesh type a patch of `role` gets in a region of `kind`: in a fluid or vegetation
/// region, `wall` for the ground, a coupled patch, a solid's interior or bottom; `patch`
/// otherwise (inlet, outlet, side, top, sky, canopy, no role). Every patch of a solid is a
/// `patch` (the reference cases' solids; `changeDictionary` makes the coupled ones
/// `mappedWall`).
pub fn patch_type(kind: RegionKind, role: Option<Role>) -> &'static str {
    match (kind, role) {
        (RegionKind::Solid, _) => "patch",
        (_, Some(Role::Ground | Role::Coupled | Role::Interior | Role::Bottom)) => "wall",
        _ => "patch",
    }
}

/// A number as the description and the dictionary write it: the shortest text that reads back
/// as the same value (`0.00739296596393882`, `200`).
///
/// Rust's `{}` never uses an exponent (`1e-300` would be 302 digits), so a number whose plain
/// text is longer than [`NUMBER_LEN`] is written with one (`1e-300`, `1.5e300`), which reads
/// back as the same value too: no number is longer than [`NUMBER_LEN`].
pub fn number(x: f64) -> String {
    // -0 is 0.
    let x = x + 0.0;
    let plain = format!("{x}");
    if plain.len() <= NUMBER_LEN { plain } else { format!("{x:e}") }
}

/// The longest text [`number`] writes: `-1.2345678901234567e-308`.
pub const NUMBER_LEN: usize = 24;

/// Whether every number of the sections is finite and above 0, as blockMesh needs them.
pub fn sections_ok(sections: &[[f64; 3]]) -> bool {
    !sections.is_empty() && sections.iter().flatten().all(|x| x.is_finite() && *x > 0.0)
}

/// The number of cells blockMesh gives each section of a span of `n` cells graded by
/// `sections`: each section's share of the cells rounded, what they lack or have too many
/// added to the first section of the largest share. `None` when blockMesh divides the span
/// uniformly instead (fewer cells than sections), and for sections that aren't all positive
/// numbers ([`sections_ok`]).
///
/// Ported from OpenFOAM's `lineDivide` constructor,
/// `src/mesh/blockMesh/blockEdges/lineDivide/lineDivide.C`, with the shares normalised as the
/// reading of `src/mesh/blockMesh/gradingDescriptor/gradingDescriptors.C` does; as in
/// OpenFOAM-12 and OpenFOAM-8 (the same code in both). Compared with blockMesh's own output by
/// the opt-in test in `tests/blocks.rs`.
// lineDivide.C: Copyright (C) 2011-2019 OpenFOAM Foundation.
// gradingDescriptors.C: Copyright (C) 2015-2021 OpenFOAM Foundation (OpenFOAM-12;
// 2015-2018 in OpenFOAM-8). Both GPL-3.0-or-later.
pub fn line_divide_counts(sections: &[[f64; 3]], n: u32) -> Option<Vec<i64>> {
    if !sections_ok(sections) || (n as usize) < sections.len() {
        return None;
    }
    let n = i64::from(n);
    let sum: f64 = sections.iter().map(|s| s[1]).sum();
    if !(sum.is_finite() && sum > 0.0) {
        return None;
    }
    let fracs: Vec<f64> = sections.iter().map(|s| s[1] / sum).collect();
    let mut counts = Vec::with_capacity(fracs.len());
    for f in &fracs {
        // A share is at most 1, so at most n + 0.5 here.
        let c = f * n as f64 + 0.5;
        if !(c.is_finite() && (0.0..=(n as f64) + 1.0).contains(&c)) {
            return None;
        }
        counts.push(c as i64);
    }
    let mut max = 0usize;
    for (i, f) in fracs.iter().enumerate() {
        if *f > fracs.get(max).copied().unwrap_or(0.0) {
            max = i;
        }
    }
    let total = counts.iter().try_fold(0i64, |t, c| t.checked_add(*c))?;
    if total != n
        && let Some(c) = counts.get_mut(max)
    {
        *c = c.checked_add(n.checked_sub(total)?)?;
    }
    Some(counts)
}

fn parse_number(t: &str) -> Option<f64> {
    t.parse::<f64>().ok().filter(|x| x.is_finite()).map(|x| x + 0.0)
}

pub(crate) fn node_number(n: &Node) -> Option<f64> {
    atom(n).and_then(parse_number)
}

pub(crate) fn node_count(n: &Node) -> Option<u64> {
    atom(n).and_then(|t| t.parse::<u64>().ok())
}

impl Grading {
    fn text(&self) -> String {
        match self {
            Grading::Ratio(r) => number(*r),
            Grading::Sections(s) => {
                let parts: Vec<String> =
                    s.iter().map(|[a, b, c]| format!("({} {} {})", number(*a), number(*b), number(*c))).collect();
                format!("({})", parts.join(" "))
            }
        }
    }

    fn read(n: &Node) -> Option<Grading> {
        match n {
            Node::Atom(_) => node_number(n).map(Grading::Ratio),
            Node::List { items, .. } if !items.is_empty() && items.len() <= MAX_SECTIONS => {
                let mut out = Vec::with_capacity(items.len());
                for s in items {
                    let Node::List { items: three, .. } = s else { return None };
                    match three.as_slice() {
                        [a, b, c] => out.push([node_number(a)?, node_number(b)?, node_number(c)?]),
                        _ => return None,
                    }
                }
                Some(Grading::Sections(out))
            }
            _ => None,
        }
    }

    /// The grading of a span of `cells` seen from its other end: a ratio inverted, the sections
    /// in the other order with their expansions inverted. blockMesh makes the same points of
    /// it within rounding (an inverted ratio is written as the shortest text of its value, so
    /// it reads back as that value; the points differ from the original's by rounding errors
    /// only). Refused, with why, for a grading that isn't positive numbers, and for sections
    /// whose cells blockMesh would share out otherwise in the other order
    /// ([`line_divide_counts`]: a tie between sections gives the extra cells to the first).
    pub fn mirrored(&self, cells: u32) -> Result<Grading, String> {
        let inverse = |e: f64| Some(1.0 / e).filter(|m| e > 0.0 && m.is_finite() && *m > 0.0);
        match self {
            Grading::Ratio(r) => inverse(*r).map(Grading::Ratio).ok_or_else(|| "its grading isn't above 0".to_string()),
            Grading::Sections(v) => {
                if !sections_ok(v) {
                    return Err("its grading's sections aren't all numbers above 0".into());
                }
                let mut out = Vec::with_capacity(v.len());
                for [f, c, e] in v.iter().rev() {
                    out.push([*f, *c, inverse(*e).ok_or("its grading's sections aren't all numbers above 0")?]);
                }
                if let Some(mut forward) = line_divide_counts(v, cells) {
                    forward.reverse();
                    if line_divide_counts(&out, cells) != Some(forward) {
                        return Err("its sections, from the other end, get their cells otherwise in blockMesh (a tie between sections): blockMesh shares out otherwise".into());
                    }
                }
                Ok(Grading::Sections(out))
            }
        }
    }

    /// Where the cells of a span of `cells` cells graded so end, as fractions of the span from
    /// its start (0 and 1 its ends): at every `step`-th cell, and the span's end. The usual
    /// geometric spread (each cell the one before times the ratio to the next; a ratio over the
    /// span, or one per section, the sections' cells shared out as [`line_divide_counts`]
    /// does), for a preview: the mesh blockMesh writes is the one to measure. A grading that
    /// isn't positive numbers, or that spreads past what a float holds, gives even cells.
    pub fn fractions(&self, cells: u32, step: usize) -> Vec<f64> {
        let step = step.max(1);
        let n = cells.max(1) as usize;
        // Within `m` cells expanding by `e` from the first to the last: where cell `i` ends.
        let spread = |i: usize, m: usize, e: f64| -> f64 {
            let even = i as f64 / m as f64;
            if m <= 1 || !(e.is_finite() && e > 0.0) || (e - 1.0).abs() < 1e-12 {
                return even;
            }
            let r = e.powf(1.0 / (m as f64 - 1.0));
            let f = (1.0 - r.powf(i as f64)) / (1.0 - r.powf(m as f64));
            if f.is_finite() { f } else { even }
        };
        // (cells, share of the length, expansion) of each part.
        let parts: Vec<(usize, f64, f64)> = match self {
            Grading::Ratio(r) => vec![(n, 1.0, *r)],
            Grading::Sections(s) => {
                let total: f64 = s.iter().map(|x| x[0]).sum();
                match line_divide_counts(s, cells) {
                    Some(counts) if total.is_finite() && total > 0.0 => s
                        .iter()
                        .zip(counts)
                        .map(|(x, c)| (usize::try_from(c).unwrap_or(0), x[0] / total, x[2]))
                        .collect(),
                    _ => vec![(n, 1.0, 1.0)],
                }
            }
        };
        let mut out = Vec::with_capacity(n / step + 2);
        let (mut at, mut start) = (0usize, 0.0f64);
        for (m, share, e) in parts {
            // Only the cells drawn are visited, whatever the number of cells.
            for i in ((step - at % step) % step..m).step_by(step) {
                out.push(start + share * spread(i, m, e));
            }
            at = at.saturating_add(m);
            start += share;
        }
        out.push(1.0);
        out
    }

    /// An upper bound of the length of its text.
    fn text_len(&self) -> usize {
        match self {
            // `(a b c)` a section and the space after it.
            Grading::Ratio(_) => NUMBER_LEN,
            Grading::Sections(v) => 2 + v.len() * (3 * NUMBER_LEN + 5),
        }
    }
}

/// Whether a patch type is a constraint (`symmetryPlane`, `empty`, `cyclic`, …): the fields'
/// conditions must be of it, so it isn't changed to another type unless asked.
pub fn is_constraint_type(t: &str) -> bool {
    is_constraint(t)
}

/// Whether a patch's type is one a call must name to change or drop: a constraint type
/// ([`is_constraint_type`]), or `wall`, which the wall functions of the fields need.
pub fn is_protected_type(t: &str) -> bool {
    is_constraint(t) || unquote(t) == "wall"
}

fn is_constraint(t: &str) -> bool {
    let t = unquote(t);
    matches!(t, "symmetryPlane" | "symmetry" | "empty" | "wedge")
        || ["cyclic", "nonConformal", "processor"].iter().any(|p| t.starts_with(p))
}

/// Whether a patch type needs entries of its own that the blocks don't hold (a neighbour
/// patch, a region to sample, a processor): the `cyclic…`, `nonConformal…` (`nonConformalCyclic`,
/// `nonConformalError`, `nonConformalMappedWall`, `nonConformalProcessorCyclic`, …), `mapped…`
/// (`mappedWall`, `mappedPatch`, …) and `processor…` types. Such a type is given after blockMesh
/// (`changeDictionary`, `createPatch`), never in the blocks' `types`.
pub fn needs_own_entries(t: &str) -> bool {
    let t = unquote(t);
    ["cyclic", "nonConformal", "mapped", "processor"].iter().any(|p| t.starts_with(p))
}

/// A patch's type as [`BlockGrid::block_mesh_dict`] found it.
fn type_len_text<'a>(types: &'a HashMap<&str, String>, name: &str) -> &'a str {
    types.get(name).map_or("patch", String::as_str)
}

/// The most problems [`BlockGrid::problems`] lists one by one.
pub const MAX_PROBLEMS: usize = 20;

/// The longest `blockMeshDict` a grid may make, in bytes.
pub const MAX_DICT_BYTES: u64 = 48 << 20;

/// The most tokens a grid's `blockMeshDict` may have: below what the dictionary parser reads
/// back (4 000 000), with a margin.
pub const MAX_DICT_TOKENS: u64 = 3_000_000;

/// The most names a grid may give in each of `faces`, `noFaces` and `types`.
pub const MAX_NAMES: usize = 10_000;

impl Axis {
    /// `( 0 (7 0.0074) 200 (5 1) 590 )`.
    pub fn text(&self) -> String {
        let mut s = String::from("(");
        for (i, x) in self.stations.iter().enumerate() {
            let _ = write!(s, " {}", number(*x));
            if let Some(span) = self.spans.get(i).filter(|_| i + 1 < self.stations.len()) {
                let _ = write!(s, " ({} {})", span.cells, span.grading.text());
            }
        }
        s.push_str(" )");
        s
    }

    /// The axis read from the items of its list, or what is wrong with them.
    pub fn read(items: &[Node]) -> Result<Axis, String> {
        const SHAPE: &str =
            "is ( station (cells grading) station … ): stations are numbers, and between two of them (cells grading)";
        let mut a = Axis::default();
        let mut it = items.iter();
        let first = it.next().ok_or_else(|| SHAPE.to_string())?;
        a.stations.push(node_number(first).ok_or_else(|| SHAPE.to_string())?);
        while let Some(span) = it.next() {
            let Node::List { items: two, .. } = span else { return Err(SHAPE.to_string()) };
            let (cells, grading) = match two.as_slice() {
                [c, g] => (c, g),
                _ => return Err(format!("{SHAPE}; `{}` isn't (cells grading)", span.compact())),
            };
            let cells = node_count(cells)
                .and_then(|c| u32::try_from(c).ok())
                .ok_or_else(|| format!("`{}`: the cells are a whole number", span.compact()))?;
            let grading = Grading::read(grading).ok_or_else(|| {
                format!(
                    "`{}`: the grading is a number or sections ((fraction cells-fraction expansion) …)",
                    span.compact()
                )
            })?;
            a.spans.push(Span { cells, grading });
            let next = it.next().ok_or_else(|| format!("{SHAPE}; it ends with a span, not a station"))?;
            a.stations.push(node_number(next).ok_or_else(|| SHAPE.to_string())?);
            if a.stations.len() > MAX_STATIONS {
                return Err(format!("has more than {MAX_STATIONS} stations"));
            }
        }
        Ok(a)
    }

    fn blocks(&self) -> usize {
        self.stations.len().saturating_sub(1)
    }

    /// The number of cells along the axis; `None` past `u64`.
    pub fn cells(&self) -> Option<u64> {
        self.spans.iter().try_fold(0u64, |t, s| t.checked_add(u64::from(s.cells)))
    }

    /// Where the cells' lines are along the axis (a preview, [`Grading::fractions`]): at every
    /// `step`-th cell of each span, and every station. Empty when the stations and the spans
    /// don't match.
    pub fn cell_lines(&self, step: usize) -> Vec<f64> {
        if self.spans.len() + 1 != self.stations.len() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, span) in self.spans.iter().enumerate() {
            let (Some(lo), Some(hi)) = (self.stations.get(i), self.stations.get(i + 1)) else { continue };
            let mut f = span.grading.fractions(span.cells, step);
            // Each span's end is the next one's start.
            f.pop();
            out.extend(f.into_iter().map(|x| lo + (hi - lo) * x));
        }
        out.extend(self.stations.last());
        out
    }
}

fn side_of(axis: usize, end: bool) -> Side {
    Side::of(axis, end)
}

impl BlockGrid {
    /// The number of blocks along each axis.
    pub fn counts(&self) -> [usize; 3] {
        [self.axes[0].blocks(), self.axes[1].blocks(), self.axes[2].blocks()]
    }

    /// The number of blocks of the whole grid (missing ones included); `None` past `usize`.
    pub fn size(&self) -> Option<usize> {
        let [a, b, c] = self.counts();
        a.checked_mul(b)?.checked_mul(c)
    }

    fn missing_set(&self) -> HashSet<[usize; 3]> {
        self.missing.iter().copied().collect()
    }

    /// The number of cells blockMesh makes; `None` when the grid has problems that leave it
    /// unknown (or past `u64`).
    pub fn cells(&self) -> Option<u64> {
        let mut per = [0u64; 3];
        for (a, axis) in self.axes.iter().enumerate() {
            if axis.spans.len() != axis.blocks() {
                return None;
            }
            per[a] = axis.spans.iter().try_fold(0u64, |s, x| s.checked_add(u64::from(x.cells)))?;
        }
        let mut total = per[0].checked_mul(per[1])?.checked_mul(per[2])?;
        for b in self.missing_set() {
            let mut n = 1u64;
            for (a, axis) in self.axes.iter().enumerate() {
                n = n.checked_mul(u64::from(axis.spans.get(b[a])?.cells))?;
            }
            total = total.checked_sub(n)?;
        }
        Some(total)
    }

    /// The patch named for `side`.
    pub fn side(&self, side: Side) -> Option<&str> {
        self.sides.iter().find(|(s, _)| *s == side).map(|(_, n)| n.as_str())
    }

    /// The patches the grid names, each once, in the order the dictionary lists them: the
    /// sides', then those of single faces, then those without faces; never `defaultFaces`.
    pub fn patch_names(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let names = self
            .sides
            .iter()
            .map(|(_, n)| n.as_str())
            .chain(self.faces.iter().map(|(n, _)| n.as_str()))
            .chain(self.no_faces.iter().map(String::as_str));
        for n in names {
            if n != DEFAULT_FACES && seen.insert(n) {
                out.push(n);
            }
        }
        out
    }

    /// Every face on the boundary of the blocks, with the side it belongs to (`Missing` for
    /// one next to a missing block). Empty when the grid is too large or has no blocks.
    fn boundary_sides(&self) -> Vec<(BlockFace, Side)> {
        let counts = self.counts();
        let mut out = Vec::new();
        match self.size() {
            Some(n) if n > 0 && n <= MAX_BLOCKS => {}
            _ => return out,
        }
        let missing = self.missing_set();
        for k in 0..counts[2] {
            for j in 0..counts[1] {
                for i in 0..counts[0] {
                    let b = [i, j, k];
                    if missing.contains(&b) {
                        continue;
                    }
                    for side in &Side::ALL[..6] {
                        let Some((a, end)) = side.axis() else { continue };
                        let next =
                            if end { b[a].checked_add(1).filter(|x| *x < counts[a]) } else { b[a].checked_sub(1) };
                        let category = match next {
                            None => *side,
                            Some(x) => {
                                let mut n = b;
                                n[a] = x;
                                if missing.contains(&n) { Side::Missing } else { continue }
                            }
                        };
                        out.push((BlockFace { block: b, side: *side }, category));
                    }
                }
            }
        }
        out
    }

    /// Every face on the boundary of the blocks with its side and its patch (`None`: none
    /// named; `defaultFaces` when so named).
    pub fn boundary(&self) -> BTreeMap<BlockFace, (Side, Option<&str>)> {
        let mut given: HashMap<BlockFace, &str> = HashMap::new();
        for (name, faces) in &self.faces {
            for f in faces {
                given.entry(*f).or_insert(name.as_str());
            }
        }
        self.boundary_sides()
            .into_iter()
            .map(|(f, cat)| {
                let name = given.get(&f).copied().or_else(|| self.side(cat));
                (f, (cat, name))
            })
            .collect()
    }

    /// The type given to `name` in `types` (one lookup; for many, [`BlockGrid::type_map`]).
    pub fn given_type(&self, name: &str) -> Option<&str> {
        self.types.iter().find(|(n, _)| n == name).map(|(_, t)| t.as_str())
    }

    /// An upper bound of the length of the `blockMeshDict` the grid makes; `None` past `u64`.
    /// Each patch typed as `types` gives it, else as a role does (5 characters at most); the
    /// header's region and location not counted ([`BlockGrid::block_mesh_dict`] adds them).
    pub fn estimated_len(&self) -> Option<u64> {
        let types = self.type_map();
        self.estimated_len_with(&|n| types.get(n).map_or(5, |t| t.len()))
    }

    /// The types `types` gives, by patch (the first of a name given twice).
    pub fn type_map(&self) -> HashMap<&str, &str> {
        let mut m = HashMap::with_capacity(self.types.len());
        for (n, t) in &self.types {
            m.entry(n.as_str()).or_insert(t.as_str());
        }
        m
    }

    /// An upper bound of the number of tokens of the `blockMeshDict` the grid makes, as the
    /// dictionary parser counts them (whitespace and comments included); `None` past `u64`.
    pub fn estimated_tokens(&self) -> Option<u64> {
        let [nx, ny, nz] = self.counts().map(|n| n as u64);
        let tokens = |g: &Grading| -> u64 {
            match g {
                Grading::Ratio(_) => 1,
                Grading::Sections(v) => 2 + 5 * v.len() as u64,
            }
        };
        let g = |a: usize| -> Option<u64> {
            self.axes[a].spans.iter().try_fold(0u64, |s, x| s.checked_add(tokens(&x.grading)))
        };
        // `( x y z )` a vertex; `hex ( 8 ) ( 3 ) simpleGrading ( … )` a block without its
        // gradings, and six faces `( a b c d )` at most.
        let vertices = (nx + 1).checked_mul(ny + 1)?.checked_mul(nz + 1)?.checked_mul(5)?;
        let blocks = nx.checked_mul(ny)?.checked_mul(nz)?.checked_mul(19 + 6 * 6)?;
        let gradings = ny
            .checked_mul(nz)?
            .checked_mul(g(0)?)?
            .checked_add(nx.checked_mul(nz)?.checked_mul(g(1)?)?)?
            .checked_add(nx.checked_mul(ny)?.checked_mul(g(2)?)?)?;
        // `name { type t ; faces ( ) ; }` a patch, and the header, the keys and defaultPatch.
        let patches = (self.patch_names().len() as u64).checked_mul(12)?;
        // The parser counts the whitespace and comments between them as tokens too: one run
        // after each at most.
        vertices.checked_add(blocks)?.checked_add(gradings)?.checked_add(patches)?.checked_add(200)?.checked_mul(2)
    }

    /// [`BlockGrid::estimated_len`] with each patch's type `type_len` characters long.
    fn estimated_len_with(&self, type_len: &dyn Fn(&str) -> usize) -> Option<u64> {
        let [nx, ny, nz] = self.counts().map(|n| n as u64);
        let g = |a: usize| -> Option<u64> {
            self.axes[a].spans.iter().try_fold(0u64, |s, x| s.checked_add(x.grading.text_len() as u64))
        };
        // `    (x y z)` a vertex.
        let vertices = (nx + 1).checked_mul(ny + 1)?.checked_mul(nz + 1)?.checked_mul(3 * NUMBER_LEN as u64 + 10)?;
        let blocks = nx.checked_mul(ny)?.checked_mul(nz)?;
        // Each block: its hex line without its gradings, 130 characters at most (8 vertex
        // labels of 6 digits at most: the stations allow fewer than 1 000 000 vertices for
        // MAX_BLOCKS blocks, 3 counts of 10 digits, and the words), and six faces at most (4 labels, 60).
        let lines = blocks.checked_mul(130 + 6 * 60)?;
        let gradings = ny
            .checked_mul(nz)?
            .checked_mul(g(0)?)?
            .checked_add(nx.checked_mul(nz)?.checked_mul(g(1)?)?)?
            .checked_add(nx.checked_mul(ny)?.checked_mul(g(2)?)?)?;
        // Each patch: `    name\n    {\n        type …;\n        faces\n        (\n        );\n    }\n`,
        // its name perhaps quoted.
        let names = self
            .patch_names()
            .iter()
            .try_fold(0u64, |t, n| t.checked_add(2 * n.len() as u64 + 2 + type_len(n) as u64 + 100))?;
        vertices.checked_add(lines)?.checked_add(gradings)?.checked_add(names)?.checked_add(4096)
    }

    /// What is wrong with the grid, each with where in the `blocks` entry (`x`, `missing`,
    /// `patches/west`, `faces/street`) and what (the first [`MAX_PROBLEMS`], then how many
    /// more). A grid with problems writes no dictionary.
    pub fn problems(&self) -> Vec<(String, String)> {
        let mut out = self.all_problems();
        if out.len() > MAX_PROBLEMS {
            let more = out.len() - MAX_PROBLEMS;
            out.truncate(MAX_PROBLEMS);
            out.push((String::new(), format!("and {more} more problems")));
        }
        out
    }

    fn all_problems(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut put = |at: &str, m: String| out.push((at.to_string(), m));
        // Many names: refused before any work over them.
        let single: usize = self.faces.iter().map(|(_, f)| f.len()).sum();
        let counts = [("faces", self.faces.len()), ("noFaces", self.no_faces.len()), ("types", self.types.len())];
        let mut too_many = false;
        for (k, n) in counts {
            if n > MAX_NAMES {
                put(k, format!("names {n} patches, more than {MAX_NAMES}"));
                too_many = true;
            }
        }
        if single > 6 * MAX_BLOCKS || self.missing.len() > 2 * MAX_BLOCKS {
            put("", format!("lists {single} single faces and {} missing blocks: too many", self.missing.len()));
            too_many = true;
        }
        if too_many {
            return out;
        }
        let mut axes_ok = true;
        for (a, axis) in self.axes.iter().enumerate() {
            let at = AXES[a];
            if axis.stations.len() > MAX_STATIONS
                || axis.spans.iter().any(|x| matches!(&x.grading, Grading::Sections(v) if v.len() > MAX_SECTIONS))
            {
                put(
                    at,
                    format!("has more than {MAX_STATIONS} stations, or a grading of more than {MAX_SECTIONS} sections"),
                );
                axes_ok = false;
                continue;
            }
            if axis.stations.len() < 2 {
                put(at, "has no span: two stations at least, with (cells grading) between".into());
                axes_ok = false;
                continue;
            }
            if axis.spans.len() + 1 != axis.stations.len() {
                put(at, "has a (cells grading) between each two stations, and only there".into());
                axes_ok = false;
                continue;
            }
            for (w, span) in axis.stations.windows(2).zip(&axis.spans) {
                let [lo, hi] = [w.first().copied().unwrap_or(0.0), w.get(1).copied().unwrap_or(0.0)];
                if !lo.is_finite() || !hi.is_finite() {
                    put(at, "has a station that isn't a number".into());
                    axes_ok = false;
                } else if hi <= lo {
                    put(at, format!("the stations don't increase: {} then {}", number(lo), number(hi)));
                    axes_ok = false;
                }
                let between = format!("between {} and {}", number(lo), number(hi));
                if span.cells == 0 {
                    put(at, format!("{between}: no cells (1 at least)"));
                }
                match &span.grading {
                    Grading::Ratio(r) if !(r.is_finite() && *r > 0.0) => {
                        put(at, format!("{between}: the grading is a ratio above 0"))
                    }
                    Grading::Sections(s) if s.is_empty() => put(at, format!("{between}: the grading has no section")),
                    Grading::Sections(s) if s.iter().flatten().any(|x| !(x.is_finite() && *x > 0.0)) => {
                        put(at, format!("{between}: each section is (fraction cells-fraction expansion), all above 0"))
                    }
                    Grading::Sections(s)
                        if line_divide_counts(s, span.cells).is_some_and(|c| c.iter().any(|n| *n < 1)) =>
                    {
                        put(
                            at,
                            format!(
                                "{between}: blockMesh would give a section no cells ({} cells over {} sections, shared as {:?}): more cells, or fewer sections",
                                span.cells,
                                s.len(),
                                line_divide_counts(s, span.cells).unwrap_or_default()
                            ),
                        )
                    }
                    _ => {}
                }
            }
        }
        if !axes_ok {
            return out;
        }
        let counts = self.counts();
        match self.size() {
            Some(n) if n <= MAX_BLOCKS => {}
            _ => {
                put(
                    "",
                    format!(
                        "has {}×{}×{} blocks, more than {MAX_BLOCKS}: fewer stations, more cells between them",
                        counts[0], counts[1], counts[2]
                    ),
                );
                return out;
            }
        }
        if self.estimated_len().is_none_or(|n| n > MAX_DICT_BYTES)
            || self.estimated_tokens().is_none_or(|n| n > MAX_DICT_TOKENS)
        {
            put(
                "",
                format!(
                    "would make a blockMeshDict larger than its dictionary can be read back ({} MB, {} tokens): fewer blocks or simpler gradings",
                    MAX_DICT_BYTES >> 20,
                    MAX_DICT_TOKENS
                ),
            );
            return out;
        }
        let mut seen = HashSet::new();
        for b in &self.missing {
            let shown = format!("({} {} {})", b[0], b[1], b[2]);
            if b.iter().zip(counts).any(|(x, n)| *x >= n) {
                put(
                    "missing",
                    format!(
                        "{shown} is outside the grid of {}×{}×{} blocks (they count from 0)",
                        counts[0], counts[1], counts[2]
                    ),
                );
            } else if !seen.insert(*b) {
                put("missing", format!("{shown} is listed twice"));
            }
        }
        if Some(seen.len()) == self.size() {
            put("missing", "leaves no block".into());
            return out;
        }
        // A layer of missing blocks: no block would carry its cells into the dictionary.
        let mut per: [Vec<usize>; 3] = [vec![0; counts[0]], vec![0; counts[1]], vec![0; counts[2]]];
        for b in &seen {
            for a in 0..3 {
                if let Some(c) = per[a].get_mut(b[a]) {
                    *c += 1;
                }
            }
        }
        for a in 0..3 {
            let layer = counts.iter().enumerate().filter(|(i, _)| *i != a).map(|(_, n)| *n).product::<usize>();
            for (x, n) in per[a].iter().enumerate() {
                if *n == layer {
                    let st = |i: usize| self.axes[a].stations.get(i).map_or_else(String::new, |v| number(*v));
                    put(
                        "missing",
                        format!(
                            "every block between {} {} and {} is missing: take the station out instead",
                            AXES[a],
                            st(x),
                            st(x + 1)
                        ),
                    );
                }
            }
        }
        // Single faces: blocks of the grid, on its boundary, each once.
        let edge: HashSet<BlockFace> = self.boundary_sides().into_iter().map(|(f, _)| f).collect();
        let mut listed: HashMap<BlockFace, &str> = HashMap::new();
        for (name, faces) in &self.faces {
            let at = format!("faces/{name}");
            for f in faces {
                let shown = format!("({} {} {} {})", f.block[0], f.block[1], f.block[2], f.side.name());
                if f.side == Side::Missing {
                    put(&at, format!("{shown}: a face is west, east, south, north, ground or top of its block"));
                } else if f.block.iter().zip(counts).any(|(x, n)| *x >= n) {
                    put(&at, format!("{shown} is not a block of the grid"));
                } else if seen.contains(&f.block) {
                    put(&at, format!("{shown} is a face of a missing block"));
                } else if !edge.contains(f) {
                    put(&at, format!("{shown} is inside the mesh: only a face on its boundary takes a patch"));
                } else if let Some(other) = listed.insert(*f, name) {
                    put(&at, format!("{shown} is listed for {other} already"));
                }
            }
        }
        // Every face named.
        let mut unnamed: Vec<Side> = Vec::new();
        for (cat, name) in self.boundary().values() {
            if name.is_none() && !unnamed.contains(cat) {
                unnamed.push(*cat);
            }
        }
        unnamed.sort();
        for cat in unnamed {
            let what = if cat == Side::Missing {
                "the faces around the missing blocks have".to_string()
            } else {
                format!("the {} side has faces with", cat.name())
            };
            put(
                &format!("patches/{}", cat.name()),
                format!("is missing: {what} no patch (every face needs one; {DEFAULT_FACES} leaves them to blockMesh)"),
            );
        }
        let with_faces: HashSet<&str> =
            self.boundary().values().filter_map(|(_, n)| *n).filter(|n| *n != DEFAULT_FACES).collect();
        for n in &self.no_faces {
            if n == DEFAULT_FACES {
                put("noFaces", format!("{DEFAULT_FACES} is blockMesh's own patch: it isn't declared"));
            } else if with_faces.contains(n.as_str()) {
                put("noFaces", format!("{n} has faces"));
            }
        }
        for n in self.patch_names() {
            if !crate::description_edit::plain_name(n) {
                put("patches", format!("`{n}` is not a plain OpenFOAM name"));
            }
        }
        let mut names: HashSet<&str> = self.patch_names().into_iter().collect();
        // blockMesh's own patch, when faces are left to it, may be typed too.
        if self.boundary().values().any(|(_, n)| n.is_none_or(|n| n == DEFAULT_FACES)) {
            names.insert(DEFAULT_FACES);
        }
        let mut typed = HashSet::new();
        for (n, t) in &self.types {
            let at = format!("types/{n}");
            if !names.contains(n.as_str()) {
                put(&at, "names no patch of the blocks".into());
            } else if !typed.insert(n.as_str()) {
                put(&at, "is given twice".into());
            } else if !crate::description_edit::plain_name(t) {
                put(&at, format!("`{t}` is not a patch type (one plain word)"));
            } else if needs_own_entries(t) {
                put(
                    &at,
                    format!(
                        "{t} needs entries of its own (neighbourPatch, sampleRegion, …) the blocks don't hold: give it in a step after blockMesh (changeDictionary, createPatch)"
                    ),
                );
            }
        }
        out
    }

    /// Whether `other` makes the same mesh: the same stations, spans and missing blocks, each
    /// face in the same patch, the same patches declared.
    pub fn same_mesh(&self, other: &BlockGrid) -> bool {
        if self.axes != other.axes || self.missing_set() != other.missing_set() {
            return false;
        }
        let names = |g: &BlockGrid| -> HashSet<String> { g.patch_names().into_iter().map(str::to_string).collect() };
        // A face no patch names is blockMesh's, as the dictionary written has it.
        let faces = |g: &BlockGrid| -> BTreeMap<BlockFace, String> {
            g.boundary().into_iter().map(|(f, (_, n))| (f, n.unwrap_or(DEFAULT_FACES).to_string())).collect()
        };
        names(self) == names(other) && faces(self) == faces(other)
    }

    /// `( (1 1 0) (3 1 0) )`.
    pub fn missing_text(&self) -> String {
        let parts: Vec<String> = self.missing.iter().map(|b| format!("({} {} {})", b[0], b[1], b[2])).collect();
        if parts.is_empty() { "( )".to_string() } else { format!("( {} )", parts.join(" ")) }
    }

    /// `{ west inlet; east outlet; }`.
    pub fn patches_text(&self) -> String {
        let parts: Vec<String> = self.sides.iter().map(|(s, n)| format!("{} {};", s.name(), word(n))).collect();
        if parts.is_empty() { "{ }".to_string() } else { format!("{{ {} }}", parts.join(" ")) }
    }

    /// `{ air_to_street ( (2 1 0 ground) ); }`.
    pub fn faces_text(&self) -> String {
        let parts: Vec<String> = self
            .faces
            .iter()
            .map(|(n, fs)| {
                let list: Vec<String> = fs
                    .iter()
                    .map(|f| format!("({} {} {} {})", f.block[0], f.block[1], f.block[2], f.side.name()))
                    .collect();
                format!("{} ( {} );", word(n), list.join(" "))
            })
            .collect();
        if parts.is_empty() { "{ }".to_string() } else { format!("{{ {} }}", parts.join(" ")) }
    }

    /// `( street roofs )`.
    pub fn no_faces_text(&self) -> String {
        let parts: Vec<String> = self.no_faces.iter().map(|n| word(n)).collect();
        if parts.is_empty() { "( )".to_string() } else { format!("( {} )", parts.join(" ")) }
    }

    /// The keys of the `blocks` entry with their values; `None` for one the grid leaves out
    /// (no missing blocks, single faces, patches without faces).
    pub fn parts(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("x", Some(self.axes[0].text())),
            ("y", Some(self.axes[1].text())),
            ("z", Some(self.axes[2].text())),
            ("missing", (!self.missing.is_empty()).then(|| self.missing_text())),
            ("patches", (!self.sides.is_empty()).then(|| self.patches_text())),
            ("faces", (!self.faces.is_empty()).then(|| self.faces_text())),
            ("noFaces", (!self.no_faces.is_empty()).then(|| self.no_faces_text())),
            ("types", (!self.types.is_empty()).then(|| self.types_text())),
        ]
    }

    /// `{ side1 slip; top symmetryPlane; }`.
    pub fn types_text(&self) -> String {
        let parts: Vec<String> = self.types.iter().map(|(n, t)| format!("{} {};", word(n), word(t))).collect();
        if parts.is_empty() { "{ }".to_string() } else { format!("{{ {} }}", parts.join(" ")) }
    }

    /// The grid with patch `from` called `to` wherever it names it (sides, single faces,
    /// patches without faces, types); whether it named it.
    pub fn rename_patch(&mut self, from: &str, to: &str) -> bool {
        let mut any = false;
        let names = self
            .sides
            .iter_mut()
            .map(|(_, n)| n)
            .chain(self.faces.iter_mut().map(|(n, _)| n))
            .chain(self.no_faces.iter_mut())
            .chain(self.types.iter_mut().map(|(n, _)| n));
        for n in names {
            if n == from {
                *n = to.to_string();
                any = true;
            }
        }
        any
    }

    /// The `blocks` entry as the description writes it, indented by `indent`.
    pub fn entry(&self, indent: &str) -> String {
        let mut s = format!("{indent}blocks\n{indent}{{\n");
        for (k, v) in self.parts() {
            if let Some(v) = v {
                // A dictionary value ends with its brace, a list with `;`.
                let end = if v.starts_with('{') { "" } else { ";" };
                let _ = writeln!(s, "{indent}    {k:<8}{v}{end}");
            }
        }
        let _ = writeln!(s, "{indent}}}");
        s
    }

    /// The grid read from a `blocks` entry's dictionary; what can't be read is a problem at
    /// `at/<key>`. `None` without its three axes.
    pub(crate) fn read(body: &Dict, at: &str, problem: &mut impl FnMut(&str, String)) -> Option<BlockGrid> {
        let mut g = BlockGrid::default();
        let mut axes: [Option<Axis>; 3] = [None, None, None];
        let mut seen = HashSet::new();
        non_entries(body, at, problem);
        for e in body.entries() {
            let here = format!("{at}/{}", e.key());
            if twice(&mut seen, e, &here, problem) {
                continue;
            }
            match e.key() {
                k @ ("x" | "y" | "z") => match list(e).map(Axis::read) {
                    Some(Ok(a)) => {
                        let i = AXES.iter().position(|x| *x == k).unwrap_or(0);
                        if let Some(slot) = axes.get_mut(i) {
                            *slot = Some(a);
                        }
                    }
                    Some(Err(m)) => problem(&here, m),
                    None => problem(
                        &here,
                        "is ( station (cells grading) station … ): stations are numbers, and between two of them (cells grading)".into(),
                    ),
                },
                "missing" => match list(e) {
                    Some(items) => {
                        for n in items {
                            match block_of(n) {
                                Some(b) => g.missing.push(b),
                                None => problem(&here, format!("`{}` isn't a block (i j k)", n.compact())),
                            }
                        }
                    }
                    None => problem(&here, "is a list of blocks: ( (i j k) … ), counted from 0".into()),
                },
                "patches" => match e.dict() {
                    Some(pb) => {
                        non_entries(pb, &here, problem);
                        let mut sides = HashSet::new();
                        for p in pb.entries() {
                            let pat = format!("{here}/{}", p.key());
                            match Side::parse(p.key()) {
                                None => problem(
                                    &pat,
                                    "is not a side: west, east, south, north, ground, top or missing".into(),
                                ),
                                Some(_) if !sides.insert(p.key()) => {
                                    problem(&pat, "is given twice: the later ones are left out".into())
                                }
                                Some(side) => match single(p) {
                                    Some(n) if plain_word(&n) => g.sides.push((side, n)),
                                    _ => problem(&pat, "is the name of its patch".into()),
                                },
                            }
                        }
                    }
                    None => problem(&here, "is a dictionary: { west inlet; … missing buildings; }".into()),
                },
                "faces" => match e.dict() {
                    Some(fb) => {
                        non_entries(fb, &here, problem);
                        let mut names = HashSet::new();
                        for p in fb.entries() {
                            let name = entry_name(p);
                            let pat = format!("{here}/{name}");
                            if name.is_empty() {
                                problem(&pat, "is not a name".into());
                                continue;
                            }
                            if !names.insert(name.clone()) {
                                problem(&pat, "is given twice: the later ones are left out".into());
                                continue;
                            }
                            let mut faces = Vec::new();
                            match list(p) {
                                Some(items) => {
                                    for n in items {
                                        match face_of(n) {
                                            Some(f) => faces.push(f),
                                            None => problem(
                                                &pat,
                                                format!("`{}` isn't a block's face (i j k side)", n.compact()),
                                            ),
                                        }
                                    }
                                }
                                None => problem(&pat, "is a list of faces: ( (i j k side) … )".into()),
                            }
                            g.faces.push((name, faces));
                        }
                    }
                    None => problem(&here, "is a dictionary: { patch ( (i j k side) … ); … }".into()),
                },
                "types" => match e.dict() {
                    Some(tb) => {
                        non_entries(tb, &here, problem);
                        let mut names = HashSet::new();
                        for t in tb.entries() {
                            let name = entry_name(t);
                            let tat = format!("{here}/{name}");
                            match single(t) {
                                _ if name.is_empty() => problem(&tat, "is not a name".into()),
                                _ if !names.insert(name.clone()) => {
                                    problem(&tat, "is given twice: the later ones are left out".into())
                                }
                                Some(kind) if plain_word(&kind) => g.types.push((name, kind)),
                                _ => problem(&tat, "is the patch's blockMesh type (patch, wall, symmetryPlane, …)".into()),
                            }
                        }
                    }
                    None => problem(&here, "is a dictionary: { patch type; … }".into()),
                },
                "noFaces" => match list(e) {
                    Some(items) => {
                        let mut listed: HashSet<String> = HashSet::new();
                        for n in items {
                            match atom(n).map(unescaped).filter(|w| plain_word(w)) {
                                Some(w) if listed.insert(w.clone()) => g.no_faces.push(w),
                                Some(w) => problem(&here, format!("{w} is listed twice")),
                                None => problem(&here, "is a list of patch names".into()),
                            }
                        }
                    }
                    None => problem(&here, "is a list of patch names: ( street roofs )".into()),
                },
                _ => problem(&here, "is not a key of blocks (x, y, z, missing, patches, faces, noFaces, types)".into()),
            }
        }
        let mut done = Vec::new();
        for (i, a) in axes.into_iter().enumerate() {
            match a {
                Some(a) => done.push(a),
                None => {
                    let k = AXES.get(i).copied().unwrap_or("x");
                    if body.get(k).is_none() {
                        problem(&format!("{at}/{k}"), "is missing: the blocks are left out".into());
                    }
                }
            }
        }
        let [x, y, z]: [Axis; 3] = done.try_into().ok()?;
        g.axes = [x, y, z];
        Some(g)
    }

    /// The `blockMeshDict` of the grid, for region `region` at `location` (`system/air`), each
    /// patch of the type `type_of` gives. Generated: its header says so ([`GENERATED`]).
    /// Refused (with why) for a grid too large to write, or whose axes don't have a span
    /// between each two stations; other problems ([`BlockGrid::problems`]) are the caller's.
    pub fn block_mesh_dict(
        &self,
        region: &str,
        location: &str,
        type_of: &dyn Fn(&str) -> String,
    ) -> Result<String, String> {
        if self.axes.iter().any(|a| a.stations.len() < 2 || a.spans.len() + 1 != a.stations.len()) {
            return Err("an axis has no span, or not one between each two stations".into());
        }
        if self.size().is_none_or(|n| n > MAX_BLOCKS) {
            return Err(format!("more than {MAX_BLOCKS} blocks"));
        }
        let types: HashMap<&str, String> = self.patch_names().into_iter().map(|n| (n, type_of(n))).collect();
        let type_len = |n: &str| types.get(n).map_or(0, String::len);
        let bound =
            self.estimated_len_with(&type_len).and_then(|n| n.checked_add(region.len() as u64 + location.len() as u64));
        if bound.is_none_or(|n| n > MAX_DICT_BYTES) || self.estimated_tokens().is_none_or(|n| n > MAX_DICT_TOKENS) {
            return Err(format!(
                "the blockMeshDict would be larger than it can be read back ({} MB, {} tokens)",
                MAX_DICT_BYTES >> 20,
                MAX_DICT_TOKENS
            ));
        }
        let counts = self.counts();
        let n = [counts[0] + 1, counts[1] + 1, counts[2] + 1];
        let mut s = format!(
            "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"{location}\";\n    object      blockMeshDict;\n}}\n\n\
             convertToMeters 1;\n\nvertices\n(\n"
        );
        for k in 0..n[2] {
            for j in 0..n[1] {
                for i in 0..n[0] {
                    let p = |a: usize, x: usize| self.axes[a].stations.get(x).copied().unwrap_or(0.0);
                    let _ = writeln!(s, "    ({} {} {})", number(p(0, i)), number(p(1, j)), number(p(2, k)));
                }
            }
        }
        s.push_str(");\n\nblocks\n(\n");
        let v = |i: usize, j: usize, k: usize| i + n[0] * (j + n[1] * k);
        let missing = self.missing_set();
        let corners = |b: [usize; 3]| -> [usize; 8] {
            let [i, j, k] = b;
            [
                v(i, j, k),
                v(i + 1, j, k),
                v(i + 1, j + 1, k),
                v(i, j + 1, k),
                v(i, j, k + 1),
                v(i + 1, j, k + 1),
                v(i + 1, j + 1, k + 1),
                v(i, j + 1, k + 1),
            ]
        };
        for k in 0..counts[2] {
            for j in 0..counts[1] {
                for i in 0..counts[0] {
                    if missing.contains(&[i, j, k]) {
                        continue;
                    }
                    let c = corners([i, j, k]);
                    let span = |a: usize, x: usize| self.axes[a].spans.get(x);
                    let (Some(sx), Some(sy), Some(sz)) = (span(0, i), span(1, j), span(2, k)) else { continue };
                    let _ = writeln!(
                        s,
                        "    hex ({} {} {} {} {} {} {} {}) ({} {} {}) simpleGrading ({} {} {})",
                        c[0],
                        c[1],
                        c[2],
                        c[3],
                        c[4],
                        c[5],
                        c[6],
                        c[7],
                        sx.cells,
                        sy.cells,
                        sz.cells,
                        sx.grading.text(),
                        sy.grading.text(),
                        sz.grading.text()
                    );
                }
            }
        }
        s.push_str(");\n\nboundary\n(\n");
        let boundary = self.boundary();
        let mut defaults = false;
        let mut by_patch: BTreeMap<&str, Vec<BlockFace>> = BTreeMap::new();
        for (f, (_, name)) in &boundary {
            match name {
                Some(n) if *n != DEFAULT_FACES => by_patch.entry(n).or_default().push(*f),
                _ => defaults = true,
            }
        }
        for name in self.patch_names() {
            let _ = writeln!(
                s,
                "    {}\n    {{\n        type {};\n        faces\n        (",
                word(name),
                type_len_text(&types, name)
            );
            for f in by_patch.get(name).map(Vec::as_slice).unwrap_or_default() {
                let c = corners(f.block);
                // Outward, as blockMesh's hex faces are.
                let q = match f.side {
                    Side::West => [c[0], c[4], c[7], c[3]],
                    Side::East => [c[1], c[2], c[6], c[5]],
                    Side::South => [c[0], c[1], c[5], c[4]],
                    Side::North => [c[3], c[7], c[6], c[2]],
                    Side::Ground => [c[0], c[3], c[2], c[1]],
                    Side::Top | Side::Missing => [c[4], c[5], c[6], c[7]],
                };
                let _ = writeln!(s, "            ({} {} {} {})", q[0], q[1], q[2], q[3]);
            }
            s.push_str("        );\n    }\n");
        }
        s.push_str(");\n");
        if defaults {
            let _ = write!(
                s,
                "\n// The faces left to blockMesh's own patch, as the case had them.\ndefaultPatch\n{{\n    name {DEFAULT_FACES};\n    type {};\n}}\n",
                type_of(DEFAULT_FACES)
            );
        }
        s.push_str("\n// ************************************************************************* //\n");
        Ok(format!(
            "// {GENERATED}\n// The blocks of region {region} in system/umcfWorkflow (docs/common-workflow.md).\n{CHECKSUM}{:016x} (what follows, as umcfGui wrote it)\n{s}",
            checksum(s.as_bytes())
        ))
    }
}

/// The header line that carries a generated file's checksum.
const CHECKSUM: &str = "// checksum ";

/// A checksum of a grid's shape: its stations, cells and gradings, missing blocks and single
/// faces, as the description writes them (not its patches or types).
pub fn shape_checksum(g: &BlockGrid) -> u64 {
    let mut text = String::new();
    for (k, v) in g.parts() {
        if matches!(k, "x" | "y" | "z" | "missing" | "faces") {
            let _ = writeln!(text, "{k} {}", v.unwrap_or_default());
        }
    }
    checksum(text.as_bytes())
}

/// FNV-1a, 64 bits: the same on every platform and version, unlike the standard library's
/// hashers.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

fn block_of(n: &Node) -> Option<[usize; 3]> {
    let Node::List { items, .. } = n else { return None };
    match items.as_slice() {
        [a, b, c] => Some([index(a)?, index(b)?, index(c)?]),
        _ => None,
    }
}

fn face_of(n: &Node) -> Option<BlockFace> {
    let Node::List { items, .. } = n else { return None };
    match items.as_slice() {
        [a, b, c, s] => {
            let side = Side::parse(atom(s)?).filter(|s| *s != Side::Missing)?;
            Some(BlockFace { block: [index(a)?, index(b)?, index(c)?], side })
        }
        _ => None,
    }
}

fn index(n: &Node) -> Option<usize> {
    node_count(n).and_then(|x| usize::try_from(x).ok())
}

/// The local coordinates of a hex's eight vertices, as blockMesh numbers them.
const HEX: [[usize; 3]; 8] = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]];

struct RawBlock {
    verts: [usize; 8],
    cells: [u32; 3],
    grading: [Grading; 3],
}

/// The station index of each coordinate of `p`.
fn station_index(stations: &[Vec<f64>; 3], p: [f64; 3]) -> Option<[usize; 3]> {
    let mut out = [0usize; 3];
    for a in 0..3 {
        out[a] = stations[a].binary_search_by(|s| s.total_cmp(&p[a])).ok()?;
    }
    Some(out)
}

/// Every item of `body` that isn't an entry (an `#include`, a `$macro`, a value without a key)
/// is a problem at `at`: the grid can't hold it, and writing the key again would lose it.
pub(crate) fn non_entries(body: &Dict, at: &str, problem: &mut impl FnMut(&str, String)) {
    for item in &body.items {
        match item {
            Item::Entry(_) => {}
            Item::Directive { name, .. } | Item::Macro { name, .. } => {
                problem(at, format!("`{}` isn't read here: mend it by hand", name.text))
            }
            Item::Value(n) => problem(at, format!("`{}` has no key", n.compact())),
        }
    }
}

/// A name or type as a word of its own, not a macro or a directive.
fn plain_word(w: &str) -> bool {
    !w.is_empty() && !w.starts_with(['$', '#'])
}

/// A `defaultPatch` entry's name and type: `defaultFaces` without a name, and `empty` without a
/// type, as OpenFOAM-8 has it (OpenFOAM-12 asks for the type).
fn default_patch(e: &umcf_foam_dict::Entry) -> Result<(String, String), String> {
    let plain = |t: &str| !t.is_empty() && !t.starts_with(['$', '#']);
    let body = e.dict().ok_or("its defaultPatch isn't a dictionary")?;
    if body.items.iter().any(|i| !matches!(i, Item::Entry(_))) {
        return Err("its defaultPatch has an #include or a macro".into());
    }
    let mut name = DEFAULT_FACES.to_string();
    let mut kind = "empty".to_string();
    for x in body.entries() {
        match x.key() {
            "name" => name = single(x).filter(|t| plain(t)).ok_or("its defaultPatch's name isn't plain")?,
            "type" => kind = single(x).filter(|t| plain(t)).ok_or("its defaultPatch's type isn't plain")?,
            k => return Err(format!("its defaultPatch has `{k}`, which the blocks don't hold")),
        }
    }
    Ok((name, kind))
}

/// A `blockMeshDict` read as a grid, or why it isn't one (curved edges, a block that isn't one
/// cell of a grid of stations, counts or gradings that differ along a span, a scale, a
/// directive or macro, a key the blocks don't hold).
pub fn import(doc: &Document) -> Result<Imported, String> {
    let mut vertices = None;
    let mut blocks = None;
    let mut boundary = None;
    let mut default: Option<(String, String)> = None;
    let mut seen = HashSet::new();
    for item in &doc.root.items {
        let e = match item {
            Item::Entry(e) => e,
            Item::Directive { name, .. } => return Err(format!("it has {}", name.text)),
            Item::Macro { name, .. } => return Err(format!("it has {}", name.text)),
            Item::Value(_) => return Err("it has a value without a key".into()),
        };
        let key = e.key();
        if !seen.insert(key) {
            return Err(format!("it gives {key} twice"));
        }
        let items = || match e.value.as_slice() {
            [Node::List { items, .. }] => Ok(items.as_slice()),
            _ => Err(format!("its {key} isn't a plain list")),
        };
        match key {
            "FoamFile" => {}
            "convertToMeters" | "scale" => match single(e).as_deref().and_then(parse_number) {
                Some(1.0) => {}
                _ => return Err(format!("its {key} isn't 1")),
            },
            "vertices" => vertices = Some(items()?),
            "blocks" => blocks = Some(items()?),
            "boundary" => boundary = Some(items()?),
            "edges" | "faces" | "mergePatchPairs" => {
                if !items()?.is_empty() {
                    return Err(format!("its {key} isn't empty"));
                }
            }
            "defaultPatch" => default = Some(default_patch(e)?),
            k => return Err(format!("it has `{k}`, which the blocks don't hold")),
        }
    }
    let vertices = vertices.ok_or("it has no vertices")?;
    let blocks = blocks.ok_or("it has no blocks")?;
    if vertices.len() > MAX_VERTICES {
        return Err(format!("it has more than {MAX_VERTICES} vertices"));
    }
    let mut points = Vec::with_capacity(vertices.len());
    for v in vertices {
        let Node::List { items, .. } = v else { return Err(format!("vertex `{}` isn't (x y z)", v.compact())) };
        match items.as_slice() {
            [x, y, z] => match (node_number(x), node_number(y), node_number(z)) {
                (Some(x), Some(y), Some(z)) => points.push([x, y, z]),
                _ => return Err(format!("vertex `{}` isn't three numbers", v.compact())),
            },
            _ => return Err(format!("vertex `{}` isn't (x y z)", v.compact())),
        }
    }
    // The blocks.
    let mut raw = Vec::new();
    let mut it = blocks.iter();
    while let Some(n) = it.next() {
        const SHAPE: &str = "a block isn't `hex (8 vertices) (cells) simpleGrading (…)`";
        if atom(n) != Some("hex") {
            return Err(format!("{SHAPE}: `{}`", n.compact()));
        }
        let Some(Node::List { items: vs, .. }) = it.next() else { return Err(SHAPE.into()) };
        let mut verts = [0usize; 8];
        if vs.len() != 8 {
            return Err(SHAPE.into());
        }
        for (slot, x) in verts.iter_mut().zip(vs) {
            *slot = index(x).filter(|i| *i < points.len()).ok_or("a block names a vertex that isn't listed")?;
        }
        let cells = match it.next() {
            Some(Node::List { items, .. }) if items.len() == 3 => {
                let mut c = [0u32; 3];
                for (slot, x) in c.iter_mut().zip(items) {
                    *slot = node_count(x).and_then(|x| u32::try_from(x).ok()).ok_or(SHAPE)?;
                }
                c
            }
            Some(Node::Atom(t)) => return Err(format!("a block names a cell zone ({})", t.text)),
            _ => return Err(SHAPE.into()),
        };
        let kind = it.next().and_then(atom);
        let Some(Node::List { items: g, .. }) = it.next() else { return Err(SHAPE.into()) };
        let grading: Vec<Grading> = match kind {
            Some("simpleGrading") if g.len() == 3 => {
                g.iter().map(Grading::read).collect::<Option<_>>().ok_or("a block's grading isn't numbers")?
            }
            Some("edgeGrading") if g.len() == 12 => {
                let all: Vec<Grading> =
                    g.iter().map(Grading::read).collect::<Option<_>>().ok_or("a block's grading isn't numbers")?;
                let mut three = Vec::new();
                for edges in all.chunks(4) {
                    let first = edges.first().ok_or(SHAPE)?;
                    if edges.iter().any(|x| x != first) {
                        return Err("a block's edgeGrading differs along one direction".into());
                    }
                    three.push(first.clone());
                }
                three
            }
            _ => return Err(SHAPE.into()),
        };
        let grading: [Grading; 3] = grading.try_into().map_err(|_| SHAPE.to_string())?;
        raw.push(RawBlock { verts, cells, grading });
        if raw.len() > MAX_BLOCKS {
            return Err(format!("it has more than {MAX_BLOCKS} blocks"));
        }
    }
    if raw.is_empty() {
        return Err("it has no blocks".into());
    }
    // blockMesh joins blocks by their vertex labels, not their points: two labels of one point
    // would keep a baffle between blocks that the grid would join.
    one_label_a_point(raw.iter().flat_map(|b| b.verts), &points)?;
    // The stations: the coordinates the blocks' vertices have.
    let mut stations: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for b in &raw {
        for v in b.verts {
            let p = points.get(v).ok_or("a block names a vertex that isn't listed")?;
            for a in 0..3 {
                stations[a].push(p[a]);
            }
        }
    }
    for s in &mut stations {
        s.sort_by(f64::total_cmp);
        s.dedup();
        if s.len() > MAX_STATIONS {
            return Err(format!("its vertices have more than {MAX_STATIONS} coordinates along an axis"));
        }
    }
    let counts = [stations[0].len() - 1, stations[1].len() - 1, stations[2].len() - 1];
    if counts.contains(&0) {
        return Err("its blocks are flat".into());
    }
    match counts[0].checked_mul(counts[1]).and_then(|x| x.checked_mul(counts[2])) {
        Some(n) if n <= MAX_BLOCKS => {}
        _ => return Err(format!("its grid would have more than {MAX_BLOCKS} blocks")),
    }
    let mut spans: [Vec<Option<Span>>; 3] = [vec![None; counts[0]], vec![None; counts[1]], vec![None; counts[2]]];
    let mut present: HashSet<[usize; 3]> = HashSet::new();
    for (bi, b) in raw.iter().enumerate() {
        let not_grid = || format!("block {bi} isn't one cell of a grid of stations along x, y and z");
        let mut at = [[0usize; 3]; 8];
        for (slot, v) in at.iter_mut().zip(b.verts) {
            let p = points.get(v).ok_or_else(not_grid)?;
            *slot = station_index(&stations, *p).ok_or_else(not_grid)?;
        }
        let origin = at[0];
        // Each local direction along one global axis, one station on.
        let mut dirs = [(0usize, true); 3];
        for (local, unit) in [1usize, 3, 4].into_iter().enumerate() {
            let to = at[unit];
            let moved: Vec<usize> = (0..3).filter(|g| to[*g] != origin[*g]).collect();
            let [g] = moved.as_slice() else { return Err(not_grid()) };
            let up = to[*g] == origin[*g] + 1;
            if !up && to[*g] + 1 != origin[*g] {
                return Err(format!("block {bi} spans more than one cell of the grid of stations"));
            }
            dirs[local] = (*g, up);
        }
        let mut axes_used = [dirs[0].0, dirs[1].0, dirs[2].0];
        axes_used.sort_unstable();
        if axes_used != [0, 1, 2] {
            return Err(not_grid());
        }
        for (c, local) in HEX.iter().enumerate() {
            let mut want = origin;
            for (l, (g, up)) in dirs.iter().enumerate() {
                if local[l] == 1 {
                    want[*g] = if *up { origin[*g] + 1 } else { origin[*g].checked_sub(1).ok_or_else(not_grid)? };
                }
            }
            if at[c] != want {
                return Err(not_grid());
            }
        }
        // Right-handed: the sign of the permutation times the directions'.
        let swaps =
            (0..3).flat_map(|i| (i + 1..3).map(move |j| (i, j))).filter(|(i, j)| dirs[*i].0 > dirs[*j].0).count();
        let downs = dirs.iter().filter(|(_, up)| !up).count();
        if (swaps + downs) % 2 == 1 {
            return Err(format!("block {bi} is inside out (its vertices go round the wrong way)"));
        }
        let mut cell = origin;
        for (l, (g, up)) in dirs.iter().enumerate() {
            if !up {
                cell[*g] = origin[*g].checked_sub(1).ok_or_else(not_grid)?;
            }
            // A block numbered against the axis: its grading seen from the axis' start.
            let grading = if *up {
                b.grading[l].clone()
            } else {
                b.grading[l]
                    .mirrored(b.cells[l])
                    .map_err(|why| format!("block {bi} runs against the {} axis: {why}", AXES[*g]))?
            };
            let span = Span { cells: b.cells[l], grading };
            let slot = spans[*g].get_mut(cell[*g]).ok_or_else(not_grid)?;
            match slot {
                Some(s) if *s != span => {
                    let lo = stations[*g].get(cell[*g]).copied().unwrap_or(0.0);
                    let hi = stations[*g].get(cell[*g] + 1).copied().unwrap_or(0.0);
                    return Err(format!(
                        "the blocks between {} {} and {} differ in cells or grading",
                        AXES[*g],
                        number(lo),
                        number(hi)
                    ));
                }
                Some(_) => {}
                None => *slot = Some(span),
            }
        }
        if !present.insert(cell) {
            return Err(format!("block {bi} fills the cell of another"));
        }
    }
    let mut axes: [Axis; 3] = Default::default();
    for a in 0..3 {
        let mut out = Vec::with_capacity(counts[a]);
        for (x, s) in spans[a].iter().enumerate() {
            match s {
                Some(s) => out.push(s.clone()),
                None => {
                    let lo = stations[a].get(x).copied().unwrap_or(0.0);
                    let hi = stations[a].get(x + 1).copied().unwrap_or(0.0);
                    return Err(format!(
                        "no block spans {} {} to {}: its cells can't be told",
                        AXES[a],
                        number(lo),
                        number(hi)
                    ));
                }
            }
        }
        axes[a] = Axis { stations: stations[a].clone(), spans: out };
    }
    let mut grid = BlockGrid { axes, ..BlockGrid::default() };
    for k in 0..counts[2] {
        for j in 0..counts[1] {
            for i in 0..counts[0] {
                if !present.contains(&[i, j, k]) {
                    grid.missing.push([i, j, k]);
                }
            }
        }
    }
    // The boundary: each face found by its corners.
    let mut types: Vec<(String, String)> = Vec::new();
    let mut names: HashSet<String> = HashSet::new();
    let mut given: HashMap<BlockFace, usize> = HashMap::new();
    let mut counted: Vec<usize> = Vec::new();
    // The faces no patch lists: blockMesh's `defaultPatch`, `defaultFaces` of type `empty`
    // unless the dictionary names and types it otherwise.
    let (default_name, default_type) = default.unwrap_or_else(|| (DEFAULT_FACES.to_string(), "empty".to_string()));
    let mut bit = boundary.unwrap_or_default().iter();
    while let Some(n) = bit.next() {
        let name = atom(n).map(unescaped).ok_or("its boundary isn't `name { type …; faces (…); }` …")?;
        if name == DEFAULT_FACES || name == default_name {
            return Err(format!("it lists {name}, its default patch, in its boundary"));
        }
        if !names.insert(name.clone()) {
            return Err(format!("its boundary lists {name} twice"));
        }
        let Some(Node::Dict { body, .. }) = bit.next() else {
            return Err("its boundary isn't `name { type …; faces (…); }` …".into());
        };
        if body.items.iter().any(|i| !matches!(i, Item::Entry(_))) {
            return Err(format!("patch {name} has a directive or macro"));
        }
        let mut kind = None;
        let mut faces: &[Node] = &[];
        for e in body.entries() {
            match e.key() {
                "type" => kind = single(e),
                "faces" => match e.value.as_slice() {
                    [Node::List { items, .. }] => faces = items,
                    _ => return Err(format!("the faces of patch {name} aren't a list")),
                },
                k => return Err(format!("patch {name} has `{k}`, which the blocks don't hold")),
            }
        }
        let kind = kind.filter(|k| !k.starts_with(['$', '#'])).ok_or(format!("patch {name} has no plain type"))?;
        let at = types.len();
        types.push((name.clone(), kind));
        counted.push(0);
        for f in faces {
            let Node::List { items, .. } = f else { return Err(format!("a face of patch {name} isn't (a b c d)")) };
            let mut corners = Vec::with_capacity(4);
            for x in items {
                let v =
                    index(x).and_then(|i| points.get(i)).ok_or(format!("a face of patch {name} names no vertex"))?;
                corners.push(station_index(&stations, *v).ok_or(format!("a face of patch {name} is off the grid"))?);
            }
            let face = face_at(&corners, counts, &present)
                .ok_or(format!("a face of patch {name} isn't a face on the boundary of the blocks"))?;
            if let Some(other) = given.insert(face, at) {
                let other = types.get(other).map(|(n, _)| n.as_str()).unwrap_or_default();
                return Err(format!("a face is in both {other} and {name}"));
            }
            if let Some(c) = counted.get_mut(at) {
                *c += 1;
            }
        }
    }
    // Each side's patch: the one most of its faces are in (the first listed of equals);
    // the others' faces listed singly.
    let all = grid.boundary_sides();
    let default_at = types.len();
    let mut majority: HashMap<Side, usize> = HashMap::new();
    for side in Side::ALL {
        let mut tally: BTreeMap<usize, usize> = BTreeMap::new();
        for (f, cat) in &all {
            if *cat == side {
                *tally.entry(given.get(f).copied().unwrap_or(default_at)).or_default() += 1;
            }
        }
        let best = tally.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(p, _)| *p);
        if let Some(p) = best {
            majority.insert(side, p);
        }
    }
    let name_of = |p: usize| types.get(p).map_or_else(|| default_name.clone(), |(n, _)| n.clone());
    for side in Side::ALL {
        if let Some(p) = majority.get(&side) {
            grid.sides.push((side, name_of(*p)));
        }
    }
    let mut singles: BTreeMap<usize, Vec<BlockFace>> = BTreeMap::new();
    for (f, cat) in &all {
        let p = given.get(f).copied().unwrap_or(default_at);
        if majority.get(cat) != Some(&p) {
            singles.entry(p).or_default().push(*f);
        }
    }
    for (p, mut fs) in singles {
        fs.sort_by_key(|f| (f.block[2], f.block[1], f.block[0], f.side));
        grid.faces.push((name_of(p), fs));
    }
    for (p, c) in counted.iter().enumerate() {
        if *c == 0 {
            grid.no_faces.push(name_of(p));
        }
    }
    // The default patch, when faces are left to it: under its own name when it has one.
    if all.iter().any(|(f, _)| !given.contains_key(f)) {
        types.push((default_name.clone(), default_type));
    }
    Ok(Imported { grid, types })
}

/// The block face whose corners are at the station indices `corners`, when it is on the
/// boundary of the blocks `present`.
fn face_at(corners: &[[usize; 3]], counts: [usize; 3], present: &HashSet<[usize; 3]>) -> Option<BlockFace> {
    if corners.len() != 4 {
        return None;
    }
    let first = corners.first()?;
    let flat: Vec<usize> = (0..3).filter(|a| corners.iter().all(|c| c[*a] == first[*a])).collect();
    let [g] = flat.as_slice() else { return None };
    let g = *g;
    let others: Vec<usize> = (0..3).filter(|a| *a != g).collect();
    let [p, q] = others.as_slice() else { return None };
    let lo = [corners.iter().map(|c| c[*p]).min()?, corners.iter().map(|c| c[*q]).min()?];
    let mut want: Vec<(usize, usize)> =
        vec![(lo[0], lo[1]), (lo[0] + 1, lo[1]), (lo[0], lo[1] + 1), (lo[0] + 1, lo[1] + 1)];
    let mut got: Vec<(usize, usize)> = corners.iter().map(|c| (c[*p], c[*q])).collect();
    want.sort_unstable();
    got.sort_unstable();
    if want != got {
        return None;
    }
    let plane = first[g];
    let cell = |x: Option<usize>| -> Option<[usize; 3]> {
        let x = x.filter(|x| *x < counts[g])?;
        let mut b = [0usize; 3];
        b[g] = x;
        b[*p] = lo[0];
        b[*q] = lo[1];
        Some(b)
    };
    let below = cell(plane.checked_sub(1)).filter(|b| present.contains(b));
    let above = cell(Some(plane)).filter(|b| present.contains(b));
    match (below, above) {
        (Some(b), None) => Some(BlockFace { block: b, side: side_of(g, true) }),
        (None, Some(b)) => Some(BlockFace { block: b, side: side_of(g, false) }),
        _ => None,
    }
}

/// The `blockMeshDict` a region's mesh reads: `system/<region>/blockMeshDict` for `mesh blocks`,
/// the dictionary the first `blockMesh` step names for a mesh built in `constant/polyMesh`.
pub fn dict_path(region: &str, build: &MeshBuild) -> Option<String> {
    match build {
        MeshBuild::Block => Some(format!("system/{region}/blockMeshDict")),
        MeshBuild::Root(steps) => steps.iter().find_map(|s| match s {
            RootStep::BlockMesh(d) => Some(d.clone()),
            _ => None,
        }),
    }
}

/// The type of each patch a dictionary's `boundary` lists (`name { type …; }`), whether or not
/// the rest of it is a grid.
pub fn listed_types(doc: &Document) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (n, t) in listed_in_order(doc) {
        out.entry(n).or_insert(t);
    }
    out
}

/// The type of each patch of region `r` the blocks write, as [`type_for`] gives it, with the
/// lookups made once (a description may name many patches).
pub fn type_lookup(r: &RegionDesc) -> impl Fn(&str) -> String + '_ {
    let given: HashMap<&str, &str> = r.blocks.as_ref().map(BlockGrid::type_map).unwrap_or_default();
    let roles: HashMap<&str, Option<Role>> = r.patches.iter().map(|p| (p.name.as_str(), p.role)).collect();
    move |name: &str| {
        if let Some(t) = given.get(name) {
            return (*t).to_string();
        }
        if name == DEFAULT_FACES {
            return "empty".into();
        }
        patch_type(r.kind, roles.get(name).copied().flatten()).to_string()
    }
}

/// The type a patch of region `r` is written with: the one its blocks' `types` give it, else
/// for blockMesh's own `defaultFaces` `empty`, else the one its role gives ([`patch_type`]),
/// else `patch`.
pub fn type_for(r: &RegionDesc, name: &str) -> String {
    type_lookup(r)(name)
}

/// Region `r`'s `blockMeshDict` written from its blocks: the path and the text. Refused, with
/// why, without blocks, without a mesh that reads a `blockMeshDict`, or for a grid too large.
pub fn generate(r: &RegionDesc) -> Result<(String, String), String> {
    let grid = r.blocks.as_ref().ok_or_else(|| format!("region {} has no blocks", r.name))?;
    let path = r
        .mesh
        .as_ref()
        .and_then(|m| dict_path(&r.name, m))
        .ok_or_else(|| format!("region {} has no mesh that reads a blockMeshDict", r.name))?;
    let location = path.rsplit_once('/').map_or("system", |(d, _)| d).to_string();
    let type_of = type_lookup(r);
    let text = grid.block_mesh_dict(&r.name, &location, &type_of)?;
    Ok((path, text))
}

/// A patch a `blockMeshDict` makes: its name, its type, and the entries it gives besides
/// `type` and `faces` (`neighbourPatch`, `inGroups`, …), which the blocks don't hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictPatch {
    pub name: String,
    pub kind: String,
    pub extra: Vec<String>,
}

/// The patches a `blockMeshDict` makes, as [`dict_patches`] reads them, and what it noted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DictPatches {
    pub patches: Vec<DictPatch>,
    /// What blockMesh does otherwise than one would read it (a `boundary` it ignores, a
    /// `defaultPatch` without a type).
    pub notes: Vec<String>,
}

/// The labels of a hex's six faces, as OpenFOAM's hex model has them.
const HEX_FACES: [[usize; 4]; 6] = [[0, 4, 7, 3], [1, 2, 6, 5], [0, 1, 5, 4], [3, 7, 6, 2], [0, 3, 2, 1], [4, 5, 6, 7]];

/// Whether blockMesh leaves faces to its `defaultPatch`: the faces of the dictionary's hexes
/// that no other hex shares (by their vertex labels, as blockMesh's topology matches them) and
/// no patch lists (`listed`, each face's labels sorted). Refused, with why, when that can't be
/// told: vertices or blocks that can't be read, a block that isn't a plain hex, two labels of
/// one point (blockMesh joins blocks by labels only, so it keeps a baffle there, two boundary
/// faces in its default patch; whether the dictionary means one can't be told, so it is left to
/// `unreadPatches`), a face shared by more than two blocks.
fn faces_left(doc: &Document, listed: &HashSet<[usize; 4]>) -> Result<bool, String> {
    let list = |key: &str| -> Result<&[Node], String> {
        match doc.get(&[key]).map(|e| e.value.as_slice()) {
            Some([Node::List { items, .. }]) => Ok(items.as_slice()),
            _ => Err(format!("its {key} can't be read")),
        }
    };
    let vertices = list("vertices")?;
    if vertices.len() > MAX_VERTICES {
        return Err(format!("it has more than {MAX_VERTICES} vertices"));
    }
    let mut points = Vec::with_capacity(vertices.len());
    for v in vertices {
        let Node::List { items, .. } = v else { return Err("a vertex can't be read".into()) };
        let p: Vec<u64> = items.iter().filter_map(node_number).map(|x| (x + 0.0).to_bits()).collect();
        let [x, y, z] = p.as_slice() else { return Err("a vertex isn't three numbers".into()) };
        points.push([*x, *y, *z]);
    }
    let mut used: HashSet<usize> = HashSet::new();
    let mut count: HashMap<[usize; 4], u8> = HashMap::new();
    let mut it = list("blocks")?.iter().peekable();
    while let Some(n) = it.next() {
        if atom(n) != Some("hex") {
            return Err(format!("a block isn't a hex: `{}`", n.compact()));
        }
        let Some(Node::List { items: vs, .. }) = it.next() else {
            return Err("a block's vertices can't be read".into());
        };
        let labels: Vec<usize> = vs.iter().filter_map(index).filter(|i| *i < vertices.len()).collect();
        let [_, _, _, _, _, _, _, _] = labels.as_slice() else {
            return Err("a block isn't eight vertices of the list".into());
        };
        used.extend(labels.iter().copied());
        // A cell zone's name, the cells, the grading's kind and the grading.
        if matches!(it.peek(), Some(Node::Atom(_))) {
            it.next();
        }
        for _ in 0..3 {
            if it.next().is_none() {
                return Err("a block ends too soon".into());
            }
        }
        for f in HEX_FACES {
            let mut key = f.map(|i| labels.get(i).copied().unwrap_or(0));
            key.sort_unstable();
            let c = count.entry(key).or_insert(0);
            *c = c.saturating_add(1);
            if *c > 2 {
                return Err("a face is shared by more than two blocks".into());
            }
        }
        if count.len() > 6 * MAX_VERTICES {
            return Err("it has too many blocks".into());
        }
    }
    // Two labels of one point: a baffle in blockMesh (see above).
    let points: Vec<[f64; 3]> = points.iter().map(|p| p.map(f64::from_bits)).collect();
    one_label_a_point(used.into_iter(), &points)?;
    Ok(count.iter().any(|(k, c)| *c == 1 && !listed.contains(k)))
}

/// The first 20 of `items`, and how many more.
pub fn list_some(items: &[&str]) -> String {
    let mut s = items.iter().take(20).copied().collect::<Vec<_>>().join(", ");
    if items.len() > 20 {
        s.push_str(&format!(", and {} more", items.len() - 20));
    }
    s
}

/// Refused when the vertices `labels` (of `points`) name one point under two labels.
fn one_label_a_point(labels: impl Iterator<Item = usize>, points: &[[f64; 3]]) -> Result<(), String> {
    let mut used: Vec<usize> = labels.collect();
    used.sort_unstable();
    used.dedup();
    let mut seen: HashMap<[u64; 3], usize> = HashMap::new();
    for i in used {
        if let Some(p) = points.get(i)
            && let Some(j) = seen.insert(p.map(|x| (x + 0.0).to_bits()), i)
        {
            return Err(format!(
                "its blocks use vertices {j} and {i}, one point under two labels: blockMesh joins blocks by labels, and keeps a baffle there"
            ));
        }
    }
    Ok(())
}

/// A patch face's labels, sorted.
fn face_key(n: &Node) -> Option<[usize; 4]> {
    let Node::List { items, .. } = n else { return None };
    let v: Vec<usize> = items.iter().filter_map(index).collect();
    let mut k: [usize; 4] = v.as_slice().try_into().ok()?;
    if v.len() != items.len() {
        return None;
    }
    k.sort_unstable();
    Some(k)
}

/// The patches a `blockMeshDict` makes, in its order: those of `boundary`, or of the older
/// `patches ( type name ( faces ) … )` (which blockMesh reads instead when both are there);
/// and its `defaultPatch` (named `defaultFaces` and typed `empty` unless it says otherwise)
/// when blockMesh leaves faces to it. Refused, with why, when they can't all be read: an
/// `#include` or a macro at its top or in a patch, a type given by a macro, a list of another
/// shape, a name given twice, or blocks whose faces left to the default patch can't be told.
pub fn dict_patches(doc: &Document) -> Result<DictPatches, String> {
    let plain = |t: &str| !t.is_empty() && !t.starts_with(['$', '#']);
    let mut out = DictPatches::default();
    for item in &doc.root.items {
        match item {
            Item::Entry(_) => {}
            Item::Directive { name, .. } | Item::Macro { name, .. } => {
                return Err(format!("it has {} at its top", name.text));
            }
            Item::Value(_) => return Err("it has a value without a key".into()),
        }
    }
    let mut keys: HashSet<&str> = HashSet::new();
    for k in doc.root.entries().map(|e| e.key()) {
        if !keys.insert(k) {
            return Err(format!("it gives {k} twice"));
        }
    }
    let list = |key: &str| -> Result<Option<&[Node]>, String> {
        match doc.get(&[key]).map(|e| e.value.as_slice()) {
            None => Ok(None),
            Some([Node::List { items, .. }]) => Ok(Some(items.as_slice())),
            Some(_) => Err(format!("its {key} isn't a plain list")),
        }
    };
    let mut listed: HashSet<[usize; 4]> = HashSet::new();
    let mut names: HashSet<String> = HashSet::new();
    let mut faces_of = |faces: &[Node], name: &str| -> Result<(), String> {
        for f in faces {
            listed.insert(face_key(f).ok_or_else(|| format!("a face of patch {name} isn't four vertex labels"))?);
        }
        Ok(())
    };
    let legacy = list("patches")?;
    match legacy {
        Some(items) => {
            if doc.get(&["boundary"]).is_some() {
                out.notes.push("it has both patches and boundary: blockMesh reads patches, and so is it read".into());
            }
            if items.len() % 3 != 0 {
                return Err("its patches isn't `type name ( faces )` triples".into());
            }
            for three in items.chunks(3) {
                let [Node::Atom(kind), Node::Atom(name), Node::List { items: faces, .. }] = three else {
                    return Err("its patches isn't `type name ( faces )` triples".into());
                };
                let (kind, name) = (unescaped(&kind.text), unescaped(&name.text));
                if !plain(&kind) || !plain(&name) {
                    return Err(format!("its patches give `{kind} {name}` by a macro"));
                }
                if !names.insert(name.clone()) {
                    return Err(format!("its patches name {name} twice"));
                }
                faces_of(faces, &name)?;
                out.patches.push(DictPatch { name, kind, extra: Vec::new() });
            }
        }
        None => {
            if let Some(items) = list("boundary")? {
                if items.len() % 2 != 0 {
                    return Err("its boundary isn't `name { type …; … }` pairs".into());
                }
                for pair in items.chunks(2) {
                    let [Node::Atom(name), Node::Dict { body, .. }] = pair else {
                        return Err("its boundary isn't `name { type …; … }` pairs".into());
                    };
                    let name = unescaped(&name.text);
                    if !plain(&name) {
                        return Err(format!("a patch of its boundary is named by `{name}`"));
                    }
                    if !names.insert(name.clone()) {
                        return Err(format!("its boundary names {name} twice"));
                    }
                    if body.items.iter().any(|i| !matches!(i, Item::Entry(_))) {
                        return Err(format!("patch {name} has an #include or a macro"));
                    }
                    let kind = body
                        .get("type")
                        .and_then(single)
                        .filter(|t| plain(t))
                        .ok_or_else(|| format!("patch {name} has no plain type"))?;
                    match body.get("faces").map(|e| e.value.as_slice()) {
                        None => {}
                        Some([Node::List { items: faces, .. }]) => faces_of(faces, &name)?,
                        Some(_) => return Err(format!("the faces of patch {name} aren't a plain list")),
                    }
                    let extra = body.keys().filter(|k| *k != "type" && *k != "faces").map(str::to_string).collect();
                    out.patches.push(DictPatch { name, kind, extra });
                }
            }
        }
    }
    let (name, kind) = match doc.get(&["defaultPatch"]) {
        Some(e) => {
            let (name, kind) = default_patch(e)?;
            if e.dict().is_some_and(|d| d.get("type").is_none()) {
                out.notes.push(format!(
                    "its defaultPatch has no type: {kind}, as OpenFOAM-8 takes it (OpenFOAM-12 asks for one)"
                ));
            }
            (name, kind)
        }
        None => (DEFAULT_FACES.to_string(), "empty".to_string()),
    };
    if faces_left(doc, &listed)? {
        if names.contains(&name) {
            return Err(format!("its default patch {name} is a patch it lists too"));
        }
        out.patches.push(DictPatch { name, kind, extra: Vec::new() });
    }
    Ok(out)
}

/// What a `blockMeshDict` holds besides its vertices, blocks and patches, which the blocks
/// don't: cell zones named by blocks, curved `edges`, projected `faces`, `geometry`,
/// `mergePatchPairs`, a scale other than 1, and any other key at its top (`fastMerge`,
/// `checkBlockFaceOrientation`, `namedVertices`, a variable of its own, …); blocks that can't
/// all be read for their zones are said so. Each said in a few words.
pub fn dict_settings(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    let nonempty = |key: &str| match doc.get(&[key]).map(|e| e.value.as_slice()) {
        None => false,
        Some([Node::List { items, .. }]) => !items.is_empty(),
        Some(_) => true,
    };
    for (key, what) in [
        ("edges", "curved edges (edges)"),
        ("faces", "projected faces (faces)"),
        ("mergePatchPairs", "patches merged (mergePatchPairs)"),
    ] {
        if nonempty(key) {
            out.push(what.to_string());
        }
    }
    for key in ["convertToMeters", "scale"] {
        if let Some(e) = doc.get(&[key])
            && single(e).as_deref().and_then(parse_number) != Some(1.0)
        {
            out.push(format!("a scale ({key} {})", e.value_text().unwrap_or_default().trim()));
        }
    }
    // Any other key at its top, said by name.
    let known = [
        "FoamFile",
        "convertToMeters",
        "scale",
        "vertices",
        "blocks",
        "edges",
        "faces",
        "mergePatchPairs",
        "boundary",
        "patches",
        "defaultPatch",
    ];
    let mut seen: HashSet<&str> = HashSet::new();
    let mut others: Vec<&str> = Vec::new();
    let mut twice: Vec<&str> = Vec::new();
    let mut twice_set: HashSet<&str> = HashSet::new();
    for k in doc.root.entries().map(|e| e.key()) {
        if !seen.insert(k) {
            if twice_set.insert(k) {
                twice.push(k);
            }
        } else if !known.contains(&k) {
            others.push(k);
        }
    }
    if !others.is_empty() {
        out.push(format!("other entries ({})", list_some(&others)));
    }
    if !twice.is_empty() {
        out.push(format!("entries given twice ({})", list_some(&twice)));
    }
    // Expressions in the vertices or blocks: a macro, #calc, a #{ … #} block.
    let expr = |n: &Node| -> bool {
        fn walk(n: &Node) -> bool {
            match n {
                Node::Atom(t) => t.text.starts_with(['$', '#']),
                Node::List { items, .. } | Node::Dims { items, .. } => items.iter().any(walk),
                Node::Dict { .. } => true,
            }
        }
        walk(n)
    };
    for key in ["vertices", "blocks"] {
        if doc.get(&[key]).is_some_and(|e| e.value.iter().any(expr)) {
            out.push(format!("expressions in its {key} ($macros, #calc)"));
        }
    }
    // Cell zones: a block's name between its vertices and its cells, read block by block.
    match doc.get(&["blocks"]).map(|e| e.value.as_slice()) {
        None => {}
        Some([Node::List { items, .. }]) => {
            let mut zones: Vec<String> = Vec::new();
            let mut zone_set: HashSet<String> = HashSet::new();
            let mut whole = true;
            let mut it = items.iter().peekable();
            while let Some(n) = it.next() {
                if atom(n) != Some("hex") || !matches!(it.next(), Some(Node::List { .. })) {
                    whole = false;
                    break;
                }
                if let Some(Node::Atom(z)) = it.peek() {
                    if zone_set.insert(z.text.clone()) {
                        zones.push(z.text.clone());
                    }
                    it.next();
                }
                let shape = matches!(it.next(), Some(Node::List { .. }))
                    && matches!(it.next().and_then(atom), Some("simpleGrading" | "edgeGrading"))
                    && matches!(it.next(), Some(Node::List { .. }));
                if !shape {
                    whole = false;
                    break;
                }
            }
            if !zones.is_empty() {
                let zones: Vec<&str> = zones.iter().map(String::as_str).collect();
                out.push(format!("cell zones of blocks ({})", list_some(&zones)));
            }
            if !whole {
                out.push("blocks not fully read (cell zones in them may be lost)".into());
            }
        }
        Some(_) => out.push("blocks not fully read (cell zones in them may be lost)".into()),
    }
    out
}

/// The patches whose type `new` gives otherwise than `old` does: each with its type in `old`
/// and in `new`, in `new`'s order.
pub fn type_changes(old: &[DictPatch], new: &[DictPatch]) -> Vec<(String, String, String)> {
    let before: HashMap<&str, &str> = old.iter().map(|p| (p.name.as_str(), p.kind.as_str())).collect();
    new.iter()
        .filter_map(|p| {
            let was = before.get(p.name.as_str())?;
            (unquote(was) != unquote(&p.kind)).then(|| (p.name.clone(), was.to_string(), p.kind.clone()))
        })
        .collect()
}

/// The patches a dictionary's `boundary` lists with their types, in its order.
pub fn listed_in_order(doc: &Document) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some([Node::List { items, .. }]) = doc.get(&["boundary"]).map(|e| e.value.as_slice()) else { return out };
    for pair in items.chunks(2) {
        if let [Node::Atom(name), Node::Dict { body, .. }] = pair
            && let Some(t) = body.get("type").and_then(single)
        {
            out.push((unescaped(&name.text), t));
        }
    }
    out
}

/// The `blockMeshDict` a region's blocks are written to, as the case has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictState {
    /// Not there.
    Missing,
    /// There, but not loaded as a dictionary (too large, or not one).
    NotLoaded,
    /// It makes the mesh the blocks make, its patches of the types the blocks give them.
    Same,
    /// It makes another mesh (or isn't a grid); `untouched`: umcfGui wrote it and nobody changed
    /// it since ([`untouched`]), so it can be written over without losing an edit.
    /// `types_only`: the same blocks, only a patch's type isn't the one the blocks' `types` or
    /// its role give now (a role given since, say).
    Differs { untouched: bool, why: String, types_only: bool },
}

/// What the `blockMeshDict` of region `r` is against its blocks: its path and state. `None`
/// when `r` has no blocks or no mesh that reads one.
pub fn dict_state(case: &Case, r: &RegionDesc) -> Option<(String, DictState)> {
    let grid = r.blocks.as_ref()?;
    let path = dict_path(&r.name, r.mesh.as_ref()?)?;
    let Some(doc) = case.dict(&path) else {
        let state = if case.exists(&path) == Some(true) { DictState::NotLoaded } else { DictState::Missing };
        return Some((path, state));
    };
    let untouched = untouched(&doc.to_string());
    let state = match import(doc) {
        Err(why) => {
            DictState::Differs { untouched, why: format!("it isn't a grid of blocks: {why}"), types_only: false }
        }
        Ok(imp) if !imp.grid.same_mesh(grid) => DictState::Differs {
            untouched,
            why: "its blocks or patches aren't the description's".into(),
            types_only: false,
        },
        Ok(imp) => {
            let type_of = type_lookup(r);
            let wrong = imp.types.iter().find_map(|(n, t)| {
                let want = type_of(n);
                (unquote(t) != want).then(|| format!("patch {n} is {t} there, the blocks make it {want}"))
            });
            match wrong {
                Some(why) => DictState::Differs { untouched, why, types_only: true },
                None => DictState::Same,
            }
        }
    };
    Some((path, state))
}

/// Whether `text` is a file umcfGui generated and nobody changed since: its header says so,
/// and the checksum there is that of the rest.
pub fn untouched(text: &str) -> bool {
    let Some(at) = text.find(CHECKSUM) else { return false };
    if !text.get(..at).is_some_and(|head| head.contains(GENERATED)) {
        return false;
    }
    let rest = text.get(at + CHECKSUM.len()..).unwrap_or_default();
    let (line, body) = rest.split_once('\n').unwrap_or((rest, ""));
    let given = line.split_whitespace().next().and_then(|h| u64::from_str_radix(h, 16).ok());
    given == Some(checksum(body.as_bytes()))
}

#[cfg(test)]
#[path = "blocks_tests.rs"]
mod tests;
