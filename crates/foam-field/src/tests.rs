use std::io::Write;

use super::*;

fn header(format: &str, class: &str, arch: &str) -> String {
    format!(
        "/*--- banner: nonuniform List<scalar> 3 (1 2 3) in a comment is not data ---*/\nFoamFile\n{{\n    version     2.0;\n    format      {format};\n    {arch}class       {class};\n    location    \"3600/air\";\n    object      T;\n}}\n"
    )
}

const BOUNDARY: &str = "boundaryField\n{\n    ground\n    {\n        type            fixedValue;\n        value           nonuniform List<scalar> 2(300 301);\n    }\n    top\n    {\n        type            zeroGradient;\n    }\n    \"(wall|roof).*\"\n    {\n        type            calculated;\n        value           $internalField;\n    }\n}\n";

#[test]
fn ascii_scalar_field() {
    let src = format!(
        "{}dimensions      [0 0 0 1 0 0 0];\n\ninternalField   nonuniform List<scalar> \n4\n(\n290\n290.5\n-1e-3\n3e+02\n)\n;\n\n{BOUNDARY}",
        header("ascii", "volScalarField", "")
    );
    let f = Field::parse(src.as_bytes()).unwrap();
    assert_eq!(f.class(), Some("volScalarField"));
    assert_eq!(f.header.location.as_deref(), Some("3600/air"));
    assert_eq!(f.dimensions().as_deref(), Some("[0 0 0 1 0 0 0]"));
    let Some(Value::List(l)) = f.internal() else { panic!("{:?}", f.internal()) };
    assert_eq!((l.kind, l.len()), (Kind::Scalar, 4));
    assert_eq!(l.as_flat().unwrap(), [290.0, 290.5, -1e-3, 300.0]);
    assert_eq!(l.component_range(), Some(vec![(-1e-3, 300.0)]));
    assert_eq!(f.patches(), ["ground", "top", "(wall|roof).*"]);
    assert_eq!(f.patch_type("top").as_deref(), Some("zeroGradient"));
    let Some(Value::List(g)) = f.patch_value("ground", "value") else { panic!() };
    assert_eq!(g.as_flat().unwrap(), [300.0, 301.0]);
    assert_eq!(f.patch_value("(wall|roof).*", "value"), Some(Value::Other("$internalField".into())));
    assert_eq!(f.lists().len(), 2);
}

#[test]
fn ascii_vectors_uniform_and_shorthands() {
    let src = format!(
        "{}internalField   nonuniform List<vector> 2((1 0 0) ( 0.5 -2 3e1 ));\nboundaryField\n{{\n    in  {{ type fixedValue; value uniform (1.6852 0 0); }}\n    out {{ type fixedValue; value nonuniform List<vector> 3{{(0 0 1)}}; }}\n    e   {{ type empty; value nonuniform List<vector> 0(); }}\n    u   {{ type fixedValue; value nonuniform 2((1 2 3) (4 5 6)); }}\n}}\n",
        header("ascii", "volVectorField", "")
    );
    let f = Field::parse(src.as_bytes()).unwrap();
    let Some(Value::List(l)) = f.internal() else { panic!() };
    assert_eq!(l.get(1), Some(&[0.5, -2.0, 30.0][..]));
    assert_eq!(l.get(2), None);
    assert_eq!(f.patch_value("in", "value"), Some(Value::Uniform(vec![1.6852, 0.0, 0.0])));
    let Some(Value::List(out)) = f.patch_value("out", "value") else { panic!() };
    assert_eq!((out.len(), out.get(2)), (3, Some(&[0.0, 0.0, 1.0][..])));
    assert!(out.as_flat().is_none());
    let Some(Value::List(e)) = f.patch_value("e", "value") else { panic!() };
    assert!(e.is_empty());
    let Some(Value::List(u)) = f.patch_value("u", "value") else { panic!() };
    assert_eq!((u.kind, u.get(1)), (Kind::Vector, Some(&[4.0, 5.0, 6.0][..])));
}

/// A binary field written the way OpenFOAM writes one: `N(` then raw bytes then `)`.
fn binary_field(arch: Arch, kind: Kind, values: &[f64]) -> Vec<u8> {
    let arch_line = format!(
        "arch        \"{};label={};scalar={}\";\n    ",
        if arch.big_endian { "MSB" } else { "LSB" },
        arch.label_bytes * 8,
        arch.scalar_bytes * 8
    );
    let mut out = header("binary", "volScalarField", &arch_line).into_bytes();
    let n = values.len() / kind.components();
    write!(out, "dimensions [0 0 0 1 0 0 0];\ninternalField   nonuniform List<{}> {n}(", kind.name()).unwrap();
    for v in values {
        let bytes: Vec<u8> =
            match (kind == Kind::Label, if kind == Kind::Label { arch.label_bytes } else { arch.scalar_bytes }) {
                (false, 8) => {
                    if arch.big_endian {
                        v.to_be_bytes().to_vec()
                    } else {
                        v.to_le_bytes().to_vec()
                    }
                }
                (false, _) => {
                    let f = *v as f32;
                    if arch.big_endian { f.to_be_bytes().to_vec() } else { f.to_le_bytes().to_vec() }
                }
                (true, 8) => {
                    let i = *v as i64;
                    if arch.big_endian { i.to_be_bytes().to_vec() } else { i.to_le_bytes().to_vec() }
                }
                (true, _) => {
                    let i = *v as i32;
                    if arch.big_endian { i.to_be_bytes().to_vec() } else { i.to_le_bytes().to_vec() }
                }
            };
        out.extend(bytes);
    }
    out.extend_from_slice(b")\n;\n");
    // In a binary file every nonuniform list is binary, patch values included: keep the
    // boundary to uniform values here.
    out.extend_from_slice(BOUNDARY.replace("nonuniform List<scalar> 2(300 301)", "uniform 300").as_bytes());
    out
}

#[test]
fn binary_every_arch() {
    let values = [290.25, -1.5, 0.0, 1e5, 7.0, 42.0];
    for big_endian in [false, true] {
        for scalar_bytes in [4, 8] {
            for label_bytes in [4, 8] {
                let arch = Arch { big_endian, label_bytes, scalar_bytes };
                for kind in [Kind::Scalar, Kind::Vector, Kind::Label] {
                    let bytes = binary_field(arch, kind, &values);
                    // Real binary data is not UTF-8 text in general.
                    let f = Field::parse(&bytes).unwrap_or_else(|e| panic!("{arch:?} {kind:?}: {e}"));
                    assert_eq!(f.header.format, Format::Binary);
                    assert_eq!(f.header.arch, arch);
                    let Some(Value::List(l)) = f.internal() else { panic!() };
                    assert_eq!(l.kind, kind);
                    assert_eq!(l.len(), values.len() / kind.components());
                    // Labels are integers: the writer truncated 290.25 to 290.
                    let want: Vec<f64> =
                        values.iter().map(|v| if kind == Kind::Label { v.trunc() } else { *v }).collect();
                    assert_eq!(l.as_flat().unwrap(), want, "{arch:?} {kind:?}");
                    // The ASCII parts after the binary data are still read.
                    assert_eq!(f.patch_type("ground").as_deref(), Some("fixedValue"));
                }
            }
        }
    }
}

#[test]
fn binary_data_may_look_like_syntax() {
    // Bytes of `)`, `;`, `"`, `//` and `nonuniform` inside binary data must not confuse the scanner.
    let tricky =
        [f64::from_le_bytes(*b")\";//}{("), f64::from_le_bytes(*b"nonunifo"), f64::from_le_bytes(*b"rm List<")];
    let bytes = binary_field(Arch::default(), Kind::Scalar, &tricky);
    let f = Field::parse(&bytes).unwrap();
    let Some(Value::List(l)) = f.internal() else { panic!() };
    assert_eq!(l.as_flat().unwrap().iter().map(|v| v.to_bits()).collect::<Vec<_>>(), tricky.map(f64::to_bits));
}

#[test]
fn gzip_is_transparent() {
    let src = format!("{}internalField nonuniform List<scalar> 3(1 2 3);\n", header("ascii", "volScalarField", ""));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(src.as_bytes()).unwrap();
    let gz = gz.finish().unwrap();
    let f = Field::parse(&gz).unwrap();
    let Some(Value::List(l)) = f.internal() else { panic!() };
    assert_eq!(l.len(), 3);
    // A zip bomb stops at the limit.
    let small = Limits { max_bytes: 100, ..Limits::default() };
    assert!(matches!(Field::parse_with(&gz, &small), Err(Error::TooLarge { .. })));
    assert!(matches!(Field::parse(&[0x1f, 0x8b, 1, 2, 3]), Err(Error::Gzip(_))));
}

#[test]
fn read_from_case_prefers_plain_then_gz() {
    let src = format!("{}internalField uniform 293.15;\n", header("ascii", "volScalarField", ""));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(src.as_bytes()).unwrap();
    let fs = umcf_core::MemFs::new().with("3600/air/T.gz", gz.finish().unwrap());
    let f = Field::read(&fs, "3600/air/T", &Limits::default()).unwrap().unwrap();
    assert_eq!(f.internal(), Some(Value::Uniform(vec![293.15])));
    assert!(Field::read(&fs, "3600/air/U", &Limits::default()).unwrap().is_none());
}

#[test]
fn hostile_lengths_and_data() {
    let h = header("ascii", "volScalarField", "");
    // A huge length with no data: an error, not a 16 GB allocation.
    let e =
        Field::parse(format!("{h}internalField nonuniform List<scalar> 2000000000(1 2);\n").as_bytes()).unwrap_err();
    assert!(matches!(e, Error::BadList { .. }), "{e}");
    let e = Field::parse(format!("{h}internalField nonuniform List<scalar> 99999999999999999999999(1);\n").as_bytes())
        .unwrap_err();
    assert!(matches!(e, Error::BadList { .. }), "{e}");
    let small = Limits { max_values: 10, ..Limits::default() };
    let e = Field::parse_with(format!("{h}internalField nonuniform List<vector> 4((0 0 0));\n").as_bytes(), &small)
        .unwrap_err();
    assert!(matches!(e, Error::TooLarge { .. }), "{e}");
    let e = Field::parse(format!("{h}internalField nonuniform List<scalar> 3(1 x 3);\n").as_bytes()).unwrap_err();
    assert!(e.to_string().contains("expected a number, found `x`"), "{e}");
    let e = Field::parse(format!("{h}internalField nonuniform List<quaternion> 1(1);\n").as_bytes()).unwrap_err();
    assert!(matches!(e, Error::Unsupported(_)), "{e}");
    let e = Field::parse(format!("{h}internalField nonuniform List<vector> 1((1 2));\n").as_bytes()).unwrap_err();
    assert!(matches!(e, Error::BadList { .. }), "{e}");
    // Binary: the declared size runs past the end of the file.
    let mut bin = binary_field(Arch::default(), Kind::Scalar, &[1.0, 2.0]);
    let at = bin.windows(3).position(|w| w == b" 2(").unwrap();
    bin[at + 1] = b'9';
    assert!(matches!(Field::parse(&bin), Err(Error::Truncated { .. } | Error::BadList { .. })));
}

#[test]
fn spherical_tensors_are_parenthesised() {
    // Review finding: `List<sphericalTensor>` is written `(1)`, one component in parentheses.
    let src = format!(
        "{}internalField nonuniform List<sphericalTensor> 2((1) (2.5));\nboundaryField {{ w {{ type fixedValue; value uniform (3); }} }}\n",
        header("ascii", "volSphericalTensorField", "")
    );
    let f = Field::parse(src.as_bytes()).unwrap();
    let Some(Value::List(l)) = f.internal() else { panic!("{:?}", f.internal()) };
    assert_eq!((l.kind, l.as_flat().unwrap()), (Kind::SphericalTensor, &[1.0, 2.5][..]));
    assert_eq!(f.patch_value("w", "value"), Some(Value::Uniform(vec![3.0])));
    // Untyped lists: `(1)` elements are sphericalTensors, bare numbers scalars.
    let src = format!("{}internalField nonuniform 2((1) (2));\n", header("ascii", "volSphericalTensorField", ""));
    let f = Field::parse(src.as_bytes()).unwrap();
    let Some(Value::List(l)) = f.internal() else { panic!() };
    assert_eq!((l.kind, l.as_flat().unwrap()), (Kind::SphericalTensor, &[1.0, 2.0][..]));
    let src = format!("{}internalField nonuniform 2(1 2);\n", header("ascii", "volScalarField", ""));
    let f = Field::parse(src.as_bytes()).unwrap();
    assert!(matches!(f.internal(), Some(Value::List(l)) if l.kind == Kind::Scalar));
}

#[test]
fn header_after_a_long_banner() {
    // Review finding: a header past the first 8 KB was ignored, and with it the class.
    let banner = format!("/*{}*/\n", "-".repeat(20_000));
    let src = format!("{banner}{}internalField uniform 1;\n", header("ascii", "volScalarField", ""));
    let f = Field::parse(src.as_bytes()).unwrap();
    assert_eq!(f.class(), Some("volScalarField"));
    // No header at all: an ASCII data file, read with defaults.
    let f = Field::parse(b"internalField uniform 1;\n").unwrap();
    assert_eq!((f.class(), f.header.format), (None, Format::Ascii));
}

#[test]
fn nan_and_inf_values() {
    // A diverged run writes nan; the fast number path hands these to the slow one.
    let src = format!(
        "{}internalField nonuniform List<scalar> 4(nan -inf 1e-3 inf);\n",
        header("ascii", "volScalarField", "")
    );
    let f = Field::parse(src.as_bytes()).unwrap();
    let Some(Value::List(l)) = f.internal() else { panic!() };
    let v = l.as_flat().unwrap();
    assert!(v[0].is_nan() && v[1] == f64::NEG_INFINITY && v[2] == 1e-3 && v[3] == f64::INFINITY);
    // A number glued to a word is not a number.
    let src = format!("{}internalField nonuniform List<scalar> 1(1.5abc);\n", header("ascii", "volScalarField", ""));
    assert!(Field::parse(src.as_bytes()).unwrap_err().to_string().contains("`1.5abc`"));
}

#[test]
fn bare_typed_lists_as_in_zone_files() {
    let src = format!(
        "{}2\n(\nheating\n{{\n    type cellZone;\n    cellLabels      List<label> 3(4 7 9);\n}}\nempty\n{{\n    type cellZone;\n    cellLabels List<label> 0();\n}}\n)\n",
        header("ascii", "regIOobject", "")
    );
    let f = Field::parse(src.as_bytes()).unwrap();
    assert_eq!(f.lists().len(), 2);
    assert_eq!(f.lists()[0].as_flat().unwrap(), [4.0, 7.0, 9.0]);
    // Review finding: typed lists of other types stay text, as in field boundaryField entries.
    let src = format!(
        "{}internalField uniform 1;\nboundaryField\n{{\n    wall\n    {{\n        type zeroGradient;\n        inGroups List<word> 1(wall);\n    }}\n}}\n",
        header("ascii", "volScalarField", "")
    );
    let f = Field::parse(src.as_bytes()).unwrap();
    assert!(f.lists().is_empty());
    assert_eq!(f.doc.value(&["boundaryField", "wall", "inGroups"]).as_deref(), Some("List<word> 1 (wall)"));
    // `MyList<label>` is a word, not a list.
    let src = format!("{}internalField uniform 1;\nx MyList<label>;\n", header("ascii", "volScalarField", ""));
    assert!(Field::parse(src.as_bytes()).unwrap().lists().is_empty());
}

#[test]
fn header_end_offset() {
    let src = format!("{}3\n(\n0 1 2\n)\n", header("ascii", "labelList", ""));
    let (h, end) = read_header_end(src.as_bytes()).unwrap();
    assert_eq!(h.class.as_deref(), Some("labelList"));
    assert!(src[end.unwrap()..].trim_start().starts_with('3'));
    assert_eq!(read_header_end(b"3(0 1 2)").unwrap().1, None);
}

#[test]
fn note_counts() {
    let src = "FoamFile\n{\n    format ascii;\n    class labelList;\n    note \"nPoints:30416  nCells:25588  nFaces:81504  nInternalFaces:72072\";\n    object owner;\n}\n";
    let h = read_header(src.as_bytes()).unwrap();
    assert_eq!(h.count("nCells"), Some(25588));
    assert_eq!(h.count("nInternalFaces"), Some(72072));
    assert_eq!(h.count("nEdges"), None);
    // As topoSet and changeDictionary write it: a space after each colon.
    let src = "FoamFile\n{\n    class labelList;\n    note \"nPoints: 630 nCells: 400 nFaces: 1400 nInternalFaces: 1000\";\n}\n";
    let h = read_header(src.as_bytes()).unwrap();
    assert_eq!(h.count("nPoints"), Some(630));
    assert_eq!(h.count("nCells"), Some(400));
    assert_eq!(h.count("nInternalFaces"), Some(1000));
    assert_eq!(h.count("nEdges"), None);
}

#[test]
fn fuzz_mutations_never_panic() {
    let ascii = format!(
        "{}internalField nonuniform List<vector> 2((1 0 0) (0 1 0));\n{BOUNDARY}",
        header("ascii", "volVectorField", "")
    );
    let binary = binary_field(Arch::default(), Kind::Vector, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n.max(1) as u64) as usize
    };
    let iters = std::env::var("UMCF_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(5_000usize);
    for base in [ascii.into_bytes(), binary] {
        for _ in 0..iters {
            let mut m = base.clone();
            for _ in 0..1 + next(4) {
                if m.is_empty() {
                    break;
                }
                let i = next(m.len());
                match next(4) {
                    0 => {
                        m.remove(i);
                    }
                    1 => m.insert(i, b"(){};9 \"/#n"[next(11)]),
                    2 => m[i] = next(256) as u8,
                    _ => m.truncate(i),
                }
            }
            outline_agrees(&m, &[1, 1 << 20]);
            if let Ok(f) = Field::parse(&m) {
                let _ = f.internal();
                let _ = f.patches();
                for l in f.lists() {
                    let _ = l.component_range();
                }
            }
        }
    }
}

/// Whether entry `key` resolves to a spliced list (bare, or after `nonuniform`).
fn resolves(f: &Field, key: &str) -> bool {
    f.doc.get(&[key]).is_some_and(
        |e| matches!(e.value.last(), Some(umcf_foam_dict::Node::Atom(p)) if f.placeholder_list(&p.text).is_some()),
    )
}

#[test]
fn placeholder_words_in_the_input_name_no_list() {
    // Review findings: a word spelling a placeholder resolved to a list the scanner spliced out
    // (reused, or moved to another entry where the scanner and the lexer disagree about
    // comments and strings: `a#{`, `${"…}`, `${/*…}`, `${//}`). Placeholders now use a tag the
    // input doesn't contain, so no input word can name a list, and no word is reserved.
    let h = header("ascii", "regIOobject", "");
    let lists = "a List<label> 2(0 1);\nv nonuniform List<scalar> 1(5);\n";
    for forged in [
        "b __umcfList0;",
        "b __umcfListA0;",
        "b __umcfListA1;",
        "x a#{}; b __umcfListA0; y #};",
        "x ${\"};b __umcfListA0 ;\"\"};",
        "x ${/*};b __umcfListA0 ;*/};",
        "x ${//} \" ;\nw \" ;\nb __umcfListA0 ;\nz \" \";",
        "x ${/*} \" */ ;\nw \"} ;\nb __umcfListA1 ;",
    ] {
        for order in [format!("{h}{lists}{forged}\n"), format!("{h}{forged}\n{lists}")] {
            let f = Field::parse(order.as_bytes()).unwrap_or_else(|e| panic!("{order}: {e}"));
            assert!(!resolves(&f, "b"), "{order}");
        }
    }
    // Mentions in comments, strings and words are plain text.
    let src = format!("{h}// __umcfList7\nnote \"__umcfListA0\";\nw __umcfListB2;\n{lists}");
    let f = Field::parse(src.as_bytes()).unwrap();
    assert!(resolves(&f, "a") && resolves(&f, "v") && !resolves(&f, "w"));
    // Every one-letter suffix taken: the tag grows to two letters and still works.
    let taken: String = (b'A'..=b'Z').map(|c| format!("w{c} __umcfList{}0;\n", c as char)).collect();
    let src = format!("{h}{taken}b __umcfListAA0;\n{lists}");
    let f = Field::parse(src.as_bytes()).unwrap();
    assert!(resolves(&f, "a") && resolves(&f, "v") && !resolves(&f, "b") && !resolves(&f, "wA"));
    // A list the lexer merges into a macro isn't where a reader looks: refused, not lost.
    let src = format!("{h}z ${{nonuniform List<scalar> 1(1)}};\n");
    assert!(matches!(Field::parse(src.as_bytes()), Err(Error::Unsupported(_))));
    // A tag stays short, whatever the input uses.
    let many: String = (0..30u8).map(|i| format!("w{i} __umcfList{};\n", (b'A' + i % 26) as char)).collect();
    let tag = scan::Tag::for_input(format!("{many}__umcfList{}", "Q".repeat(10_000)).as_bytes());
    assert!(tag.word(0).len() < 16, "{tag:?}");
}

#[test]
fn placeholders_name_exactly_one_list() {
    // Review finding: digits right after a list ran on into its placeholder, so `0()1` named
    // list 1 (and `0()01` too, through a leading zero).
    let h = header("ascii", "regIOobject", "");
    for src in [
        format!("{h}a List<label> 0()1;\nb List<label> 3(5 6 7);\n"),
        format!("{h}a List<label> 0()01;\nb List<label> 1(5);\n"),
    ] {
        let f = Field::parse(src.as_bytes()).unwrap();
        let a = f.doc.get(&["a"]).unwrap();
        assert!(
            !matches!(a.value.as_slice(), [umcf_foam_dict::Node::Atom(p)] if f.placeholder_list(&p.text).is_some()),
            "{src}"
        );
    }
    let tag = scan::Tag::for_input(b"");
    assert_eq!(tag.index(&format!("{}1", tag.word(0))), None, "leading zero");
    assert_eq!(tag.index(&tag.word(1)), Some(1));
}

#[test]
fn bare_typed_lists_that_are_not_data_stay_text() {
    // Review finding: splicing every `List<T>` made a file fail when one isn't data.
    let h = header("ascii", "volScalarField", "");
    let src = format!("{h}internalField uniform 1;\ncoeffs {{ a List<scalar> (1 2 3); b List<scalar> 2($x $y); }}\n");
    let f = Field::parse(src.as_bytes()).unwrap();
    assert!(f.lists.is_empty());
    assert!(f.doc.to_string().contains("List<scalar> (1 2 3)"));
}

#[test]
fn empty_binary_list_is_its_size_alone() {
    // Review finding: decomposePar writes `value nonuniform List<vector> \n0\n;` for a patch with
    // no faces on a processor, in binary.
    let src = format!(
        "{}dimensions [0 1 -1 0 0 0 0];\ninternalField uniform (0 0 0);\nboundaryField\n{{\n    wall\n    {{\n        type fixedValue;\n        value nonuniform List<vector> \n0\n;\n    }}\n}}\n",
        header("binary", "volVectorField", "")
    );
    let f = Field::parse(src.as_bytes()).unwrap();
    let v = f.patch_value("wall", "value");
    assert!(matches!(v, Some(Value::List(l)) if l.is_empty()), "{v:?}");
}

#[test]
fn failing_bare_lists_cost_linear_time() {
    // Review finding: every bare `List<T>` that isn't data was retried after counting lines
    // from the start of the file (and scanning a bad word to its end): quadratic, hours for a
    // few MB. With the fix these parse in well under a second; quadratic would not finish.
    let h = header("ascii", "volScalarField", "");
    let many = "List<scalar> ;\n".repeat(200_000);
    let src = format!("{h}internalField uniform 1;\na ( {many} );\n");
    let _ = Field::parse(src.as_bytes());
    // Unquoted, so every `List<` is an attempt and each bad word `x;List<…` reaches the slow
    // number path (no whitespace, parenthesis or `}` ends it): capped at 64 bytes, not scanned
    // to the end of the file.
    let chain = "List<scalar>1{x;".repeat(40_000);
    let src = format!("{h}internalField uniform 1;\na {chain}\n");
    let _ = Field::parse(src.as_bytes());
    // The line number of a real error is still right.
    let src = format!("{h}internalField nonuniform List<scalar> 2(1 x);\n");
    let e = Field::parse(src.as_bytes()).unwrap_err().to_string();
    let line = src.lines().position(|l| l.contains("2(1 x)")).unwrap() + 1;
    assert!(e.starts_with(&format!("line {line}:")), "{e}");
}

#[test]
fn bad_lists_after_nonuniform_still_fail() {
    // The text fallback is for bare `List<T>` only: field data that doesn't read is an error.
    let h = header("ascii", "volScalarField", "");
    let src = format!("{h}internalField nonuniform List<scalar> 2($a $b);\n");
    assert!(matches!(Field::parse(src.as_bytes()), Err(Error::BadList { .. })));
}

#[test]
fn long_bad_words_are_errors() {
    // Review finding: a word past the 64-byte cap was parsed from its first 64 bytes.
    let h = header("ascii", "volScalarField", "");
    let src = format!("{h}internalField nonuniform List<scalar> 2(1.{}+2);\n", "0".repeat(62));
    assert!(matches!(Field::parse(src.as_bytes()), Err(Error::BadList { .. })));
    let long = format!("{h}internalField nonuniform List<scalar> 1(1.{});\n", "0".repeat(200));
    assert!(Field::parse(long.as_bytes()).is_ok(), "a long valid number still reads (fast path)");
}

#[test]
fn rewriting_keeps_every_byte_it_does_not_edit() {
    let none = |_: &mut Document| Ok(());
    let h = header("ascii", "volScalarField", "");
    let mut inputs: Vec<Vec<u8>> = vec![
        format!(
            "{h}dimensions      [0 0 0 1 0 0 0];\n\ninternalField   nonuniform List<scalar> \n4\n(\n290\n290.5\n-1e-3\n3e+02\n)\n;\n\n{BOUNDARY}"
        )
        .into_bytes(),
        format!("{h}internalField nonuniform\tList<scalar>   2(1 2)  ;\nboundaryField {{ a {{ type fixedValue; value nonuniform 1(3); }} }}\n").into_bytes(),
        format!("{h}2\n(\nheating\n{{\n    type cellZone;\n    cellLabels      List<label> 3(4 7 9);\n}}\n)\n").into_bytes(),
    ];
    for big_endian in [false, true] {
        let arch = Arch { big_endian, label_bytes: 4, scalar_bytes: 8 };
        let mut b = binary_field(arch, Kind::Vector, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        b.extend_from_slice(b"boundaryField\n{\n    ground { type fixedValue; value uniform 300; }\n}\n");
        inputs.push(b);
    }
    for input in &inputs {
        assert_eq!(rewrite(input, &Limits::default(), none).unwrap(), *input, "{}", String::from_utf8_lossy(input));
    }
    // An edit changes its entry only.
    let input = inputs.first().unwrap();
    let out = rewrite(input, &Limits::default(), |d| {
        d.set(&["boundaryField", "top"], "{ type fixedValue; value uniform 290; }").map_err(Error::from)
    })
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let before = String::from_utf8_lossy(input);
    assert!(text.starts_with(before.split("    top").next().unwrap()), "{text}");
    assert!(text.contains("nonuniform List<scalar> \n4\n(\n290\n290.5\n-1e-3\n3e+02\n)\n;"), "{text}");
    assert!(
        text.contains("value nonuniform List<scalar> 2(300 301);")
            || text.contains("value           nonuniform List<scalar> 2(300 301);"),
        "{text}"
    );
    let f = Field::parse(text.as_bytes()).unwrap();
    assert_eq!(f.patch_type("top").as_deref(), Some("fixedValue"));
    assert_eq!(f.patch_value("top", "value"), Some(Value::Uniform(vec![290.0])));
    assert_eq!(f.lists().len(), 2);
    // A binary file keeps its data byte for byte.
    let binary = inputs.last().unwrap();
    let out = rewrite(binary, &Limits::default(), |d| {
        d.set(&["boundaryField", "ground"], "{ type zeroGradient; }").map_err(Error::from)
    })
    .unwrap();
    let f = Field::parse(&out).unwrap();
    assert_eq!(f.lists().len(), 1);
    let Some(Value::List(l)) = f.internal() else { panic!() };
    assert_eq!(l.as_flat().unwrap(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    assert_eq!(out.get(..binary.len() - 60), binary.get(..binary.len() - 60));
    assert_eq!(f.patch_type("ground").as_deref(), Some("zeroGradient"));
}

#[test]
fn rewriting_gzip_and_refusing_placeholders() {
    let src = format!(
        "{}internalField nonuniform List<scalar> 3(1 2 3);\nboundaryField {{ }}\n",
        header("ascii", "volScalarField", "")
    );
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(src.as_bytes()).unwrap();
    let gz = gz.finish().unwrap();
    let out = rewrite(&gz, &Limits::default(), |d| {
        d.set(&["boundaryField", "a"], "{ type zeroGradient; }").map_err(Error::from)
    })
    .unwrap();
    assert!(out.starts_with(&[0x1f, 0x8b]));
    let f = Field::parse(&out).unwrap();
    assert_eq!(f.patch_type("a").as_deref(), Some("zeroGradient"));
    assert_eq!(f.lists().len(), 1);
    // A value naming a list's placeholder would copy the list into it: refused.
    let tag = scan::Tag::for_input(src.as_bytes());
    let word = tag.word(0);
    let e = rewrite(src.as_bytes(), &Limits::default(), |d| {
        d.set(&["boundaryField", "a"], &format!("{{ type fixedValue; value {word}; }}")).map_err(Error::from)
    });
    assert!(matches!(e, Err(Error::Unsupported(_))), "{e:?}");
}

/// A case whose readers hand out `step` bytes at a time, so a stream's chunks end anywhere.
struct Trickle {
    fs: umcf_core::MemFs,
    step: usize,
}

struct Drip {
    bytes: std::io::Cursor<Vec<u8>>,
    step: usize,
}

impl std::io::Read for Drip {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.step);
        self.bytes.read(&mut buf[..n])
    }
}

impl umcf_core::CaseFs for Trickle {
    fn read(&self, rel: &str, limit: u64) -> std::result::Result<Option<Vec<u8>>, umcf_core::FsError> {
        self.fs.read(rel, limit)
    }
    fn reader(
        &self,
        rel: &str,
        limit: u64,
    ) -> std::result::Result<Option<Box<dyn std::io::Read + Send>>, umcf_core::FsError> {
        let step = self.step;
        Ok(self
            .fs
            .read(rel, limit)?
            .map(|b| Box::new(Drip { bytes: std::io::Cursor::new(b), step }) as Box<dyn std::io::Read + Send>))
    }
    fn is_file(&self, rel: &str) -> bool {
        self.fs.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.fs.is_dir(rel)
    }
    fn list(&self, rel: &str) -> std::result::Result<Vec<String>, umcf_core::FsError> {
        self.fs.list(rel)
    }
}

fn outline_of(bytes: &[u8], step: usize) -> Result<Field> {
    let t = Trickle { fs: umcf_core::MemFs::new().with("0/T", bytes.to_vec()), step };
    Field::outline(&t, "0/T", &Limits::default(), u64::MAX).map(|f| f.expect("the file is there"))
}

/// The outline of `bytes`, read in chunks of each of `steps` bytes, is what `Field::parse`
/// reads without the values: the same header, the same document (placeholders included), the
/// same lists by kind and length, and an error where it fails. The one difference: a list before
/// the `FoamFile` header (no real file has one) is read as ASCII, where `Field::parse` reads it
/// in the format the header gives later; in a binary file the outline may then fail.
fn outline_agrees(bytes: &[u8], steps: &[usize]) {
    let whole = Field::parse(bytes);
    for &step in steps {
        let got = outline_of(bytes, step);
        let text = String::from_utf8_lossy(bytes);
        if list_before_header(bytes) {
            continue;
        }
        match (&whole, got) {
            (Ok(w), Ok(o)) => {
                assert_eq!(o.header, w.header, "{step}: {text}");
                // The placeholders' tag is chosen from the text alone (list data that spells one
                // can change it): the words are the same but for it.
                let words = |f: &Field| f.doc.to_string().replace(f.tag.prefix(), "@");
                assert_eq!(words(&o), words(w), "{step}: {text}");
                let shape =
                    |f: &Field| f.lists().iter().map(|l| (l.kind, l.len(), l.is_repeated())).collect::<Vec<_>>();
                assert_eq!(shape(&o), shape(w), "{step}: {text}");
                for (a, b) in o.lists().iter().zip(w.lists()) {
                    // `N{value}` keeps its value; every other list is counted only.
                    assert_eq!(a.has_values(), a.is_repeated() || a.is_empty(), "{step}: {text}");
                    if a.is_repeated() {
                        assert_eq!(a.get(0), b.get(0), "{step}: {text}");
                    }
                }
            }
            (Err(Error::BadList { line: a, .. }), Err(Error::BadList { line: b, .. })) => {
                assert_eq!(*a, b, "{step}: the error's line: {text}")
            }
            (Err(_), Err(_)) => {}
            (w, o) => panic!("{step}: whole {:?}, outline {:?}: {text}", w.as_ref().err(), o.err()),
        }
    }
}

/// Whether a `List<` comes before the `FoamFile` header, in a file that may be binary.
fn list_before_header(bytes: &[u8]) -> bool {
    let at = |w: &[u8]| bytes.windows(w.len()).position(|x| x == w);
    at(b"binary").is_some() && at(b"List<").is_some_and(|l| at(b"FoamFile").is_none_or(|f| l < f))
}

#[test]
fn an_outline_is_the_field_without_its_values() {
    let h = |class| header("ascii", class, "");
    let samples: Vec<Vec<u8>> = vec![
        format!(
            "{}dimensions      [0 0 0 1 0 0 0];\n\ninternalField   nonuniform List<scalar> \n4\n(\n290\n290.5\n-1e-3\n3e+02\n)\n;\n\n{BOUNDARY}",
            h("volScalarField")
        )
        .into_bytes(),
        format!(
            "{}internalField   nonuniform List<vector> 2((1 0 0) ( 0.5 -2 3e1 ));\nboundaryField\n{{\n    in  {{ type fixedValue; value uniform (1.6852 0 0); }}\n    out {{ type fixedValue; value nonuniform List<vector> 3{{(0 0 1)}}; }}\n    e   {{ type empty; value nonuniform List<vector> 0(); }}\n    u   {{ type fixedValue; value nonuniform 2((1 2 3) (4 5 6)); }}\n}}\n",
            h("volVectorField")
        )
        .into_bytes(),
        format!("{}internalField nonuniform List<sphericalTensor> 2((1) (2));\n", h("volSphericalTensorField")).into_bytes(),
        format!("{}internalField nonuniform 2((1) (2));\n", h("volSphericalTensorField")).into_bytes(),
        format!("{}internalField nonuniform List<symmTensor> 1((1 2 3 4 5 6));\n", h("volSymmTensorField")).into_bytes(),
        format!("{}internalField nonuniform List<scalar> 3(nan -inf 1e400);\n", h("volScalarField")).into_bytes(),
        format!("{}internalField nonuniform List<scalar> 1(1.{});\n", h("volScalarField"), "0".repeat(200)).into_bytes(),
        format!("{}a List<label> 2(0 1);\nb List<label> 0()1;\nc List<label> 3{{7}};\n", h("regIOobject")).into_bytes(),
        format!(
            "{}internalField uniform 1;\ncoeffs {{ a List<scalar> (1 2 3); b List<scalar> ; c List<scalar> 2($x $y); d List<vector> 3(1 \"a;}}\" 2); }}\n",
            h("volScalarField")
        )
            .into_bytes(),
        // Comments, strings and code blocks that spell lists are text.
        format!(
            "{}// nonuniform List<scalar> 2(1 2)\nnote \"nonuniform List<scalar> 1(5)\";\ncode #{{ nonuniform List<scalar> 1(5) #}};\n/* List<label> 1(3) */\ninternalField nonuniform List<scalar> 1(5);\n",
            h("volScalarField")
        )
        .into_bytes(),
        // Words that spell placeholders.
        format!("{}w __umcfListA0;\ninternalField nonuniform List<scalar> 1(5);\n", h("volScalarField")).into_bytes(),
        // No header at all.
        b"internalField nonuniform List<scalar> 2(1 2);\n".to_vec(),
        // Errors: too few, too many, a word, a short vector, an unclosed header, an unknown type.
        format!("{}internalField nonuniform List<scalar> 5(1 2);\n", h("volScalarField")).into_bytes(),
        format!("{}internalField nonuniform List<scalar> 1(1 2);\n", h("volScalarField")).into_bytes(),
        format!("{}internalField nonuniform List<scalar> 2(1 x);\n", h("volScalarField")).into_bytes(),
        format!("{}internalField nonuniform List<scalar> 2($a $b);\n", h("volScalarField")).into_bytes(),
        format!("{}internalField nonuniform List<vector> 1((1 2));\n", h("volVectorField")).into_bytes(),
        format!("{}internalField nonuniform List<vector> 1((1 2 3 4));\n", h("volVectorField")).into_bytes(),
        format!("{}internalField nonuniform List<word> 1(a);\n", h("volScalarField")).into_bytes(),
        b"FoamFile { format ascii;\ninternalField nonuniform List<scalar> 1(1);\n".to_vec(),
        format!("{}internalField nonuniform List<scalar> 3(1 2", h("volScalarField")).into_bytes(),
        binary_field(Arch::default(), Kind::Vector, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        // Escaped quotes, wherever a chunk ends.
        format!("{}note \"a\\\" nonuniform List<scalar> 1(5) \\\\\";\ninternalField uniform 1;\n", h("volScalarField")).into_bytes(),
        // Errors on later lines name them.
        format!("{}internalField nonuniform List<scalar> 2(1\nx\n);\n", h("volScalarField")).into_bytes(),
        format!("{}internalField nonuniform List<scalar>\n\n2\n{{\nx}};\n", h("volScalarField")).into_bytes(),
        format!("{}internalField nonuniform List<scalar>\n\nx;\n", h("volScalarField")).into_bytes(),
        // A bare list that stays text, before a binary file's header.
        [b"x List<scalar> ;\n".to_vec(), binary_field(Arch::default(), Kind::Scalar, &[1.0])].concat(),
        // A binary list of size 0 written as its size alone, a word right after it: not a token.
        format!("{}x nonuniform List<scalar> 0List<scalar> 0;\n", header("binary", "volScalarField", "")).into_bytes(),
        // A length with leading zeros, more than 20 digits of it.
        format!("{}internalField nonuniform List<scalar> 0000000000000000000002(1 2);\n", h("volScalarField")).into_bytes(),
        format!("{}internalField nonuniform List<scalar> 0000(\n);\n", h("volScalarField")).into_bytes(),
        // Binary: an untyped list too long for the limit; data that ends where its `)` would be.
        format!("{}internalField nonuniform 99999999999999999(", header("binary", "volScalarField", "")).into_bytes(),
        [header("binary", "volScalarField", "").into_bytes(), b"internalField nonuniform List<scalar> 1(12345678".to_vec()].concat(),
        // Binary data that spells a placeholder.
        binary_field(Arch::default(), Kind::Scalar, &[f64::from_le_bytes(*b"__umcfLi"), f64::from_le_bytes(*b"stA0 x y")]),
        binary_field(Arch { big_endian: true, label_bytes: 8, scalar_bytes: 4 }, Kind::Label, &[1.0, 2.0, 3.0]),
        binary_field(
            Arch::default(),
            Kind::Scalar,
            &[f64::from_le_bytes(*b")\";//}{("), f64::from_le_bytes(*b"nonunifo"), f64::from_le_bytes(*b"rm List<")],
        ),
        format!(
            "{}internalField uniform (0 0 0);\nboundaryField\n{{\n    wall\n    {{\n        type fixedValue;\n        value nonuniform List<vector> \n0\n;\n    }}\n}}\n",
            header("binary", "volVectorField", "")
        )
        .into_bytes(),
    ];
    for s in &samples {
        outline_agrees(s, &[1, 2, 3, 7, 64, 1 << 20]);
    }
    // A truncated binary list.
    let mut cut = binary_field(Arch::default(), Kind::Vector, &[1.0, 2.0, 3.0]);
    cut.truncate(cut.len() - 200);
    outline_agrees(&cut, &[1, 1 << 20]);
}

#[test]
fn an_outline_reads_gzip_and_stays_within_its_limits() {
    let src =
        format!("{}internalField nonuniform List<scalar> 3(1 2 3);\n{BOUNDARY}", header("ascii", "volScalarField", ""));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(src.as_bytes()).unwrap();
    let gz = gz.finish().unwrap();
    let fs = umcf_core::MemFs::new().with("0/T.gz", gz.clone());
    let f = Field::outline(&fs, "0/T", &Limits::default(), u64::MAX).unwrap().unwrap();
    assert_eq!(f.doc.to_string(), Field::parse(&gz).unwrap().doc.to_string());
    assert_eq!(f.patch_type("ground").as_deref(), Some("fixedValue"));
    assert!(Field::outline(&fs, "0/U", &Limits::default(), u64::MAX).unwrap().is_none());
    // The file, decompressed, against `max_bytes`; what is kept (no list) against `max_text`.
    let small = Limits { max_bytes: gz.len() as u64 + 1, ..Limits::default() };
    assert!(src.len() > gz.len() + 1);
    assert!(matches!(Field::outline(&fs, "0/T", &small, u64::MAX), Err(Error::TooLarge { .. })));
    assert!(matches!(Field::outline(&fs, "0/T", &Limits::default(), 100), Err(Error::TooLarge { .. })));
    // A big list costs nothing against `max_text`: only its placeholder is kept.
    let many = (0..200_000).map(|i| format!("({i} 0 0)")).collect::<Vec<_>>().join("\n");
    let big = format!(
        "{}internalField nonuniform List<vector> 200000({many});\n{BOUNDARY}",
        header("ascii", "volVectorField", "")
    );
    let fs = umcf_core::MemFs::new().with("0/U", big.clone());
    let f = Field::outline(&fs, "0/U", &Limits::default(), 4096).unwrap().unwrap();
    let Some(Value::List(l)) = f.internal() else { panic!() };
    assert_eq!((l.kind, l.len(), l.has_values(), l.get(0), l.as_flat()), (Kind::Vector, 200_000, false, None, None));
    assert!(big.len() > 2_000_000);
    // And the list's length against `max_values`.
    let few = Limits { max_values: 1000, ..Limits::default() };
    assert!(matches!(Field::outline(&fs, "0/U", &few, u64::MAX), Err(Error::TooLarge { .. })));
    // Errors name their line, as the whole reader's do.
    let src = format!("{}internalField nonuniform List<scalar> 2(1\nx);\n", header("ascii", "volScalarField", ""));
    let line = src.lines().position(|l| l == "x);").unwrap() + 1;
    let e = outline_of(src.as_bytes(), 3).unwrap_err().to_string();
    assert!(e.starts_with(&format!("line {line}:")), "{e}");
}

/// The most the linear-time tests below may take where the quadratic cost they guard against
/// took minutes. About a second here in a debug build. Loose, so that a slow CI runner of another
/// system (macOS, Windows), running other tests at the same time, passes.
const LINEAR_SECS: u64 = 30;

#[test]
fn an_outline_bounds_comments_strings_and_failed_bare_lists() {
    // Review findings: an unclosed comment, string or code block was kept whole, past the text
    // bound; failed bare lists in a binary file read the same bytes ahead again and again.
    let h = header("ascii", "volScalarField", "");
    for open in ["/*", "\"", "#{", "//"] {
        let src = format!("{h}internalField uniform 1;\n{open}{}", "x".repeat(200_000));
        let fs = umcf_core::MemFs::new().with("0/T", src);
        let got = Field::outline(&fs, "0/T", &Limits::default(), 1000);
        assert!(matches!(got, Err(Error::TooLarge { .. })), "{open}: {:?}", got.err());
    }
    let mut src = header("binary", "volScalarField", "").into_bytes();
    src.extend_from_slice(b"junk (");
    for _ in 0..2_000 {
        src.extend_from_slice(b" List<scalar> 100000 (");
    }
    src.extend(vec![b'x'; 1 << 20]);
    let fs = umcf_core::MemFs::new().with("0/T", src.clone());
    #[allow(clippy::disallowed_methods)]
    let started = std::time::Instant::now();
    let got = Field::outline(&fs, "0/T", &Limits::default(), 4 << 20);
    // Each attempt reads up to 800 kB ahead: 2000 of them would be 1.6 GB, quadratic in the file.
    assert!(matches!(got, Err(Error::TooLarge { .. })), "{:?}", got.err());
    // Tighter than `LINEAR_SECS`: re-reading 1.6 GB may take seconds, not minutes (about 0.1 s
    // here, within this on macOS CI).
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
}

#[test]
fn failed_bare_lists_cost_linear_time_in_an_outline() {
    // Review findings: each failed bare list searched the text so far for the header again, and
    // `N{` attempts read ahead without counting it: minutes for a few MB.
    #[allow(clippy::disallowed_methods)]
    let started = std::time::Instant::now();
    let src = "List<scalar> ;\n".repeat(200_000);
    outline_agrees(src.as_bytes(), &[1 << 20]);
    let src = format!("{}{}", header("ascii", "volScalarField", ""), "List<scalar> 1{".repeat(200_000));
    let fs = umcf_core::MemFs::new().with("0/T", src);
    let _ = Field::outline(&fs, "0/T", &Limits::default(), 16 << 20);
    let src = format!("FoamFile {{ format ascii;\n{}", "List<scalar> ;\n".repeat(200_000));
    let _ = outline_of(src.as_bytes(), 1 << 20);
    assert!(started.elapsed().as_secs() < LINEAR_SECS, "{:?}", started.elapsed());
}

#[test]
fn an_outline_bounds_what_its_lists_cost_and_reads_a_bad_header_once() {
    // Review findings: a file of empty lists grew without bound (each list kept, nothing charged);
    // a header that doesn't read was parsed again for every bare list after it.
    let src = format!("{}a {}", header("ascii", "volScalarField", ""), "nonuniform 0()".repeat(100_000));
    let fs = umcf_core::MemFs::new().with("0/T", src);
    let got = Field::outline(&fs, "0/T", &Limits::default(), 100_000);
    assert!(matches!(got, Err(Error::TooLarge { .. })), "{:?}", got.err());
    #[allow(clippy::disallowed_methods)]
    let started = std::time::Instant::now();
    let src = format!(
        "FoamFile {{ format ascii; arch \"bogus\"; /*{}*/ }}\n{}",
        "x".repeat(1 << 20),
        "List<scalar> ;\n".repeat(16_000)
    );
    let fs = umcf_core::MemFs::new().with("0/T", src);
    assert!(Field::outline(&fs, "0/T", &Limits::default(), 16 << 20).is_err());
    assert!(started.elapsed().as_secs() < LINEAR_SECS, "{:?}", started.elapsed());
}
