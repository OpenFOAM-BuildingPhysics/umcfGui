//! Reads one field file and reports how long decompressing and parsing took:
//! `cargo run --release -p umcf-foam-field --example read_field -- <file>`.
// A native benchmark: reading the clock is fine here (the lint guards wasm builds).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)]

use std::time::Instant;

use umcf_foam_field::{Field, Limits, gunzip};

fn main() {
    let path = std::env::args().nth(1).expect("usage: read_field <file>");
    let bytes = std::fs::read(&path).unwrap();
    let t = Instant::now();
    let raw =
        if bytes.starts_with(&[0x1f, 0x8b]) { gunzip(&bytes, Limits::default().max_bytes).unwrap() } else { bytes };
    let t_gz = t.elapsed();
    let t = Instant::now();
    let f = Field::parse(&raw).unwrap();
    let t_parse = t.elapsed();
    let n: usize = f.lists().iter().map(|l| l.len()).sum();
    let mb = raw.len() as f64 / 1e6;
    println!(
        "{path}: {:.0} MB text, {n} elements; gunzip {:.2}s ({:.0} MB/s), parse {:.2}s ({:.0} MB/s)",
        mb,
        t_gz.as_secs_f64(),
        mb / t_gz.as_secs_f64(),
        t_parse.as_secs_f64(),
        mb / t_parse.as_secs_f64()
    );
}
