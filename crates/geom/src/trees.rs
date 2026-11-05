//! Tree crowns as a closed surface, from a table of trees: each crown a rhombicuboctahedron of
//! the tree's crown diameter and height, standing on its z (the bottom of the foliage). The
//! shape is the one of the vegetation workflow's `tree_shape_simple.py`; here every face is wound
//! counter-clockwise seen from outside, the eight triangular faces are triangles (not quads with
//! a corner twice, which gave triangles without area), and z is the crown's base as the table
//! says, not its centre.

use crate::Vec3;
use crate::stl::{Format, Stl, Triangle};

/// One tree: where its crown starts and how big it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tree {
    pub x: f64,
    pub y: f64,
    /// The bottom of the crown (the foliage).
    pub z: f64,
    /// Crown diameter (`CD`).
    pub diameter: f64,
    /// Crown height (`CH`).
    pub height: f64,
}

/// The most trees one table or surface takes: at most about 12.6 kB of ASCII STL a tree (the
/// longest numbers STL text holds) keeps the surface under the case's 1 GiB file limit.
pub const MAX_TREES: usize = 80_000;

/// The largest coordinate a crown may reach: STL stores single-precision numbers.
const MAX_COORD: f64 = 1e30;

impl Tree {
    /// Why this tree can't be made: a size that isn't positive, or a crown beyond what STL
    /// holds.
    pub fn problem(&self) -> Option<String> {
        let all = [self.x, self.y, self.z, self.diameter, self.height];
        if all.iter().any(|v| !v.is_finite()) {
            return Some("a number that isn't finite".into());
        }
        if self.diameter <= 0.0 || self.height <= 0.0 {
            return Some("the crown diameter and height are positive".into());
        }
        let reach = [self.x.abs() + self.diameter, self.y.abs() + self.diameter, self.z.abs() + self.height];
        if reach.iter().any(|v| *v > MAX_COORD) {
            return Some(format!("the crown reaches beyond {MAX_COORD:e} m, more than STL holds"));
        }
        // STL's single precision has about 7 digits: a crown much smaller than its coordinates
        // would come out with corners in the same place.
        let scale = self.x.abs().max(self.y.abs()).max(self.z.abs()).max(1.0);
        if self.diameter.min(self.height) < 1e-6 * scale {
            return Some(
                "the crown is too small for its coordinates in single precision (STL): move the coordinates to a local origin"
                    .into(),
            );
        }
        None
    }
}

/// The trees of a table: one tree a line, the numbers separated by tabs, commas or spaces (one kind
/// for the whole table, the first line's; with tabs or commas an empty cell is an error, not
/// skipped). A first line of column names (`x y z CD CH` in any order, case ignored; `D`/`H` and
/// `diameter`/`height` stand for the crown's when there is no `CD`/`CH`; other columns are left
/// alone), or none: then a tree is exactly five numbers, x, y, z, CD, CH. A leading byte-order
/// mark, blank lines and lines starting with `#` or `//` are skipped.
pub fn read_table(text: &str) -> Result<Vec<Tree>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut cols: Option<[usize; 5]> = None;
    let mut width = 5;
    let mut sep: Option<Sep> = None;
    let mut out = Vec::new();
    let mut first = true;
    for (k, raw) in text.lines().enumerate() {
        let bare = raw.trim();
        if bare.is_empty() || bare.starts_with('#') || bare.starts_with("//") {
            continue;
        }
        // Spaces and a carriage return off the ends, not tabs: a leading tab is an empty first
        // cell, and trimming it would move every value one column left.
        let line = raw.trim_matches(|c| c == ' ' || c == '\r');
        // The separator is the first line's, for the whole table.
        let sep = *sep.get_or_insert_with(|| separator(line));
        let words = fields(line, sep).map_err(|e| format!("line {}: {e}", k + 1))?;
        if first {
            first = false;
            // A header has a cell that is a word; an empty cell is a missing number.
            if words.iter().any(|w| !w.is_empty() && w.parse::<f64>().is_err()) {
                cols = Some(header(&words).map_err(|e| format!("line {}: {e}", k + 1))?);
                width = words.len();
                continue;
            }
        }
        // A line wider than the header would shift its values (R writes row names this way).
        // Empty cells at the end (a separator closing each line) don't count.
        let used = words.iter().rposition(|w| !w.is_empty()).map_or(0, |i| i + 1);
        if cols.is_some() && used > width {
            return Err(format!(
                "line {}: {used} cells but {width} column names (row names? write the table without them, e.g. R's row.names = FALSE)",
                k + 1
            ));
        }
        // Space-separated cells can't be empty: a shorter line has left one out, and the ones
        // after it would shift.
        if sep == Sep::Space && cols.is_some() && words.len() < width {
            return Err(format!(
                "line {}: {} cells but {width} column names (a value left out? a space-separated table can't leave a cell empty: use tabs or commas)",
                k + 1,
                words.len()
            ));
        }
        if cols.is_none() && words.len() != 5 {
            return Err(format!(
                "line {}: {} numbers; without a line of column names a tree is five: x y z CD CH",
                k + 1,
                words.len()
            ));
        }
        let [cx, cy, cz, cd, ch] = cols.unwrap_or([0, 1, 2, 3, 4]);
        let num = |c: usize, what: &str| -> Result<f64, String> {
            let w = words.get(c).ok_or_else(|| format!("line {}: no {what}", k + 1))?;
            w.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("line {}: the {what} `{}` is not a number", k + 1, short(w)))
        };
        let t = Tree {
            x: num(cx, "x")?,
            y: num(cy, "y")?,
            z: num(cz, "z")?,
            diameter: num(cd, "crown diameter")?,
            height: num(ch, "crown height")?,
        };
        if let Some(p) = t.problem() {
            return Err(format!("line {}: {p}", k + 1));
        }
        if out.len() >= MAX_TREES {
            return Err(format!("more than {MAX_TREES} trees"));
        }
        out.push(t);
    }
    Ok(out)
}

/// The most columns a line may have.
const MAX_COLUMNS: usize = 1024;

/// `s` for an error message: at most 80 characters.
fn short(s: &str) -> String {
    match s.char_indices().nth(80) {
        Some((i, _)) => format!("{}…", s.get(..i).unwrap_or_default()),
        None => s.to_string(),
    }
}

/// How a table's cells are separated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sep {
    Tab,
    Comma,
    /// One or more spaces (or tabs) between cells, none before the first.
    Space,
}

/// The separator of a line, by what is outside quoted cells: tabs if any, else commas, else
/// spaces.
fn separator(line: &str) -> Sep {
    // A quote opens a quoted cell only at a cell's start, as in `fields`.
    let mut quoted = false;
    let mut start = true;
    let (mut tab, mut comma) = (false, false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            continue;
        }
        match c {
            '"' if start => quoted = true,
            '\t' => tab = true,
            ',' => comma = true,
            _ => {}
        }
        start = matches!(c, '\t' | ',' | ' ');
    }
    if tab {
        Sep::Tab
    } else if comma {
        Sep::Comma
    } else {
        Sep::Space
    }
}

/// The cells of a line, separated by `sep`. With tabs or commas an empty cell is kept (as an
/// empty word, which is no number), so a missing value doesn't shift the ones after it. A cell
/// that starts with a double quote runs to the closing one, whatever it holds (`""` inside is a
/// quote), as spreadsheets and R write them, and loses its quotes; a quote elsewhere is text.
fn fields(line: &str, sep: Sep) -> Result<Vec<String>, String> {
    let is_sep = |c: char| match sep {
        Sep::Tab => c == '\t',
        Sep::Comma => c == ',',
        Sep::Space => c == ' ',
    };
    if sep == Sep::Space && line.contains('\t') && separator(line) == Sep::Tab {
        return Err("the first line is separated by spaces, this one by tabs: use one kind in the whole table".into());
    }
    let mut cells: Vec<String> = Vec::new();
    let push = |cells: &mut Vec<String>, cell: String| -> Result<(), String> {
        if cells.len() >= MAX_COLUMNS {
            return Err(format!("more than {MAX_COLUMNS} columns"));
        }
        cells.push(cell);
        Ok(())
    };
    let mut chars = line.chars().peekable();
    loop {
        // Space-separated: any run of separators is one, and there is none before the first cell.
        if sep == Sep::Space {
            while chars.peek().is_some_and(|c| is_sep(*c)) {
                chars.next();
            }
            if chars.peek().is_none() {
                break;
            }
        } else {
            while chars.peek().is_some_and(|c| *c == ' ') {
                chars.next();
            }
        }
        let mut cell = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        cell.push('"');
                        chars.next();
                    } else {
                        closed = true;
                        break;
                    }
                } else {
                    cell.push(c);
                }
            }
            if !closed {
                return Err("a quote that isn't closed".into());
            }
            // Up to the separator after it (spaces only).
            while let Some(c) = chars.peek().copied() {
                if is_sep(c) {
                    break;
                }
                if c != ' ' {
                    return Err(format!("text after the quoted cell \"{}\"", short(&cell)));
                }
                chars.next();
            }
        } else {
            while let Some(c) = chars.peek().copied() {
                if is_sep(c) {
                    break;
                }
                cell.push(c);
                chars.next();
            }
            cell = cell.trim().to_string();
        }
        push(&mut cells, cell)?;
        match chars.next() {
            Some(_) if sep != Sep::Space => {
                // A separator at the end of the line leaves an empty last cell.
                if chars.peek().is_none() {
                    push(&mut cells, String::new())?;
                    break;
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    Ok(cells)
}

/// The columns of x, y, z, the crown diameter and height in a line of column names. `CD` and
/// `CH` win over `D`/`diameter` and `H`/`height` (a table may give the tree's height as `H`
/// beside the crown's); two columns of the same kind otherwise are an error. Other columns are
/// left alone.
fn header(words: &[String]) -> Result<[usize; 5], String> {
    let fields: [(&str, &[&str], &[&str]); 5] = [
        ("x", &["x"], &[]),
        ("y", &["y"], &[]),
        ("z", &["z"], &[]),
        ("crown diameter", &["CD"], &["D", "diameter"]),
        ("crown height", &["CH"], &["H", "height"]),
    ];
    let mut out = [0usize; 5];
    for (slot, (what, primary, alias)) in out.iter_mut().zip(fields) {
        let of = |names: &[&str]| -> Vec<usize> {
            words
                .iter()
                .enumerate()
                .filter(|(_, w)| names.iter().any(|n| w.eq_ignore_ascii_case(n)))
                .map(|(k, _)| k)
                .collect()
        };
        let p = of(primary);
        let found = if p.is_empty() { of(alias) } else { p };
        *slot = match found.as_slice() {
            [k] => *k,
            [] => {
                return Err(format!(
                    "no column for the {what}: the column names are x, y, z, CD and CH (separated by tabs, spaces or commas), not `{}`",
                    short(&words.join(" "))
                ));
            }
            _ => return Err(format!("two columns for the {what} in `{}`", short(&words.join(" ")))),
        };
    }
    Ok(out)
}

/// The crowns of `trees` as one ASCII surface whose single solid is `solid`: 44 triangles a
/// tree, closed, wound outwards.
pub fn crowns(trees: &[Tree], solid: &str) -> Stl {
    let mut triangles = Vec::with_capacity(trees.len().saturating_mul(44));
    for t in trees {
        crown(t, &mut triangles);
    }
    Stl {
        format: Format::Ascii,
        solids: vec![solid.to_string()],
        triangles,
        notes: Vec::new(),
        openfoam: None,
        header_solid: false,
    }
}

/// The 18 square faces and the 8 triangles of a rhombicuboctahedron, by the corners below.
const SQUARES: [[usize; 4]; 18] = [
    [0, 1, 2, 3],
    [5, 6, 7, 4],
    [8, 9, 10, 11],
    [12, 13, 14, 15],
    [16, 17, 18, 19],
    [20, 21, 22, 23],
    [0, 16, 17, 1],
    [2, 3, 20, 21],
    [1, 2, 13, 12],
    [0, 3, 9, 8],
    [4, 5, 18, 19],
    [6, 7, 23, 22],
    [5, 6, 14, 15],
    [4, 7, 10, 11],
    [17, 12, 15, 18],
    [13, 21, 22, 14],
    [20, 9, 10, 23],
    [8, 16, 19, 11],
];
const TRIANGLES: [[usize; 3]; 8] =
    [[1, 12, 17], [2, 21, 13], [3, 20, 9], [0, 16, 8], [5, 18, 15], [6, 14, 22], [4, 11, 19], [7, 10, 23]];

fn crown(t: &Tree, out: &mut Vec<Triangle>) {
    let s2 = std::f64::consts::SQRT_2;
    let w = t.diameter * s2 / (s2 + 2.0);
    // The middle band's height and the slanted bands' above and below it.
    let (z, za, zb) = if t.height >= t.diameter {
        (t.height - 2.0 * w / s2, w / s2, w / s2)
    } else {
        let z = t.height * s2 / (s2 + 2.0);
        (z, z / s2, z / s2)
    };
    let o = 0.5 * w + w / s2;
    let (h, m) = (0.5 * w, 0.5 * z);
    let corners: [[f64; 3]; 24] = [
        [-h, -h, -m - zb],
        [h, -h, -m - zb],
        [h, h, -m - zb],
        [-h, h, -m - zb],
        [-h, -h, m + za],
        [h, -h, m + za],
        [h, h, m + za],
        [-h, h, m + za],
        [-o, -h, -m],
        [-o, h, -m],
        [-o, h, m],
        [-o, -h, m],
        [o, -h, -m],
        [o, h, -m],
        [o, h, m],
        [o, -h, m],
        [-h, -o, -m],
        [h, -o, -m],
        [h, -o, m],
        [-h, -o, m],
        [-h, o, -m],
        [h, o, -m],
        [h, o, m],
        [-h, o, m],
    ];
    // Standing on z: the lowest corners at the tree's z.
    let lift = m + zb;
    let at = |k: usize| -> Vec3 {
        let c = corners.get(k).copied().unwrap_or_default();
        Vec3::new(t.x + c[0], t.y + c[1], t.z + lift + c[2])
    };
    let centre = Vec3::new(t.x, t.y, t.z + lift + 0.5 * (za - zb));
    let mut push = |a: Vec3, b: Vec3, c: Vec3| {
        // Counter-clockwise seen from outside: the normal points away from the centre (the
        // crown is convex, so every face's points are on the outer side).
        let n = (b - a).cross(c - a);
        let mid = Vec3::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0, (a.z + b.z + c.z) / 3.0);
        let (b, c) = if n.dot(mid - centre) < 0.0 { (c, b) } else { (b, c) };
        let n = (b - a).cross(c - a);
        let len = n.dot(n).sqrt();
        let unit = if len > 0.0 { [n.x / len, n.y / len, n.z / len] } else { [0.0; 3] };
        out.push(Triangle {
            vertices: [a, b, c].map(|p| [p.x as f32, p.y as f32, p.z as f32]),
            normal: unit.map(|v| v as f32),
            solid: 0,
        });
    };
    for [a, b, c, d] in SQUARES {
        push(at(a), at(b), at(c));
        push(at(a), at(c), at(d));
    }
    for [a, b, c] in TRIANGLES {
        push(at(a), at(b), at(c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::analyse;

    const TABLE: &str = "x\ty\tz\tCD\tCH\n260\t260\t2.5\t5.5\t5.5\n330\t330\t2.5\t5.5\t5.5\n";

    #[test]
    fn crowns_are_closed_wound_outwards_and_without_empty_triangles() {
        // Tall, round and flat crowns: both branches of the shape.
        for (d, h) in [(5.5, 5.5), (4.0, 9.0), (8.0, 3.0)] {
            let trees = [Tree { x: 10.0, y: 20.0, z: 2.5, diameter: d, height: h }];
            let stl = crowns(&trees, "trees");
            let r = analyse(&stl);
            assert_eq!(r.triangles, 44);
            assert_eq!((r.degenerate.count, r.duplicate.count, r.open_edges), (0, 0, 0), "{d}x{h}");
            assert_eq!((r.non_manifold_edges, r.inconsistent_edges), (0, 0), "{d}x{h}");
            assert_eq!((r.parts, r.closed_parts, r.inward_parts), (1, 1, 0), "{d}x{h}");
            // Standing on z, as wide and tall as the table says.
            let b = r.bbox.unwrap();
            let close = |a: f64, b: f64| (a - b).abs() < 1e-4;
            assert!(close(b.min.z, 2.5) && close(b.max.z, 2.5 + h), "{b:?}");
            assert!(close(b.max.x - b.min.x, d) && close(b.max.y - b.min.y, d), "{b:?}");
            assert!(close(0.5 * (b.min.x + b.max.x), 10.0) && close(0.5 * (b.min.y + b.max.y), 20.0), "{b:?}");
            // Every stored normal agrees with the winding.
            for t in &stl.triangles {
                let n = t.cross();
                let s = Vec3::new(t.normal[0].into(), t.normal[1].into(), t.normal[2].into());
                assert!(n.dot(s) > 0.0);
            }
        }
    }

    #[test]
    fn a_table_gives_one_crown_each() {
        let trees = read_table(TABLE).unwrap();
        assert_eq!(trees.len(), 2);
        assert_eq!(trees[1], Tree { x: 330.0, y: 330.0, z: 2.5, diameter: 5.5, height: 5.5 });
        let stl = crowns(&trees, "trees");
        let r = analyse(&stl);
        assert_eq!((r.triangles, r.parts, r.closed_parts), (88, 2, 2));
        assert_eq!(r.regions, ["trees"]);
        let b = r.bbox.unwrap();
        assert!((b.min.z - 2.5).abs() < 1e-4 && (b.max.z - 8.0).abs() < 1e-4, "{b:?}");
        // Written and read back, it is the same surface.
        let back = crate::stl::read(&stl.to_bytes(), &crate::stl::Limits::default()).unwrap();
        assert_eq!(back.triangles.len(), 88);
        assert_eq!(analyse(&back).inconsistent_edges, 0);
    }

    #[test]
    fn tables_with_other_columns_separators_or_no_header() {
        let t = read_table("# trees\nCH, CD, z, y, x\n6, 4, 1, 2, 3\n\n// end\n").unwrap();
        assert_eq!(t, [Tree { x: 3.0, y: 2.0, z: 1.0, diameter: 4.0, height: 6.0 }]);
        let t = read_table("1 2 3 4 5\n6 7 8 9 10\n").unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], Tree { x: 1.0, y: 2.0, z: 3.0, diameter: 4.0, height: 5.0 });
        let t = read_table("X Y Z diameter height\n1 2 3 4 5\n").unwrap();
        assert_eq!(t[0].height, 5.0);
        assert!(read_table("x y z\n1 2 3\n").unwrap_err().contains("no column for the crown diameter"));
        // The tree's height beside the crown's: CH is the crown's.
        let t = read_table("x y z H CD CH\n1 2 3 20 4 5\n").unwrap();
        assert_eq!((t[0].diameter, t[0].height), (4.0, 5.0));
        assert!(
            read_table("x y z D diameter CH\n1 2 3 4 4 5\n")
                .unwrap_err()
                .contains("two columns for the crown diameter")
        );
        // Without names, exactly five numbers.
        assert!(read_table("1 2 3 20 4 5\n").unwrap_err().contains("6 numbers"));
        // A byte-order mark (spreadsheet exports) is not a column name.
        assert_eq!(read_table("\u{feff}x y z CD CH\n1 2 3 4 5\n").unwrap().len(), 1);
        assert!(read_table("x;y;z;CD;CH\n").unwrap_err().contains("tabs, spaces or commas"));
        // An empty cell is no number, and doesn't shift the cells after it.
        assert!(
            read_table("x,y,z,CD,CH,H\n1,2,3,,5,20\n").unwrap_err().contains("the crown diameter `` is not a number")
        );
        assert!(read_table("x\ty\tz\tCD\tCH\tH\n1\t2\t3\t\t5\t20\n").unwrap_err().contains("is not a number"));
        // A cell with a space, with tabs between the cells, is one cell.
        let t = read_table("species\tx\ty\tz\tCD\tCH\nQuercus robur\t1\t2\t3\t4\t5\n").unwrap();
        assert_eq!(t[0], Tree { x: 1.0, y: 2.0, z: 3.0, diameter: 4.0, height: 5.0 });
        // A line of endless columns stops early.
        // R's write.table: spaces between cells, quoted strings with spaces.
        let t = read_table("\"species\" \"n\" \"x\" \"y\" \"z\" \"CD\" \"CH\"\n\"Quercus robur\" 7 10 20 30 4 5\n")
            .unwrap();
        assert_eq!(t[0], Tree { x: 10.0, y: 20.0, z: 30.0, diameter: 4.0, height: 5.0 });
        // R's row names: a cell more than the header has names.
        let e = read_table("\"x\" \"y\" \"z\" \"CD\" \"CH\"\n\"1\" 10 20 30 4 5\n").unwrap_err();
        assert!(e.contains("6 cells but 5 column names"), "{e}");
        // write.csv's empty name over the row names is a column like any other.
        assert_eq!(read_table("\"\",\"x\",\"y\",\"z\",\"CD\",\"CH\"\n\"1\",10,20,30,4,5\n").unwrap()[0].x, 10.0);
        // A quote inside a cell is text; a comma inside quotes doesn't make a space table CSV.
        let t = read_table("pot\tx\ty\tz\tCD\tCH\n12\" pot\t1\t2\t3\t4\t5\n").unwrap();
        assert_eq!(t[0].x, 1.0);
        let t = read_table("\"a, b\" x y z CD CH\nq 1 2 3 4 5\n").unwrap();
        assert_eq!(t[0].height, 5.0);
        let e = read_table("x y z CD CH n\n1 3 4 5 6\n").unwrap_err();
        assert!(e.contains("5 cells but 6 column names"), "{e}");
        // A separator closing each line is no extra column.
        assert_eq!(read_table("x\ty\tz\tCD\tCH\n1\t2\t3\t4\t5\t\n").unwrap().len(), 1);
        assert_eq!(read_table("x,y,z,CD,CH\n1,2,3,4,5,\n").unwrap().len(), 1);
        // A space header over tab data would collapse empty cells: refused.
        let e = read_table("x y z CD CH n\n1\t\t3\t4\t5\t6\n").unwrap_err();
        assert!(e.contains("separated by spaces, this one by tabs"), "{e}");
        // A quote inside a header word doesn't hide the separator.
        assert_eq!(read_table("a\"b,x,y,z,CD,CH\nq,1,2,3,4,5\n").unwrap()[0].x, 1.0);
        // Long cells are cut in messages.
        let e = read_table(&format!("x y z CD CH\n1 2 3 {} 5\n", "a".repeat(10_000))).unwrap_err();
        assert!(e.len() < 300 && e.ends_with("…` is not a number"), "{e}");
        let long = format!("{}\n", "1 ".repeat(100_000));
        assert!(read_table(&long).unwrap_err().contains("more than 1024 columns"));
        // An empty first cell (a GIS export's empty id) stays a cell: nothing shifts.
        let t = read_table("id\tx\ty\tz\tCD\tCH\n\t1\t2\t3\t4\t5\r\n").unwrap();
        assert_eq!(t[0], Tree { x: 1.0, y: 2.0, z: 3.0, diameter: 4.0, height: 5.0 });
        assert!(read_table("x\ty\tz\tCD\tCH\tn\n\t1\t2\t3\t4\t5\n").unwrap_err().contains("the x `` is not a number"));
        // Without a header, an empty cell is a missing number, not a header.
        assert!(read_table("1\t\t3\t4\t5\n").unwrap_err().contains("the y `` is not a number"));
        // Quoted cells: a comma inside is part of the cell, and the quotes come off.
        let t = read_table("note,n,x,y,z,CD,CH,m\n\"a, b\",7,10,20,30,4,5,6\n").unwrap();
        assert_eq!(t[0], Tree { x: 10.0, y: 20.0, z: 30.0, diameter: 4.0, height: 5.0 });
        let t = read_table("\"x\",\"y\",\"z\",\"CD\",\"CH\"\n\"1\",2,3,4,5\n").unwrap();
        assert_eq!(t[0].x, 1.0);
        let t = read_table("\"x\" \"y\" \"z\" \"CD\" \"CH\"\n1 2 3 4 5\n").unwrap();
        assert_eq!(t.len(), 1);
        assert!(read_table("x,y,z,CD,CH\n\"1,2,3,4,5\n").unwrap_err().contains("quote that isn't closed"));
        assert!(read_table("1e39 2 3 4 5\n").unwrap_err().contains("beyond"));
        assert!(read_table("1e8 2 3 1 5\n").unwrap_err().contains("too small"));
        assert!(read_table("1 2 3 1e-300 5\n").unwrap_err().contains("too small"));
        // Map coordinates with a real crown are fine.
        let map = read_table("2600000 1200000 450 5.5 5.5\n2600000 1200000 450 2.6 2.6\n").unwrap();
        for t in &map {
            let r = analyse(&crowns(std::slice::from_ref(t), "trees"));
            assert_eq!((r.degenerate.count, r.open_edges, r.closed_parts, r.inward_parts), (0, 0, 1, 0), "{t:?}");
        }
        assert!(read_table("1 2 3 4\n").unwrap_err().contains("line 1: 4 numbers"));
        assert!(read_table("1 2 3 four 5\n2 2 2 2 2\n").unwrap_err().contains("no column for the x"));
        assert!(read_table("1 2 3 4 5\n1 2 3 x 5\n").unwrap_err().contains("line 2: the crown diameter `x`"));
        assert!(read_table("1 2 3 0 5\n").unwrap_err().contains("positive"));
        assert!(read_table("1 2 3 inf 5\n").unwrap_err().contains("not a number"));
        assert_eq!(read_table("").unwrap(), []);
    }
}
