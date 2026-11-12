//! A box around the case's surfaces (gaps row 5): a region's `box` entry in the workflow
//! description, which its blocks are made from, one block from the surfaces' bounds plus a
//! margin on each side, with a cell size or the cells along each axis.
//!
//! ```c++
//! box
//! {
//!     surfaces ( buildings.stl trees.stl );   // in constant/triSurface
//!     margin   ( 100 200 50 50 0 60 );        // west east south north ground top
//!     cellSize 2;                             // or cells ( 200 75 40 );
//! }
//! ```
//!
//! The margins are on the sides as the blocks name them: `west`/`east` along x, `south`/`north`
//! along y, `ground`/`top` along z. With a cell size the cells are cubes: the box grows by less
//! than a cell at the far end of each axis (east, north, top), and along the gravity away from
//! the ground (the side the gravity points to), so that each axis is a whole number of cells.

use std::collections::HashSet;
use std::fmt::Write as _;

use umcf_foam_dict::{Dict, Node};

use crate::blocks::{Axis, Grading, Side, Span, node_count, node_number, non_entries, number};
use crate::description::{atom, list, twice, unescaped, word};

/// The most cells a box may have: a cell size in the wrong unit (millimetres) would otherwise
/// write a dictionary blockMesh can't mesh in any memory.
pub const MAX_CELLS: u64 = 1_000_000_000;

/// The most surfaces a box may name.
pub const MAX_SURFACES: usize = 1_000;

/// The sides of the margins, in their order.
pub const SIDES: [&str; 6] = ["west", "east", "south", "north", "ground", "top"];

/// How the box is cut into cells.
#[derive(Debug, Clone, PartialEq)]
pub enum BoxCells {
    /// Cubes of this size; the box grows by less than a cell at the far end of each axis, away
    /// from the ground along the gravity.
    Size(f64),
    /// The cells along x, y and z; the box is as the bounds and margins make it.
    Counts([u32; 3]),
}

/// A region's `box`: the surfaces whose bounds it is around, its margins and its cells.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshBox {
    /// Surfaces in `constant/triSurface`, by file name.
    pub surfaces: Vec<String>,
    /// West, east, south, north, ground, top: how far the box reaches past the bounds.
    pub margin: [f64; 6],
    pub cells: BoxCells,
    /// The checksum of the blocks' shape (stations, cells, gradings, missing blocks, single
    /// faces: [`crate::blocks::shape_checksum`]) the box last made, written by `mesh.box`:
    /// blocks with another were edited since.
    pub made: Option<u64>,
}

impl MeshBox {
    /// What is wrong with the box itself (not with its surfaces), as `(key, message)`.
    pub fn problems(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if self.surfaces.is_empty() {
            out.push(("surfaces", "names no surface".to_string()));
        }
        if self.surfaces.len() > MAX_SURFACES {
            out.push(("surfaces", format!("names {} surfaces, {MAX_SURFACES} at most", self.surfaces.len())));
        }
        let mut seen = HashSet::new();
        if let Some(bad) = self.surfaces.iter().find(|s| !is_surface_file(s)) {
            out.push((
                "surfaces",
                format!("{bad} isn't an STL file of constant/triSurface (a file name ending .stl, .stlb, .stl.gz or .stlb.gz)"),
            ));
        }
        for s in &self.surfaces {
            if !seen.insert(s.as_str()) {
                out.push(("surfaces", format!("names {s} twice")));
                break;
            }
        }
        for (m, side) in self.margin.iter().zip(SIDES) {
            if !(m.is_finite() && *m >= 0.0) {
                out.push(("margin", format!("{side}: {} isn't a distance of 0 or more", number(*m))));
            }
        }
        match &self.cells {
            BoxCells::Size(h) if !(h.is_finite() && *h > 0.0) => {
                out.push(("cellSize", format!("{} isn't a size above 0", number(*h))));
            }
            BoxCells::Counts(n) if n.contains(&0) => out.push(("cells", "has an axis without cells".to_string())),
            _ => {}
        }
        out
    }

    /// The three axes of the box around the bounds `min`..`max`: one span each, its grading 1.
    /// An error when the box is empty along an axis, isn't finite, or has more than
    /// [`MAX_CELLS`] cells.
    /// `ground`: the side the gravity points to, which stays where the margin puts it.
    pub fn axes(&self, min: [f64; 3], max: [f64; 3], ground: Side) -> Result<[Axis; 3], String> {
        if let Some((k, m)) = self.problems().first() {
            return Err(format!("{k} {m}"));
        }
        let mut axes: [Axis; 3] = Default::default();
        let mut total: u64 = 1;
        for (a, axis) in axes.iter_mut().enumerate() {
            let name = ["x", "y", "z"].get(a).copied().unwrap_or("?");
            let (lo_m, hi_m) = (self.margin.get(2 * a).copied(), self.margin.get(2 * a + 1).copied());
            let (Some(lo_m), Some(hi_m), Some(lo), Some(hi)) = (lo_m, hi_m, min.get(a), max.get(a)) else {
                return Err("the bounds have three axes".to_string());
            };
            let mut lo = lo - lo_m;
            let mut hi = hi + hi_m;
            // The ground at the far end of its axis: the box grows at the near end there.
            let from_hi = ground.axis() == Some((a, true));
            let length = hi - lo;
            if !(lo.is_finite() && hi.is_finite() && length.is_finite()) {
                return Err(format!("along {name} the box isn't finite"));
            }
            if length <= 0.0 {
                return Err(format!(
                    "along {name} the box has no length (the surfaces are flat along it and its margins are 0): give it a margin"
                ));
            }
            let cells = match &self.cells {
                BoxCells::Counts(n) => u64::from(n.get(a).copied().unwrap_or(1)),
                BoxCells::Size(h) => {
                    let q = length / h;
                    // A length within rounding of a whole number of cells is that number, not
                    // one more (coordinates far from the origin lose digits in `hi - lo`).
                    let n = if (q - q.round()).abs() <= 1e-9 * q.max(1.0) { q.round() } else { q.ceil() };
                    if !(n.is_finite() && n >= 1.0 && n <= MAX_CELLS as f64) {
                        return Err(format!(
                            "along {name} a cell size of {} makes more than {MAX_CELLS} cells",
                            number(*h)
                        ));
                    }
                    // Within MAX_CELLS, so exact as an integer.
                    let n = n as u64;
                    // Never inside the bounds plus margins: a cell within rounding of the size
                    // keeps the end where they put it.
                    if from_hi {
                        lo = lo.min(hi - n as f64 * h);
                    } else {
                        hi = hi.max(lo + n as f64 * h);
                    }
                    if !(lo.is_finite() && hi.is_finite()) {
                        return Err(format!("along {name} the box isn't finite"));
                    }
                    n
                }
            };
            total = total.saturating_mul(cells);
            if total > MAX_CELLS {
                return Err(format!(
                    "the box would have more than {MAX_CELLS} cells: a larger cell size, fewer cells or smaller margins"
                ));
            }
            let cells = u32::try_from(cells).map_err(|_| format!("along {name} too many cells"))?;
            *axis = Axis { stations: vec![lo, hi], spans: vec![Span { cells, grading: Grading::Ratio(1.0) }] };
        }
        Ok(axes)
    }

    /// `( buildings.stl trees.stl )`.
    pub fn surfaces_text(&self) -> String {
        let parts: Vec<String> = self.surfaces.iter().map(|s| word(s)).collect();
        if parts.is_empty() { "( )".to_string() } else { format!("( {} )", parts.join(" ")) }
    }

    /// `( 100 200 50 50 0 60 )`.
    pub fn margin_text(&self) -> String {
        let parts: Vec<String> = self.margin.iter().map(|m| number(*m)).collect();
        format!("( {} )", parts.join(" "))
    }

    /// The keys of the `box` entry with their values; `None` for the one of the two ways of
    /// cutting it into cells it doesn't take.
    pub fn parts(&self) -> Vec<(&'static str, Option<String>)> {
        let (size, counts) = match &self.cells {
            BoxCells::Size(h) => (Some(number(*h)), None),
            BoxCells::Counts(n) => (None, Some(format!("( {} {} {} )", n[0], n[1], n[2]))),
        };
        vec![
            ("surfaces", Some(self.surfaces_text())),
            ("margin", Some(self.margin_text())),
            ("cellSize", size),
            ("cells", counts),
            ("made", self.made.map(|c| c.to_string())),
        ]
    }

    /// The `box` entry as the description writes it, indented by `indent`.
    pub fn entry(&self, indent: &str) -> String {
        let mut s = format!("{indent}box\n{indent}{{\n");
        for (k, v) in self.parts() {
            if let Some(v) = v {
                let _ = writeln!(s, "{indent}    {k:<9}{v};");
            }
        }
        let _ = writeln!(s, "{indent}}}");
        s
    }

    /// The box read from a `box` entry's dictionary; what can't be read is a problem at
    /// `at/<key>`. `None` when a key it needs is missing or doesn't read.
    pub(crate) fn read(body: &Dict, at: &str, problem: &mut impl FnMut(&str, String)) -> Option<MeshBox> {
        let mut surfaces = None;
        let mut margin = None;
        let mut size = None;
        let mut counts = None;
        let mut made = None;
        let mut ok = true;
        let mut seen = HashSet::new();
        non_entries(body, at, problem);
        for e in body.entries() {
            let here = format!("{at}/{}", e.key());
            if twice(&mut seen, e, &here, problem) {
                continue;
            }
            match e.key() {
                "surfaces" => {
                    let names: Option<Vec<String>> = list(e).and_then(|items| {
                        (items.len() <= MAX_SURFACES).then(|| items.iter().map(name_of).collect())?
                    });
                    match names {
                        Some(n) => surfaces = Some(n),
                        None => {
                            ok = false;
                            problem(
                                &here,
                                format!(
                                    "is a list of the surfaces' file names, at most {MAX_SURFACES}: ( buildings.stl … )"
                                ),
                            );
                        }
                    }
                }
                "margin" => {
                    let six: Option<Vec<f64>> =
                        list(e).filter(|i| i.len() == 6).and_then(|i| i.iter().map(node_number).collect());
                    match six.as_deref() {
                        Some([w, ea, s, n, g, t]) => margin = Some([*w, *ea, *s, *n, *g, *t]),
                        _ => {
                            ok = false;
                            problem(&here, "is six distances: ( west east south north ground top )".into());
                        }
                    }
                }
                "cellSize" => match e.value.as_slice() {
                    [n] if node_number(n).is_some() => size = node_number(n),
                    _ => {
                        ok = false;
                        problem(&here, "is a number: the cells' size".into());
                    }
                },
                "cells" => {
                    let three: Option<Vec<u32>> = list(e)
                        .filter(|i| i.len() == 3)
                        .and_then(|i| i.iter().map(|n| node_count(n).and_then(|c| u32::try_from(c).ok())).collect());
                    match three.as_deref() {
                        Some([x, y, z]) => counts = Some([*x, *y, *z]),
                        _ => {
                            ok = false;
                            problem(&here, "is the cells along x, y and z: ( 200 75 40 )".into());
                        }
                    }
                }
                "made" => match e.value.as_slice() {
                    [n] if node_count(n).is_some() => made = node_count(n),
                    _ => {
                        ok = false;
                        problem(&here, "is the checksum of the blocks the box made (a whole number)".into());
                    }
                },
                _ => problem(&here, "isn't a key of a box: surfaces, margin, cellSize, cells or made".into()),
            }
        }
        let cells = match (size, counts) {
            (Some(h), None) => Some(BoxCells::Size(h)),
            (None, Some(n)) => Some(BoxCells::Counts(n)),
            (Some(_), Some(_)) => {
                problem(at, "gives cellSize and cells: one of the two".into());
                None
            }
            (None, None) => {
                if ok {
                    problem(at, "has no cellSize or cells".into());
                }
                None
            }
        };
        for (k, v) in [("surfaces", surfaces.is_some()), ("margin", margin.is_some())] {
            if !v && ok && !seen.contains(k) {
                problem(at, format!("has no {k}"));
            }
        }
        Some(MeshBox { surfaces: surfaces?, margin: margin?, cells: cells?, made })
    }
}

/// Whether `name` is a file of `constant/triSurface` OpenFOAM reads as STL: no directory, not
/// hidden, one of its STL extensions (in lower case, as OpenFOAM matches them).
pub fn is_surface_file(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
        && [".stl", ".stlb", ".stl.gz", ".stlb.gz"].iter().any(|e| name.len() > e.len() && name.ends_with(e))
}

/// A surface's file name in a `surfaces` list: a word, or a string (which may start with `$`
/// or `#`, as the description writes such a name; a word that does is a macro or directive).
fn name_of(n: &Node) -> Option<String> {
    let t = atom(n)?;
    let quoted = t.len() >= 2 && t.starts_with('"') && t.ends_with('"');
    let s = unescaped(t);
    (!s.is_empty() && (quoted || !s.starts_with(['$', '#']))).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_of(margin: [f64; 6], cells: BoxCells) -> MeshBox {
        MeshBox { surfaces: vec!["buildings.stl".to_string()], margin, cells, made: None }
    }

    fn read(text: &str) -> (Option<MeshBox>, Vec<String>) {
        let doc = umcf_foam_dict::Document::parse(text).unwrap();
        let body = doc.get(&["box"]).unwrap().dict().unwrap();
        let mut problems = Vec::new();
        let b = MeshBox::read(body, "box", &mut |at: &str, m: String| problems.push(format!("{at}: {m}")));
        (b, problems)
    }

    #[test]
    fn a_cell_size_makes_cubes_and_grows_the_far_sides() {
        let b = box_of([10.0, 20.0, 5.0, 5.0, 0.0, 30.0], BoxCells::Size(2.0));
        let [x, y, z] = b.axes([0.0, 0.0, 0.0], [51.0, 10.0, 15.0], Side::Ground).unwrap();
        // x: -10 .. 71, 81 long: 41 cells of 2, so to 72.
        assert_eq!(x.stations, [-10.0, 72.0]);
        assert_eq!(x.spans[0].cells, 41);
        assert_eq!(y.stations, [-5.0, 15.0]);
        assert_eq!(y.spans[0].cells, 10);
        // The ground stays at the bounds' bottom.
        assert_eq!(z.stations, [0.0, 46.0]);
        assert_eq!(z.spans[0].cells, 23);
        assert_eq!(z.spans[0].grading, Grading::Ratio(1.0));
    }

    #[test]
    fn the_box_grows_away_from_the_ground() {
        // The gravity along +y: the ground is north, the box grows south.
        let b = box_of([0.0, 0.0, 1.0, 0.0, 0.0, 0.0], BoxCells::Size(2.0));
        let [_, y, z] = b.axes([0.0; 3], [2.0, 9.5, 2.0], Side::North).unwrap();
        assert_eq!(y.stations, [-2.5, 9.5]);
        assert_eq!(z.stations, [0.0, 2.0]);
        assert_eq!(y.spans[0].cells, 6);
    }

    #[test]
    fn surface_names_are_files_of_tri_surface() {
        for bad in ["sub/a.stl", ".a.stl", "notes.txt", "a.STL", ".stl"] {
            let mut b = box_of([0.0; 6], BoxCells::Size(1.0));
            b.surfaces = vec![bad.to_string()];
            assert!(b.problems().iter().any(|(k, _)| *k == "surfaces"), "{bad}");
        }
        for good in ["a.stl", "a b.stlb", "a.stl.gz", "a.stlb.gz"] {
            let mut b = box_of([0.0; 6], BoxCells::Size(1.0));
            b.surfaces = vec![good.to_string()];
            assert!(b.problems().is_empty(), "{good}");
        }
    }

    #[test]
    fn the_box_is_never_short_of_the_bounds() {
        let b = box_of([0.0; 6], BoxCells::Size(1.0));
        let [x, _, _] = b.axes([0.0; 3], [10_000_000.009, 1.0, 1.0], Side::Ground).unwrap();
        // Within rounding of whole cells: no cell more, but the end where the bounds are.
        assert_eq!(x.spans[0].cells, 10_000_000);
        assert_eq!(x.stations[1], 10_000_000.009);
        // Never short of the bounds, also by a rounding of `lo + n * h`.
        for i in 1..2000 {
            let target = 2.5 * f64::from(i) + 1e-12 * f64::from(i % 7);
            let [x, _, _] = b.axes([0.0; 3], [target, 1.0, 1.0], Side::Ground).unwrap();
            assert!(x.stations[1] >= target, "{target}");
        }
    }

    #[test]
    fn a_surface_name_with_a_dollar_reads_back() {
        let mut b = box_of([0.0; 6], BoxCells::Size(1.0));
        b.surfaces = vec!["$roof.stl".to_string()];
        let (read_back, problems) = read(&b.entry(""));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(read_back, Some(b));
        let (_, problems) = read("box { surfaces ( $roof ); margin ( 1 1 1 1 0 1 ); cellSize 1; }");
        assert!(problems.iter().any(|p| p.starts_with("box/surfaces")), "{problems:?}");
    }

    #[test]
    fn a_length_within_rounding_of_whole_cells_takes_no_more() {
        let b = box_of([0.0; 6], BoxCells::Size(0.1));
        let [x, _, _] = b.axes([0.0, 0.0, 0.0], [0.7, 0.3, 0.3], Side::Ground).unwrap();
        assert_eq!(x.spans[0].cells, 7);
    }

    #[test]
    fn counts_keep_the_box_as_the_margins_make_it() {
        let b = box_of([1.0, 1.0, 1.0, 1.0, 0.0, 2.0], BoxCells::Counts([7, 3, 5]));
        let [x, y, z] = b.axes([0.0, 0.0, 0.0], [10.0, 4.0, 3.0], Side::Ground).unwrap();
        assert_eq!((x.stations.clone(), x.spans[0].cells), (vec![-1.0, 11.0], 7));
        assert_eq!((y.stations.clone(), y.spans[0].cells), (vec![-1.0, 5.0], 3));
        assert_eq!((z.stations.clone(), z.spans[0].cells), (vec![0.0, 5.0], 5));
    }

    #[test]
    fn a_box_that_cant_be_made_says_why() {
        let flat = box_of([1.0, 1.0, 1.0, 1.0, 0.0, 0.0], BoxCells::Size(1.0));
        let e = flat.axes([0.0; 3], [5.0, 5.0, 0.0], Side::Ground).unwrap_err();
        assert!(e.contains("along z the box has no length"), "{e}");
        let tiny = box_of([0.0; 6], BoxCells::Size(1e-6));
        let e = tiny.axes([0.0; 3], [100.0, 100.0, 100.0], Side::Ground).unwrap_err();
        assert!(e.contains("more than 1000000000 cells"), "{e}");
        let huge = box_of([0.0; 6], BoxCells::Size(1e-300));
        let e = huge.axes([0.0; 3], [1e300, 1.0, 1.0], Side::Ground).unwrap_err();
        assert!(e.contains("cells"), "{e}");
        let wide = box_of([f64::MAX, 0.0, 0.0, 0.0, 0.0, 0.0], BoxCells::Counts([1, 1, 1]));
        let e = wide.axes([-f64::MAX, 0.0, 0.0], [1.0, 1.0, 1.0], Side::Ground).unwrap_err();
        assert!(e.contains("isn't finite"), "{e}");
        let negative = box_of([-1.0, 0.0, 0.0, 0.0, 0.0, 0.0], BoxCells::Counts([1, 1, 1]));
        let e = negative.axes([0.0; 3], [1.0; 3], Side::Ground).unwrap_err();
        assert!(e.contains("west: -1 isn't a distance"), "{e}");
        let none = box_of([0.0; 6], BoxCells::Counts([1, 0, 1]));
        assert!(none.axes([0.0; 3], [1.0; 3], Side::Ground).unwrap_err().contains("without cells"));
        let size = box_of([0.0; 6], BoxCells::Size(0.0));
        assert!(size.axes([0.0; 3], [1.0; 3], Side::Ground).unwrap_err().contains("isn't a size above 0"));
    }

    #[test]
    fn a_box_reads_back_as_it_is_written() {
        for cells in [BoxCells::Size(2.5), BoxCells::Counts([200, 75, 40])] {
            let b = MeshBox {
                surfaces: vec!["buildings.stl".to_string(), "my trees.stl".to_string()],
                margin: [100.0, 200.0, 50.5, 50.0, 0.0, 0.1 + 0.2],
                cells,
                made: Some(u64::MAX),
            };
            let text = b.entry("");
            let (read, problems) = read(&text);
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(read.as_ref(), Some(&b), "{text}");
        }
    }

    #[test]
    fn what_doesnt_read_is_a_problem() {
        let (b, p) = read("box { surfaces ( a.stl ); margin ( 1 2 3 ); cellSize 2; }");
        assert!(b.is_none());
        assert!(p.iter().any(|p| p.starts_with("box/margin: is six distances")), "{p:?}");
        let (b, p) = read("box { surfaces ( a.stl ); margin ( 1 1 1 1 0 1 ); cellSize 2; cells ( 1 1 1 ); }");
        assert!(b.is_none());
        assert!(p.iter().any(|p| p.contains("one of the two")), "{p:?}");
        let (b, p) = read("box { surfaces ( a.stl ); margin ( 1 1 1 1 0 1 ); }");
        assert!(b.is_none());
        assert!(p.iter().any(|p| p.contains("no cellSize or cells")), "{p:?}");
        let (b, p) = read("box { margin ( 1 1 1 1 0 1 ); cellSize 1; level 3; }");
        assert!(b.is_none());
        assert!(p.iter().any(|p| p == "box: has no surfaces"), "{p:?}");
        assert!(p.iter().any(|p| p.starts_with("box/level: isn't a key")), "{p:?}");
        let (b, p) = read("box { surfaces ( $s ); margin ( 1 1 1 1 0 1 ); cells ( 1 -1 1 ); }");
        assert!(b.is_none());
        assert!(p.iter().any(|p| p.starts_with("box/surfaces")), "{p:?}");
        assert!(p.iter().any(|p| p.starts_with("box/cells")), "{p:?}");
        let (_, p) = read("box { surfaces ( a.stl ); #include \"x\" margin ( 1 1 1 1 0 1 ); cellSize 1; }");
        assert!(p.iter().any(|p| p.contains("isn't read here")), "{p:?}");
    }
}
