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
