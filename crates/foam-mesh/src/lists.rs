//! Readers for the list files of a polyMesh: `labelList`, `vectorField`, `faceList` and
//! `faceCompactList`, ASCII and binary. Labels are stored as `u32`: the mesh indices of a
//! 50-million-face mesh take half the memory they would as `f64`.

use umcf_foam_field::{Arch, Format, Header};

use crate::{Error, Limits};

pub(crate) struct Reader<'a> {
    file: &'a str,
    b: &'a [u8],
    i: usize,
    binary: bool,
    arch: Arch,
}

/// A vector with room for `n`, or an error when the memory isn't there (instead of aborting).
fn alloc<T>(n: usize) -> Result<Vec<T>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::TooLarge { what: "list (out of memory)", limit: n })?;
    Ok(v)
}

/// Room for `more` elements beyond the current length, or an error instead of an abort.
fn grow<T>(v: &mut Vec<T>, more: usize) -> Result<(), Error> {
    v.try_reserve(more)
        .map_err(|_| Error::TooLarge { what: "list (out of memory)", limit: v.len().saturating_add(more) })
}

fn is_ws(b: u8) -> bool {
    b.is_ascii_whitespace()
}

impl<'a> Reader<'a> {
    pub fn new(file: &'a str, bytes: &'a [u8], start: usize, header: &Header) -> Self {
        Reader { file, b: bytes, i: start, binary: header.format == Format::Binary, arch: header.arch }
    }

    fn line(&self) -> usize {
        self.b.get(..self.i).unwrap_or(self.b).iter().filter(|c| **c == b'\n').count() + 1
    }

    fn bad(&self, msg: impl Into<String>) -> Error {
        Error::Bad { file: self.file.to_string(), line: self.line(), msg: msg.into() }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Whitespace and comments.
    pub fn skip_trivia(&mut self) {
        loop {
            match (self.peek(), self.b.get(self.i + 1).copied()) {
                (Some(c), _) if is_ws(c) => self.i += 1,
                (Some(b'/'), Some(b'/')) => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.i += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    self.i += 2;
                    while self.peek().is_some() && !(self.peek() == Some(b'*') && self.b.get(self.i + 1) == Some(&b'/'))
                    {
                        self.i += 1;
                    }
                    self.i = (self.i + 2).min(self.b.len());
                }
                _ => return,
            }
        }
    }

    /// Whitespace, the hot path inside lists: OpenFOAM writes no comments there, so the full
    /// comment-aware skip runs only when a `/` shows up.
    fn skip_ws(&mut self) {
        while let Some(c) = self.b.get(self.i) {
            if is_ws(*c) {
                self.i += 1;
            } else {
                if *c == b'/' {
                    self.skip_trivia();
                }
                return;
            }
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), Error> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.bad(format!("expected `{}`", c as char)))
        }
    }

    /// An ASCII non-negative integer.
    fn integer(&mut self) -> Result<u64, Error> {
        self.skip_ws();
        let start = self.i;
        let mut v: u64 = 0;
        while let Some(c) = self.peek().filter(u8::is_ascii_digit) {
            v = v
                .checked_mul(10)
                .and_then(|v| v.checked_add(u64::from(c - b'0')))
                .ok_or_else(|| self.bad("number too large"))?;
            self.i += 1;
        }
        if self.i == start {
            let found = String::from_utf8_lossy(self.b.get(self.i..self.i + 12).unwrap_or(&[])).to_string();
            return Err(self.bad(format!(
                "expected a non-negative integer, found `{}`",
                found.split_whitespace().next().unwrap_or("")
            )));
        }
        Ok(v)
    }

    fn label(&mut self) -> Result<u32, Error> {
        let v = self.integer()?;
        u32::try_from(v).map_err(|_| self.bad(format!("label {v} does not fit in 32 bits")))
    }

    fn count(&mut self, limits: &Limits) -> Result<usize, Error> {
        let n = self.integer()?;
        let n = usize::try_from(n).map_err(|_| Error::TooLarge { what: "list", limit: limits.max_elements })?;
        if n > limits.max_elements {
            return Err(Error::TooLarge { what: "list", limit: limits.max_elements });
        }
        Ok(n)
    }

    fn real(&mut self) -> Result<f64, Error> {
        self.skip_ws();
        let rest = self.b.get(self.i..).unwrap_or(&[]);
        match fast_float2::parse_partial::<f64, _>(rest) {
            Ok((v, n)) if n > 0 && rest.get(n).is_none_or(|c| is_ws(*c) || matches!(c, b'(' | b')')) => {
                self.i += n;
                Ok(v)
            }
            _ => Err(self.bad("expected a number")),
        }
    }

    /// Never reserve more than the input could hold: an ASCII element takes at least two bytes.
    fn room(&self, n: usize, per: usize) -> usize {
        n.saturating_mul(per).min(self.b.len().saturating_sub(self.i) / 2 + 1)
    }

    fn binary_block(&mut self, n: usize, width: usize) -> Result<&'a [u8], Error> {
        let size = n.checked_mul(width).ok_or(Error::Truncated { file: self.file.to_string() })?;
        let data =
            self.b.get(self.i..).and_then(|r| r.get(..size)).ok_or(Error::Truncated { file: self.file.to_string() })?;
        self.i += size;
        Ok(data)
    }

    fn binary_label(&self, c: &[u8]) -> Result<u32, Error> {
        let v: u64 = match c.len() {
            8 => {
                let a: [u8; 8] = c.try_into().map_err(|_| self.bad("label width"))?;
                if self.arch.big_endian { u64::from_be_bytes(a) } else { u64::from_le_bytes(a) }
            }
            _ => {
                let a: [u8; 4] = c.try_into().map_err(|_| self.bad("label width"))?;
                u64::from(if self.arch.big_endian { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) })
            }
        };
        u32::try_from(v).map_err(|_| self.bad(format!("label {v} does not fit in 32 bits")))
    }

    fn binary_real(&self, c: &[u8]) -> Result<f64, Error> {
        Ok(match c.len() {
            8 => {
                let a: [u8; 8] = c.try_into().map_err(|_| self.bad("scalar width"))?;
                f64::from_bits(if self.arch.big_endian { u64::from_be_bytes(a) } else { u64::from_le_bytes(a) })
            }
            _ => {
                let a: [u8; 4] = c.try_into().map_err(|_| self.bad("scalar width"))?;
                f64::from(f32::from_bits(if self.arch.big_endian {
                    u32::from_be_bytes(a)
                } else {
                    u32::from_le_bytes(a)
                }))
            }
        })
    }

    /// OpenFOAM writes an empty binary list as its size alone, with no parentheses (they come
    /// with the data, `UListIO.C`): `neighbour` of a processor without internal faces is `0`.
    fn empty_binary(&mut self, n: usize) -> bool {
        self.skip_trivia();
        self.binary && n == 0 && self.peek() != Some(b'(')
    }

    /// `N (l l l)`, `N{l}`, binary `N(raw)`, or binary `0` alone.
    pub fn labels(&mut self, limits: &Limits) -> Result<Vec<u32>, Error> {
        let n = self.count(limits)?;
        self.skip_trivia();
        if self.peek() == Some(b'{') {
            if n > limits.max_uniform {
                return Err(Error::TooLarge { what: "uniform list", limit: limits.max_uniform });
            }
            self.i += 1;
            let v = self.label()?;
            self.expect(b'}')?;
            let mut out = alloc(n)?;
            out.resize(n, v);
            return Ok(out);
        }
        if self.empty_binary(n) {
            return Ok(Vec::new());
        }
        self.expect(b'(')?;
        let out = if self.binary && n > 0 {
            let width = self.arch.label_bytes;
            let data = self.binary_block(n, width)?;
            let mut out = alloc(n)?;
            for c in data.chunks_exact(width) {
                out.push(self.binary_label(c)?);
            }
            out
        } else {
            let mut out = alloc(self.room(n, 1))?;
            for _ in 0..n {
                out.push(self.label()?);
            }
            out
        };
        self.expect(b')')?;
        Ok(out)
    }

    /// [`Self::labels`], keeping only the elements from `skip` on: the list's length and those
    /// elements. Binary elements before `skip` aren't decoded; ASCII ones are parsed (each still
    /// a label) but not kept.
    pub fn labels_from(&mut self, limits: &Limits, skip: usize) -> Result<(usize, Vec<u32>), Error> {
        let n = self.count(limits)?;
        let keep = n.saturating_sub(skip);
        self.skip_trivia();
        if self.peek() == Some(b'{') {
            if n > limits.max_uniform {
                return Err(Error::TooLarge { what: "uniform list", limit: limits.max_uniform });
            }
            self.i += 1;
            let v = self.label()?;
            self.expect(b'}')?;
            let mut out = alloc(keep)?;
            out.resize(keep, v);
            return Ok((n, out));
        }
        if self.empty_binary(n) {
            return Ok((0, Vec::new()));
        }
        self.expect(b'(')?;
        let out = if self.binary && n > 0 {
            let width = self.arch.label_bytes;
            let data = self.binary_block(n, width)?;
            let mut out = alloc(keep)?;
            for c in data.chunks_exact(width).skip(skip) {
                out.push(self.binary_label(c)?);
            }
            out
        } else {
            let mut out = alloc(self.room(keep, 1))?;
            for k in 0..n {
                let l = self.label()?;
                if k >= skip {
                    out.push(l);
                }
            }
            out
        };
        self.expect(b')')?;
        Ok((n, out))
    }

    /// `N ((x y z) …)` or, binary, `N(raw)`.
    pub fn vectors(&mut self, limits: &Limits) -> Result<Vec<[f64; 3]>, Error> {
        let n = self.count(limits)?;
        if self.empty_binary(n) {
            return Ok(Vec::new());
        }
        self.expect(b'(')?;
        let out = if self.binary && n > 0 {
            let width = self.arch.scalar_bytes;
            let data =
                self.binary_block(n.checked_mul(3).ok_or(Error::Truncated { file: self.file.to_string() })?, width)?;
            let mut out = alloc(n)?;
            for c in data.chunks_exact(width * 3) {
                let (x, rest) = c.split_at(width);
                let (y, z) = rest.split_at(width);
                out.push([self.binary_real(x)?, self.binary_real(y)?, self.binary_real(z)?]);
            }
            out
        } else {
            let mut out = alloc(self.room(n, 1))?;
            for _ in 0..n {
                self.expect(b'(')?;
                let p = [self.real()?, self.real()?, self.real()?];
                self.expect(b')')?;
                out.push(p);
            }
            out
        };
        self.expect(b')')?;
        Ok(out)
    }

    /// ASCII `faceList`: `N (k(a b c …) …)`. Returns compact offsets (N + 1) and point labels.
    pub fn face_list(&mut self, limits: &Limits) -> Result<(Vec<u32>, Vec<u32>), Error> {
        if self.binary {
            return Err(Error::Unsupported(format!(
                "{}: binary faceList (OpenFOAM writes binary faces as faceCompactList)",
                self.file
            )));
        }
        let n = self.count(limits)?;
        self.expect(b'(')?;
        let mut offsets = alloc(self.room(n, 1).saturating_add(1))?;
        let mut points = alloc(self.room(n, 4))?;
        offsets.push(0u32);
        for _ in 0..n {
            let k = self.count(limits)?;
            self.expect(b'(')?;
            // The reservation above assumes quads; polygon faces grow it, fallibly. A face can't
            // hold more points than its text has room for.
            grow(&mut points, k.min(self.room(k, 1)))?;
            for _ in 0..k {
                points.push(self.label()?);
            }
            self.expect(b')')?;
            let end = u32::try_from(points.len())
                .map_err(|_| Error::TooLarge { what: "face points", limit: u32::MAX as usize })?;
            grow(&mut offsets, 1)?;
            offsets.push(end);
        }
        self.expect(b')')?;
        Ok((offsets, points))
    }

    /// ASCII `faceList` from face `skip` on: the number of faces, and compact offsets and point
    /// labels of faces `skip..`. The faces before are passed over to their `)`, not read.
    pub fn face_list_from(&mut self, limits: &Limits, skip: usize) -> Result<(usize, Vec<u32>, Vec<u32>), Error> {
        if self.binary {
            return Err(Error::Unsupported(format!(
                "{}: binary faceList (OpenFOAM writes binary faces as faceCompactList)",
                self.file
            )));
        }
        let n = self.count(limits)?;
        self.expect(b'(')?;
        let keep = n.saturating_sub(skip);
        let mut offsets = alloc(self.room(keep, 1).saturating_add(1))?;
        let mut points = alloc(self.room(keep, 4))?;
        offsets.push(0u32);
        for i in 0..n {
            let k = self.count(limits)?;
            self.expect(b'(')?;
            if i < skip {
                let rest = self.b.get(self.i..).unwrap_or_default();
                let end =
                    rest.iter().position(|c| *c == b')').ok_or(Error::Truncated { file: self.file.to_string() })?;
                self.i += end + 1;
                continue;
            }
            grow(&mut points, k.min(self.room(k, 1)))?;
            for _ in 0..k {
                points.push(self.label()?);
            }
            self.expect(b')')?;
            let end = u32::try_from(points.len())
                .map_err(|_| Error::TooLarge { what: "face points", limit: u32::MAX as usize })?;
            grow(&mut offsets, 1)?;
            offsets.push(end);
        }
        self.expect(b')')?;
        Ok((n, offsets, points))
    }

    /// `faceCompactList` from face `skip` on, as [`Self::face_list_from`]: all the offsets are
    /// read (they say where face `skip` starts), the point labels only from there.
    pub fn face_compact_list_from(
        &mut self,
        limits: &Limits,
        skip: usize,
    ) -> Result<(usize, Vec<u32>, Vec<u32>), Error> {
        let offsets = self.labels(limits)?;
        let n = offsets.len().saturating_sub(1);
        let first = offsets.get(skip.min(n)).copied().unwrap_or(0);
        let (total, points) = self.labels_from(limits, first as usize)?;
        let well_formed = offsets.first() == Some(&0)
            && offsets.windows(2).all(|w| w[0] <= w[1])
            && offsets.last().is_some_and(|l| *l as usize == total);
        if !well_formed {
            return Err(self.bad("faceCompactList offsets don't match the point list"));
        }
        let tail = offsets.get(skip.min(n)..).unwrap_or_default().iter().map(|o| o - first).collect();
        Ok((n, tail, points))
    }

    /// `faceCompactList`: the offsets list (N + 1 labels) then the point labels.
    pub fn face_compact_list(&mut self, limits: &Limits) -> Result<(Vec<u32>, Vec<u32>), Error> {
        let offsets = self.labels(limits)?;
        let points = self.labels(limits)?;
        let well_formed = offsets.first() == Some(&0)
            && offsets.windows(2).all(|w| w[0] <= w[1])
            && offsets.last().is_some_and(|l| *l as usize == points.len());
        if !well_formed {
            return Err(self.bad("faceCompactList offsets don't match the point list"));
        }
        Ok((offsets, points))
    }
}
