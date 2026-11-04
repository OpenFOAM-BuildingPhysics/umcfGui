//! PNG encoding of an [`Image`]: 8-bit RGBA, each row filtered with `Sub`, zlib-compressed.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::raster::Image;

/// CRC-32 (ISO 3309, as PNG uses it) of `parts` one after the other.
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c: u32 = 0xffff_ffff;
    for b in parts.iter().flat_map(|p| p.iter()) {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// The PNG file of `image`.
pub fn encode(image: &Image) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let row = w * 4;
    let mut raw = Vec::with_capacity((row + 1) * h);
    for y in 0..h {
        let line = image.rgba.get(y * row..(y + 1) * row).unwrap_or_default();
        raw.push(1); // Sub: each byte minus the one a pixel to its left.
        for (i, b) in line.iter().enumerate() {
            let left = i.checked_sub(4).and_then(|j| line.get(j)).copied().unwrap_or(0);
            raw.push(b.wrapping_sub(left));
        }
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::new(6));
    // Writing into a Vec doesn't fail.
    let data = z.write_all(&raw).and_then(|()| z.finish()).unwrap_or_default();

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&image.width.to_be_bytes());
    ihdr.extend_from_slice(&image.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA, deflate, standard filters, no interlace.
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &data);
    chunk(&mut out, b"IEND", &[]);
    out
}
