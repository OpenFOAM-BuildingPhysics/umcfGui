use super::*;

const FV_SCHEMES: &str = r#"/*--------------------------------*- C++ -*----------------------------------*\
| =========                 |                                                 |
\*---------------------------------------------------------------------------*/
FoamFile
{
    version     2.0;
    format      ascii;
    class       dictionary;
    location    "system";
    object      fvSchemes;
}
// * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * //

ddtSchemes
{
    default         steadyState;
}

divSchemes
{
    default         none;
    div(phi,U)      bounded Gauss linearUpwind grad(U); // momentum
    div(((rho*nuEff)*dev2(T(grad(U))))) Gauss linear;
}

// ************************************************************************* //
"#;

const FIELD: &str = r#"FoamFile { version 2.0; format ascii; class volVectorField; object U; }
#include "include/initialConditions"
dimensions      [0 1 -1 0 0 0 0];
internalField   uniform $flowVelocity;
boundaryField
{
    #includeEtc "caseDicts/setConstraintTypes"
    inlet
    {
        type            fixedValue;
        value           $internalField;
    }
    "(wall|ground).*" { type noSlip; }
    $:defaults;
    code
    {
        code #{ Info<< "hi" << endl; #};
        n    #calc "$a * 2";
        v    3{0.5};
    }
}
"#;

fn round_trip(src: &str) {
    let doc = Document::parse(src).unwrap();
    assert_eq!(doc.to_string(), src);
}

#[test]
fn round_trips_byte_for_byte() {
    round_trip(FV_SCHEMES);
    round_trip(FIELD);
    round_trip("");
    round_trip("   \n// only a comment\n");
    round_trip("a 1;;b 2;\n");
    round_trip("missing semi 1\n");
    round_trip("d { x 1 }\n");
    round_trip("5\n(\n(0 0 0)\n(1 0 0)\n)\n");
    round_trip("(\t\t\t\n(\t0\t0\t)\n(\t3600\t223.7\t)\n)\n");
}

#[test]
fn function_call_directives() {
    let src = "functions\n{\n    #includeFunc fieldAverage(U, p, prime2Mean = yes)\n    #includeFunc residuals(p, sigma)\n\n    #includeFunc cutPlaneSurface\n    (\n        point=(0 0 0),\n        normal=(0 0 1),\n        fields=(alpha.water p_rgh)\n    )\n    #includeFunc patchSurface(patch=\".*(Walls|NonCouple)\")\n    #includeConstraint limitPressure(minFactor=0.1, maxFactor=2)\n}\nnext 1;\n";
    let doc = Document::parse(src).unwrap();
    assert_eq!(doc.to_string(), src);
    let f = doc.get(&["functions"]).and_then(Entry::dict).unwrap();
    let names: Vec<_> = f.directives().map(|(n, _)| n).collect();
    assert_eq!(names, ["#includeFunc"; 4].into_iter().chain(["#includeConstraint"]).collect::<Vec<_>>());
    assert_eq!(doc.value(&["next"]).as_deref(), Some("1"));
}

#[test]
fn bare_list_data_files() {
    // constant/IDN, sunPosVector, T_room.dat: OpenFOAM reads these as a plain List.
    let doc = Document::parse("(\n(0 298.15)\n(518400 298.15)\n)\n").unwrap();
    let data: Vec<String> = doc.root.values().map(Node::compact).collect();
    assert_eq!(data, ["((0 298.15) (518400 298.15))"]);
    assert_eq!(doc.root.entries().count(), 0);
    assert!(Document::parse("] x").is_err());
}

#[test]
fn lookups() {
    let doc = Document::parse(FV_SCHEMES).unwrap();
    assert_eq!(doc.header("class").as_deref(), Some("dictionary"));
    assert_eq!(doc.header("location").as_deref(), Some("system"));
    assert_eq!(doc.value(&["divSchemes", "div(phi,U)"]).as_deref(), Some("bounded Gauss linearUpwind grad(U)"));
    assert_eq!(doc.value(&["ddtSchemes", "default"]).as_deref(), Some("steadyState"));
    assert!(doc.get(&["divSchemes", "nope"]).is_none());
    assert!(doc.get(&[]).is_none());
    let keys: Vec<_> = doc.root.keys().collect();
    assert_eq!(keys, ["FoamFile", "ddtSchemes", "divSchemes"]);

    let doc = Document::parse(FIELD).unwrap();
    assert_eq!(doc.value(&["dimensions"]).as_deref(), Some("[0 1 -1 0 0 0 0]"));
    assert_eq!(doc.value(&["internalField"]).as_deref(), Some("uniform $flowVelocity"));
    assert_eq!(doc.value(&["boundaryField", "(wall|ground).*", "type"]).as_deref(), Some("noSlip"));
    assert_eq!(doc.value(&["boundaryField", "code", "n"]).as_deref(), Some("#calc \"$a * 2\""));
    let dirs: Vec<_> = doc.root.directives().collect();
    assert_eq!(dirs, [("#include", Some("include/initialConditions".to_string()))]);
    let bf = doc.get(&["boundaryField"]).and_then(Entry::dict).unwrap();
    assert!(bf.items.iter().any(|i| matches!(i, Item::Macro { name, .. } if name.text == "$:defaults")));
    assert!(bf.items.iter().any(|i| matches!(i, Item::Directive { name, .. } if name.text == "#includeEtc")));
}

#[test]
fn last_duplicate_wins() {
    let doc = Document::parse("a 1;\na 2;\n").unwrap();
    assert_eq!(doc.get(&["a"]).and_then(Entry::as_f64), Some(2.0));
}

#[test]
fn set_existing_keeps_alignment_and_comments() {
    let mut doc = Document::parse(FV_SCHEMES).unwrap();
    doc.set(&["divSchemes", "div(phi,U)"], "bounded Gauss upwind").unwrap();
    let out = doc.to_string();
    assert!(out.contains("    div(phi,U)      bounded Gauss upwind; // momentum\n"), "{out}");
    assert_eq!(out.len(), FV_SCHEMES.len() - "linearUpwind grad(U)".len() + "upwind".len());
}

#[test]
fn set_missing_appends_like_siblings() {
    let mut doc = Document::parse(FV_SCHEMES).unwrap();
    doc.set(&["ddtSchemes", "ddt(rho,U)"], "Euler").unwrap();
    let out = doc.to_string();
    assert!(out.contains("    default         steadyState;\n    ddt(rho,U)      Euler;\n}"), "{out}");
}

#[test]
fn set_creates_parent_dictionaries() {
    let mut doc = Document::parse(FV_SCHEMES).unwrap();
    doc.set(&["laplacianSchemes", "default"], "Gauss linear corrected").unwrap();
    let out = doc.to_string();
    let expect = "\n\nlaplacianSchemes\n{\n    default         Gauss linear corrected;\n}\n\n// ****";
    assert!(out.contains(expect), "{out}");
    // Reparse: the edit is valid OpenFOAM syntax.
    let again = Document::parse(&out).unwrap();
    assert_eq!(again.value(&["laplacianSchemes", "default"]).as_deref(), Some("Gauss linear corrected"));
    assert!(again.root.problems().is_empty());
}

#[test]
fn set_dictionary_value() {
    let mut doc = Document::parse(FIELD).unwrap();
    doc.set(&["boundaryField", "outlet"], "{ type inletOutlet; inletValue uniform (0 0 0); }").unwrap();
    let out = doc.to_string();
    let expect =
        "\n    outlet\n    {\n        type            inletOutlet;\n        inletValue      uniform (0 0 0);\n    }\n}";
    assert!(out.contains(expect), "{out}");
    assert_eq!(
        Document::parse(&out).unwrap().value(&["boundaryField", "outlet", "type"]).as_deref(),
        Some("inletOutlet")
    );
}

#[test]
fn set_into_value_entry_is_an_error() {
    let mut doc = Document::parse("a 1;\n").unwrap();
    assert_eq!(doc.set(&["a", "b"], "2"), Err(Error::NotADict("a".into())));
    assert_eq!(doc.set(&[], "2"), Err(Error::EmptyPath));
    assert!(doc.set(&["a"], "2;").is_err());
    assert!(doc.set(&["a"], "(1 2").is_err());
    // A dictionary ends the entry: what follows would be read back as another entry.
    assert!(doc.set(&["a"], "{ b 1; } c 2").is_err());
    assert!(doc.set(&["a"], "{ b 1; } {}").is_err());
    assert_eq!(doc.to_string(), "a 1;\n");
    // Something in front of the dictionary is fine (`#codeStream { … }`).
    doc.set(&["a"], "#codeStream { code #{ #}; }").unwrap();
    assert!(Document::parse(&doc.to_string()).unwrap().root.problems().is_empty(), "{doc}");
}

#[test]
fn quoted_keys() {
    let mut doc = Document::parse("x { }\n").unwrap();
    doc.set(&["x", "(U|k) cells"], "1").unwrap();
    assert!(doc.to_string().contains("\n    \"(U|k) cells\"   1;"), "{}", doc);
    assert_eq!(Document::parse(&doc.to_string()).unwrap().value(&["x", "(U|k) cells"]).as_deref(), Some("1"));
    // A key starting with `(` would read back as a list unquoted.
    doc.set(&["x", "(U|k)"], "2").unwrap();
    assert!(doc.to_string().contains("\n    \"(U|k)\"         2;"), "{}", doc);
    assert_eq!(Document::parse(&doc.to_string()).unwrap().value(&["x", "(U|k)"]).as_deref(), Some("2"));
    // A pattern that doesn't start with `(` is quoted too: unquoted, it isn't one.
    doc.set(&["x", "k.*"], "4").unwrap();
    doc.set(&["x", "h|e"], "5").unwrap();
    assert!(doc.to_string().contains("\n    \"k.*\"           4;\n    \"h|e\"           5;"), "{}", doc);
    // `div(phi,U)` stays unquoted.
    doc.set(&["x", "div(phi,U)"], "3").unwrap();
    assert!(doc.to_string().contains("\n    div(phi,U)      3;"), "{}", doc);
}

#[test]
fn remove_entries() {
    let mut doc = Document::parse(FV_SCHEMES).unwrap();
    assert!(doc.remove(&["divSchemes", "div(phi,U)"]).unwrap());
    assert!(!doc.remove(&["divSchemes", "div(phi,U)"]).unwrap());
    assert!(!doc.remove(&["nope", "x"]).unwrap());
    let out = doc.to_string();
    assert!(out.contains("    default         none;\n    div(((rho"), "{out}");
    assert!(!out.contains("momentum"));
    // The last entry of a dictionary and of the file, with a trailing comment each.
    let mut doc = Document::parse("d\n{\n    a 1;\n    b 2; // bee\n}\nc 3; // sea\n").unwrap();
    assert!(doc.remove(&["d", "b"]).unwrap());
    assert!(doc.remove(&["c"]).unwrap());
    assert_eq!(doc.to_string(), "d\n{\n    a 1;\n}\n");
    // Duplicates all go, so an earlier one doesn't resurface.
    let mut doc = Document::parse("a 1;\na 2;\nb 3;\n").unwrap();
    assert!(doc.remove(&["a"]).unwrap());
    assert_eq!(doc.to_string(), "\nb 3;\n");
}

#[test]
fn problems_are_reported_not_fatal() {
    let doc = Document::parse("a 1\n").unwrap();
    assert_eq!(doc.root.problems(), ["a: missing `;`"]);
    let doc = Document::parse("d { x 1; ; }\n").unwrap();
    assert_eq!(doc.root.problems(), ["d: stray `;`"]);
}

#[test]
fn errors_have_positions() {
    let e = Document::parse("a {\n  b (1 2;\n}\n").unwrap_err();
    assert!(matches!(e, Error::Unexpected { at: (3, 1), .. }), "{e}");
    let e = Document::parse("a {\n b 1;\n").unwrap_err();
    assert!(matches!(e, Error::Unclosed { open: '{', at: (1, 3) }), "{e}");
    // A stray `}` at the top level is kept (OpenFOAM tolerates it) and reported.
    let doc = Document::parse("a 1;\n}\nb 2;\n").unwrap();
    assert_eq!(doc.to_string(), "a 1;\n}\nb 2;\n");
    assert_eq!(doc.root.problems(), ["stray `}`"]);
    assert_eq!(doc.value(&["b"]).as_deref(), Some("2"));
    assert_eq!(Document::parse_bytes(&[0xff, 0xfe, 0x00]), Err(Error::NotText));
}

#[test]
fn limits_hold() {
    let deep = "(".repeat(10_000);
    assert!(matches!(Document::parse(&format!("a {deep};")), Err(Error::TooDeep { .. })));
    let small = Limits { max_bytes: 10, ..Limits::default() };
    assert!(matches!(Document::parse_with("a 1; b 2; c 3;", &small), Err(Error::TooLarge { .. })));
    let few = Limits { max_tokens: 5, ..Limits::default() };
    assert!(matches!(Document::parse_with("a 1; b 2; c 3;", &few), Err(Error::TooLarge { .. })));
}

/// xorshift: deterministic pseudo-random input without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn fuzz_never_panics_and_round_trips() {
    const ALPHABET: &[&str] = &[
        "a", "1", " ", "\n", "{", "}", "(", ")", "[", "]", ";", "\"", "\\", "//", "/*", "*/", "#{", "#}", "$", "${",
        "#include", "é", "-", "e5",
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let seeds = std::env::var("UMCF_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(20_000);
    for _ in 0..seeds {
        let len = rng.below(40);
        let src: String = (0..len).map(|_| ALPHABET[rng.below(ALPHABET.len())]).collect();
        if let Ok(doc) = Document::parse(&src) {
            assert_eq!(doc.to_string(), src, "round trip failed for {src:?}");
            let _ = doc.root.problems();
            let mut d = doc.clone();
            let _ = d.set(&["a", "b"], "1");
            let _ = d.remove(&["a"]);
            let _ = Document::parse(&d.to_string());
        }
    }
}

#[test]
fn fuzz_mutations_of_real_files() {
    let mut rng = Rng(42);
    for base in [FV_SCHEMES, FIELD] {
        let bytes = base.as_bytes();
        for _ in 0..2_000 {
            let mut m = bytes.to_vec();
            for _ in 0..1 + rng.below(4) {
                if m.is_empty() {
                    break;
                }
                let i = rng.below(m.len());
                match rng.below(3) {
                    0 => {
                        m.remove(i);
                    }
                    1 => m.insert(i, b"{}();\"#$/*"[rng.below(10)]),
                    _ => m.truncate(i),
                }
            }
            if let Ok(doc) = Document::parse_bytes(&m) {
                assert_eq!(doc.to_string().as_bytes(), &m[..]);
            }
        }
    }
}

#[test]
fn append_adds_a_list_item_like_the_ones_before() {
    let src = "pointSync false;\n\npatches\n(\n    // the inlet\n    {\n        name in;\n        constructFrom patches;\n        patches (a);\n    }\n);\n\n// end\n";
    let mut doc = Document::parse(src).unwrap();
    doc.append(&["patches"], "{ name roof; patchInfo { type wall; } constructFrom set; set roof; }").unwrap();
    let out = doc.to_string();
    let added = "    }\n    {\n        name            roof;\n        patchInfo\n        {\n            type            wall;\n        }\n        constructFrom   set;\n        set             roof;\n    }\n);\n\n// end\n";
    assert!(out.starts_with(&src[..src.find("    }\n);").unwrap()]), "{out}");
    assert!(out.ends_with(added), "{out}");
    let again = Document::parse(&out).unwrap();
    assert!(again.root.problems().is_empty());

    // An empty list on one line gets the item on its own line, and the `)` after it.
    let mut doc = Document::parse("patches ();\n").unwrap();
    doc.append(&["patches"], "b").unwrap();
    assert_eq!(doc.to_string(), "patches (\n    b\n);\n");
    // A comment before the `)` stays after the item before it.
    let mut doc = Document::parse("patches ( a /* note */ );\n").unwrap();
    doc.append(&["patches"], "b").unwrap();
    assert_eq!(doc.to_string(), "patches ( a /* note */ \n    b\n);\n");
    let mut doc = Document::parse("patches ( a // note\n);\n").unwrap();
    doc.append(&["patches"], "b").unwrap();
    assert_eq!(doc.to_string(), "patches ( a // note\n    b\n);\n");
    // A block comment over lines before the `)`: the item goes after it, not into it.
    let mut doc = Document::parse("patches ( a /* x\n   y */ );\n").unwrap();
    doc.append(&["patches"], "b").unwrap();
    let out = doc.to_string();
    assert_eq!(out, "patches ( a /* x\n   y */ \n    b\n);\n");
    let again = Document::parse(&out).unwrap();
    let Some([Node::List { items, .. }]) = again.get(&["patches"]).map(|e| e.value.as_slice()) else { panic!("{out}") };
    assert_eq!(items.len(), 2, "{out}");
    let mut doc = Document::parse("patches\n(\n    a\n    /* one\n       two */\n);\n").unwrap();
    doc.append(&["patches"], "b").unwrap();
    assert_eq!(doc.to_string(), "patches\n(\n    a\n    /* one\n       two */\n    b\n);\n");
    // A comment on the `)`'s line stays after `a`.
    let mut doc = Document::parse("patches\n(\n    a\n    /* one */ );\n").unwrap();
    doc.append(&["patches"], "b").unwrap();
    assert_eq!(doc.to_string(), "patches\n(\n    a\n    /* one */ \n    b\n);\n");
    let mut doc = Document::parse("patches\n(\n    a\n    // the end\n);\n").unwrap();
    doc.append(&["patches"], "b").unwrap();
    assert_eq!(doc.to_string(), "patches\n(\n    a\n    // the end\n    b\n);\n");

    assert_eq!(doc.append(&["pointSync"], "b"), Err(Error::NotFound("pointSync".into())));
    let mut doc = Document::parse("pointSync false;\n").unwrap();
    assert_eq!(doc.append(&["pointSync"], "b"), Err(Error::NotAList("pointSync".into())));
    assert!(doc.append(&["pointSync"], "a b").is_err());
    assert_eq!(doc.to_string(), "pointSync false;\n");
}

/// `Document::rename` changes only the key, and says when there is nothing to rename.
#[test]
fn rename_at_a_path_keeps_the_rest() {
    let text = "a\n{\n    // the west\n    west            { role inlet; }\n    east            { role outlet; }\n}\n";
    let mut doc = Document::parse(text).unwrap();
    assert!(doc.rename(&["a", "west"], "upwind").unwrap());
    assert_eq!(doc.to_string(), text.replace("west            {", "upwind            {"));
    assert!(!doc.rename(&["a", "north"], "x").unwrap());
    assert!(!doc.rename(&["b", "west"], "x").unwrap());
    assert!(doc.rename(&["a", "east", "role", "x"], "y").is_err());
    assert!(doc.rename(&[], "y").is_err());
}

/// `Document::set_inline` keeps a dictionary value on the key's line, new or replaced.
#[test]
fn an_inline_dictionary_stays_on_its_line() {
    let text = "p\n{\n    // west\n    west            { role inlet; }\n    odd             { }\n    east\n    {\n        role outlet;\n    }\n}\n";
    let mut d = Document::parse(text).unwrap();
    d.set_inline(&["p", "west"], "{ role outlet; partner ( a b ); }").unwrap();
    d.set_inline(&["p", "odd"], "{ role side; }").unwrap();
    d.set_inline(&["p", "east"], "{ role side; }").unwrap();
    d.set_inline(&["p", "new"], "{ role top; }").unwrap();
    d.set_inline(&["q", "a"], "{ }").unwrap();
    let out = d.to_string();
    assert!(out.contains("    // west\n    west            { role outlet; partner ( a b ); }\n"), "{out}");
    assert!(out.contains("    odd             { role side; }\n"), "{out}");
    assert!(out.contains("    east            { role side; }\n"), "{out}");
    assert!(out.contains("    new             { role top; }\n"), "{out}");
    assert!(out.contains("q\n{\n    a               { }\n}"), "{out}");
    // It reads back as the same entries.
    let back = Document::parse(&out).unwrap();
    assert_eq!(back.value(&["p", "east", "role"]).as_deref(), Some("side"));
    assert_eq!(back.to_string(), out);
}

/// Plain `set` keeps a value's own layout: on the line after its key, it stays there.
#[test]
fn a_value_on_the_next_line_stays_there_when_set() {
    let text = "a\n{\n    value\n        uniform 0;\n}\n";
    let mut d = Document::parse(text).unwrap();
    d.set(&["a", "value"], "uniform 1").unwrap();
    assert_eq!(d.to_string(), "a\n{\n    value\n        uniform 1;\n}\n");
}

/// A comment in front of an inline dictionary's brace stays when it is set again.
#[test]
fn a_comment_before_an_inline_dictionary_stays() {
    let text = "p\n{\n    west // the wind\n    {\n        role inlet;\n    }\n}\n";
    let mut d = Document::parse(text).unwrap();
    d.set_inline(&["p", "west"], "{ role side; }").unwrap();
    let out = d.to_string();
    assert!(out.contains("west // the wind\n    { role side; }"), "{out}");
    assert_eq!(Document::parse(&out).unwrap().value(&["p", "west", "role"]).as_deref(), Some("side"));
}

#[test]
fn a_new_last_entry_leaves_the_comment_on_the_line_before_where_it_was() {
    // At the root: the comment is in the document's tail.
    let mut doc = Document::parse("a 1; // one\nb 2; // two\n").unwrap();
    doc.set(&["c"], "3").unwrap();
    assert_eq!(doc.to_string(), "a 1; // one\nb 2; // two\nc 3;\n");
    // In a dictionary: in its closing brace's lead; with a new parent, too.
    let mut doc = Document::parse("d\n{\n    a 1; // one\n}\nx 1; /* x */\n").unwrap();
    doc.set(&["d", "b"], "2").unwrap();
    doc.set(&["e", "f"], "3").unwrap();
    assert_eq!(doc.to_string(), "d\n{\n    a 1; // one\n    b 2;\n}\nx 1; /* x */\ne\n{\n    f               3;\n}\n");
    // A block comment going on past the line stays whole, the new entry after it.
    let mut doc = Document::parse("a 1;\nb 2; /* two\n  more */\n").unwrap();
    doc.set(&["c"], "3").unwrap();
    assert_eq!(doc.to_string(), "a 1;\nb 2; /* two\n  more */\nc 3;\n");
    assert_eq!(doc.value(&["c"]).as_deref(), Some("3"));
    let mut doc = Document::parse("d\n{\n    a 1; /* x\n    y */\n}\n").unwrap();
    doc.set(&["d", "b"], "2").unwrap();
    assert_eq!(doc.to_string(), "d\n{\n    a 1; /* x\n    y */\n    b 2;\n}\n");
    // Several on the line, and what is on the lines after stays after.
    let mut doc = Document::parse("a 1; /* x */ // y\n// z\n").unwrap();
    doc.set(&["b"], "2").unwrap();
    assert_eq!(doc.to_string(), "a 1; /* x */ // y\nb 2;\n// z\n");
    // A block comment that never ends is left where it is.
    let mut doc = Document::parse("a 1;").unwrap();
    doc.tail = " /* open".into();
    doc.set(&["b"], "2").unwrap();
    assert!(doc.to_string().ends_with(" /* open"));
    // A comment on a line of its own after the last entry stays where it is.
    let mut doc = Document::parse("a 1;\n// end\n").unwrap();
    doc.set(&["b"], "2").unwrap();
    assert_eq!(doc.to_string(), "a 1;\nb 2;\n// end\n");
}

#[test]
fn switches_read_as_openfoam_reads_them() {
    for w in ["true", "on", "yes", "y", "t", "any", "1", "-2", "\"yes\""] {
        assert_eq!(crate::switch(w), Some(true), "{w}");
    }
    for w in ["false", "off", "no", "n", "f", "none", "0"] {
        assert_eq!(crate::switch(w), Some(false), "{w}");
    }
    for w in ["maybe", "1.5", "", "True"] {
        assert_eq!(crate::switch(w), None, "{w}");
    }
}

#[test]
fn a_word_goes_into_the_list_after_its_key_word() {
    let add = |src: &str, word: &str, value: &str| {
        let mut doc = Document::parse(src).unwrap();
        doc.add_to_sublist("regions", word, value).unwrap();
        doc.to_string()
    };
    let tutorial =
        "regions\n(\n    fluid   (air)\n\tsolid   (street windward leeward)\n    vegetation ()\n);\n// end\n";
    assert_eq!(
        add(tutorial, "solid", "roofs"),
        "regions\n(\n    fluid   (air)\n\tsolid   (street windward leeward roofs)\n    vegetation ()\n);\n// end\n"
    );
    assert_eq!(
        add(tutorial, "vegetation", "vegetation"),
        "regions\n(\n    fluid   (air)\n\tsolid   (street windward leeward)\n    vegetation (vegetation)\n);\n// end\n"
    );
    assert_eq!(add("regions ( fluid ( ) );", "fluid", "air"), "regions ( fluid ( air ) );");
    // A word that isn't there: a new pair, its list at the first pair's column.
    assert_eq!(
        add("regions\n(\n    fluid       (air)\n);\n", "solid", "street"),
        "regions\n(\n    fluid       (air)\n    solid       (street)\n);\n"
    );
    // On their own lines, items stay so.
    assert_eq!(
        add("regions\n(\n    solid\n    (\n        a\n    )\n);\n", "solid", "b"),
        "regions\n(\n    solid\n    (\n        a\n        b\n    )\n);\n"
    );
    let mut doc = Document::parse("regions { fluid (air); }").unwrap();
    assert!(doc.add_to_sublist("regions", "solid", "x").is_err());
    let mut doc = Document::parse("regions ( fluid air );").unwrap();
    assert!(doc.add_to_sublist("regions", "fluid", "x").is_err());
}

#[test]
fn a_word_added_to_a_list_keeps_its_layout_and_comments() {
    let add = |src: &str| {
        let mut doc = Document::parse(src).unwrap();
        doc.add_word(&["regions", "solid"], "roofs").unwrap();
        doc.to_string()
    };
    assert_eq!(add("regions { solid ( street ); }"), "regions { solid ( street roofs ); }");
    assert_eq!(add("regions { solid (street); }"), "regions { solid (street roofs); }");
    assert_eq!(add("regions { solid (street /* c */); }"), "regions { solid (street /* c */ roofs); }");
    assert_eq!(add("regions { solid (); }"), "regions { solid (roofs); }");
    assert_eq!(add("regions { solid ( ); }"), "regions { solid ( roofs ); }");
    assert_eq!(add("regions { solid ( /* none */ ); }"), "regions { solid ( /* none */ roofs ); }");
    assert_eq!(
        add(
            "regions\n{\n    solid\n        (\n            street // the street\n            /* end */\n        );\n}\n"
        ),
        "regions\n{\n    solid\n        (\n            street // the street\n            /* end */\n            roofs\n        );\n}\n"
    );
    let mut doc = Document::parse("regions { solid 1(street); }").unwrap();
    assert!(doc.add_word(&["regions", "solid"], "roofs").is_err());
    // One line, a pair that isn't there: on the line.
    let mut doc = Document::parse("regions ( fluid (air) solid (street) );").unwrap();
    doc.add_to_sublist("regions", "vegetation", "vegetation").unwrap();
    assert_eq!(doc.to_string(), "regions ( fluid (air) solid (street) vegetation (vegetation) );");
}

#[test]
fn a_word_added_after_a_line_comment_gets_a_line_of_its_own() {
    let add = |src: &str| {
        let mut doc = Document::parse(src).unwrap();
        doc.add_word(&["regions", "solid"], "roofs").unwrap();
        let out = doc.to_string();
        // Read again, the list has the word: no comment swallowed it.
        let again = Document::parse(&out).unwrap();
        assert!(again.value(&["regions", "solid"]).unwrap().contains("roofs"), "{out}");
        out
    };
    assert_eq!(add("regions { solid (a // c\n); }"), "regions { solid (a // c\n    roofs\n); }");
    assert_eq!(add("regions { solid ( // none\n); }"), "regions { solid ( // none\n    roofs\n); }");
    assert_eq!(add("regions { solid (a // c\n    ); }"), "regions { solid (a // c\n        roofs\n    ); }");
    // OpenFOAM-8: in a pair's list, and a new pair after a comment.
    let mut doc = Document::parse("regions ( fluid (air) solid ( street // x\n    ) );").unwrap();
    doc.add_to_sublist("regions", "solid", "roofs").unwrap();
    assert_eq!(doc.to_string(), "regions ( fluid (air) solid ( street // x\n        roofs\n    ) );");
    let mut doc = Document::parse("regions ( fluid (air) solid (street) // c\n);").unwrap();
    doc.add_to_sublist("regions", "vegetation", "vegetation").unwrap();
    let out = doc.to_string();
    assert_eq!(out, "regions ( fluid (air) solid (street) // c\n    vegetation (vegetation)\n);");
    // No spaces left at the end of a line.
    let mut doc = Document::parse("regions ( );").unwrap();
    doc.add_to_sublist("regions", "vegetation", "vegetation").unwrap();
    assert_eq!(doc.to_string(), "regions (\n    vegetation (vegetation)\n);");
}

#[test]
fn what_isn_t_one_word_isn_t_added_to_a_list() {
    for bad in ["", "roof //", "a/*b", "a b", "(a)", "a;", "\"a\"", "a}", "a\tb"] {
        let src = "regions { solid ( street ); }";
        let mut doc = Document::parse(src).unwrap();
        assert!(matches!(doc.add_word(&["regions", "solid"], bad), Err(Error::NotAWord(_))), "{bad:?}");
        assert_eq!(doc.to_string(), src, "{bad:?}");
        let src = "regions ( fluid (air) );";
        let mut doc = Document::parse(src).unwrap();
        assert!(matches!(doc.add_to_sublist("regions", "fluid", bad), Err(Error::NotAWord(_))), "{bad:?}");
        assert!(matches!(doc.add_to_sublist("regions", bad, "x"), Err(Error::NotAWord(_))), "{bad:?}");
        assert_eq!(doc.to_string(), src, "{bad:?}");
    }
    // A word with a slash, a dot or a dollar is one.
    let mut doc = Document::parse("regions { solid ( street ); }").unwrap();
    doc.add_word(&["regions", "solid"], "a/b.c").unwrap();
    assert_eq!(doc.to_string(), "regions { solid ( street a/b.c ); }");
}

#[test]
fn new_entries_follow_their_siblings_and_removals_keep_the_comments_around_them() {
    // Aligned like the values before it; on the line of a dictionary written on one.
    let mut doc = Document::parse("m\n{\n    rho     1600; // kg/m3\n    mu      3.5;  // vapour\n}\n").unwrap();
    doc.set(&["m", "wcap"], "2").unwrap();
    assert_eq!(doc.to_string(), "m\n{\n    rho     1600; // kg/m3\n    mu      3.5;  // vapour\n    wcap    2;\n}\n");
    // The last one taken out: its comment goes, the one before it stays.
    doc.remove(&["m", "wcap"]).unwrap();
    doc.remove(&["m", "mu"]).unwrap();
    assert_eq!(doc.to_string(), "m\n{\n    rho     1600; // kg/m3\n}\n");
    // One in the middle.
    let mut doc = Document::parse("a 1; // one\nb 2; // two\nc 3; // three\n").unwrap();
    doc.remove(&["b"]).unwrap();
    assert_eq!(doc.to_string(), "a 1; // one\nc 3; // three\n");
    // In a list's dictionary written on one line.
    let mut doc = Document::parse("l\n(\n    { name soil; rho 1600; mu 3.5; }\n);\n").unwrap();
    let Some(Node::List { items, .. }) = doc.root.get_mut("l").and_then(|e| e.value.first_mut()) else { panic!() };
    items[0].dict_set(&["wcap"], "2").unwrap();
    items[0].dict_remove(&["mu"]).unwrap();
    assert_eq!(doc.to_string(), "l\n(\n    { name soil; rho 1600; wcap 2; }\n);\n");
}

#[test]
fn edits_next_to_comments_never_hide_an_entry_in_one() {
    // A new entry after a `//` comment on a one-line dictionary, or at the root.
    for (src, path, want) in [
        ("d { a 1; b 2; // note\n}\n", &["d", "c"][..], &["d", "c"][..]),
        ("a 1; b 2; // note\n", &["c"][..], &["c"][..]),
    ] {
        let mut doc = Document::parse(src).unwrap();
        doc.set(path, "3").unwrap();
        let out = doc.to_string();
        let back = Document::parse(&out).unwrap();
        assert_eq!(back.value(want).as_deref(), Some("3"), "{out}");
        assert!(out.contains("// note\n"), "{out}");
    }
    // In a list's dictionary.
    let mut doc = Document::parse("l ( { name s; rho 1; // kg\n } );\n").unwrap();
    let Some(Node::List { items, .. }) = doc.root.get_mut("l").and_then(|e| e.value.first_mut()) else { panic!() };
    items[0].dict_set(&["wcap"], "2").unwrap();
    let out = doc.to_string();
    assert!(out.contains("// kg\n") && Document::parse(&out).is_ok(), "{out}");
    // A removal after a `//` comment: what follows stays an entry.
    let mut doc = Document::parse("a 1; // x\nb 2; c 3;\n").unwrap();
    doc.remove(&["b"]).unwrap();
    let out = doc.to_string();
    assert_eq!(Document::parse(&out).unwrap().value(&["c"]).as_deref(), Some("3"), "{out}");
    assert!(out.contains("a 1; // x\n"), "{out}");
    // A block comment over lines stays whole; the removed entry's own goes.
    for (src, gone, keep) in [
        ("a 1; /* x\n y */\nb 2;\nc 3;\n", "b", Some("c")),
        ("a 1; /* x\n y */\nb 2;\n", "b", None),
        ("a 1;\nb 2; /* bee */\nc 3;\n", "b", Some("c")),
    ] {
        let mut doc = Document::parse(src).unwrap();
        doc.remove(&[gone]).unwrap();
        let out = doc.to_string();
        let back = Document::parse(&out).unwrap_or_else(|e| panic!("{src:?} → {out:?}: {e}"));
        if let Some(k) = keep {
            assert_eq!(back.value(&[k]).as_deref(), Some("3"), "{out}");
        }
        assert!(!out.contains("bee"), "{out}");
        assert_eq!(out.contains("/* x\n y */"), src.contains("/* x\n y */"), "{out}");
    }
    // A dictionary whose last line holds two entries isn't one written on one line.
    let mut doc = Document::parse("d\n{\n    a 1;\n    b 2; e 3;\n}\n").unwrap();
    doc.set(&["d", "f"], "4").unwrap();
    assert!(doc.to_string().contains("e 3;\n    f"), "{doc}");
}

#[test]
fn on_one_line_a_comment_is_the_entry_s_before_it() {
    for (src, gone, want) in [
        ("a 1; /* ca */ b 2;\nc 3;\n", "b", "a 1; /* ca */\nc 3;\n"),
        ("d { a 1; /* ca */ b 2; /* cb */ c 3; }\n", "d/b", "d { a 1; /* ca */ c 3; }\n"),
        ("a 1; b 2; /* cb */ c 3;\n", "c", "a 1; b 2; /* cb */\n"),
        ("a 1;\nb 2; /* cb\n */ c 3;\n", "b", "a 1;\nc 3;\n"),
    ] {
        let mut doc = Document::parse(src).unwrap();
        let path: Vec<&str> = gone.split('/').collect();
        doc.remove(&path).unwrap();
        assert_eq!(doc.to_string(), want, "{src:?}");
    }
}

#[test]
fn the_entry_after_a_removed_one_on_its_line_takes_its_place() {
    for (src, gone, want) in [
        ("a 1; b 2; c 3;\n", "a", "b 2; c 3;\n"),
        ("a 1; /* ca */ b 2; c 3;\n", "a", "b 2; c 3;\n"),
        ("d { a 1; b 2; }\n", "d/a", "d { b 2; }\n"),
        ("a 1; // x\n    b 2; c 3;\n", "b", "a 1; // x\n    c 3;\n"),
        ("a 1;\n    b 2; c 3;\n", "b", "a 1;\n    c 3;\n"),
        ("a 1; /* ca */ b 2; c 3;\n", "b", "a 1; /* ca */ c 3;\n"),
        // CRLF: no second `\r`.
        ("a 1; // x\r\n    b 2; c 3;\r\n", "b", "a 1; // x\r\n    c 3;\r\n"),
        ("a 1; // x\r\n    b 2;\r\n    c 3;\r\n", "b", "a 1; // x\r\n    c 3;\r\n"),
    ] {
        let mut doc = Document::parse(src).unwrap();
        let path: Vec<&str> = gone.split('/').collect();
        doc.remove(&path).unwrap();
        assert_eq!(doc.to_string(), want, "{src:?}");
    }
}

#[test]
fn a_single_space_before_the_values_gives_one_space() {
    let mut doc = Document::parse("d\n{\n    name a;\n    rho 1;\n}\n").unwrap();
    doc.set(&["d", "mu"], "2").unwrap();
    assert!(doc.to_string().contains("    rho 1;\n    mu 2;\n}"), "{doc}");
}
