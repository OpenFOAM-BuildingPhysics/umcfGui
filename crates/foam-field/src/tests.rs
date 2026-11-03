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
    let chain = "List<scalar>1{x;".repeat(40_000);
    let src = format!("{h}internalField uniform 1;\na \"{chain}\";\n");
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
