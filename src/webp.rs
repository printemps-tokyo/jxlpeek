//! WebP header summary for `--compare`: the RIFF chunks of RFC 9649. Only
//! the fixed-size headers are read (the VP8 frame tag and key-frame start
//! code, the VP8L signature word, VP8X, ANIM and ANMF); no pixel data.

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Webp {
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
    pub animated: bool,
    /// `lossy` (VP8), `lossless` (VP8L), or `mixed` for an animation that
    /// uses both.
    pub coding: &'static str,
    pub frames: u64,
    pub duration_ms: u64,
    /// ANIM loop count; 0 means forever (RFC 9649 section 2.7.1.1).
    pub loop_count: Option<u16>,
    pub icc_bytes: u64,
    pub exif_bytes: u64,
    pub xmp_bytes: u64,
    pub issues: Vec<String>,
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 2)
        .map(|s| u32::from(u16::from_le_bytes([s[0], s[1]])))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 3)
        .map(|s| u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn sniff(data: &[u8]) -> bool {
    data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP"
}

/// RIFF chunks: four-character code and payload (padding to even sizes is
/// skipped). Stops with an issue on a size past the end.
fn chunks<'a>(data: &'a [u8], issues: &mut Vec<String>) -> Vec<([u8; 4], &'a [u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&data[at..at + 4]);
        let size = le32(data, at + 4).unwrap_or(0) as usize;
        let start = at + 8;
        if size > data.len() - start {
            issues.push(format!(
                "chunk '{}' at offset {at} declares {size} bytes, {} remain",
                String::from_utf8_lossy(&kind),
                data.len() - start
            ));
            break;
        }
        out.push((kind, &data[start..start + size]));
        at = start + size + (size & 1);
    }
    if at < data.len() && at + 8 > data.len() {
        issues.push(format!(
            "{} stray bytes after the last chunk",
            data.len() - at
        ));
    }
    out
}

/// (width, height, lossless, alpha) of one VP8 or VP8L bitstream chunk.
fn bitstream(kind: &[u8; 4], body: &[u8]) -> Option<(u32, u32, bool, bool)> {
    match kind {
        b"VP8 " => {
            // Frame tag: bit 0 clear on a key frame; then the start code.
            let tag = *body.first()?;
            if tag & 1 != 0 || body.get(3..6)? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            Some((
                le16(body, 6)? & 0x3fff,
                le16(body, 8)? & 0x3fff,
                false,
                false,
            ))
        }
        b"VP8L" => {
            if *body.first()? != 0x2f {
                return None;
            }
            let bits = le32(body, 1)?;
            if bits >> 29 != 0 {
                return None; // version_number must be 0 (RFC 9649 section 3.4)
            }
            Some((
                (bits & 0x3fff) + 1,
                ((bits >> 14) & 0x3fff) + 1,
                true,
                (bits >> 28) & 1 != 0,
            ))
        }
        _ => None,
    }
}

pub fn parse(data: &[u8]) -> Webp {
    let mut w = Webp::default();
    let mut issues = Vec::new();
    let riff_size = le32(data, 4).unwrap_or(0) as usize;
    let end = (8 + riff_size).min(data.len());
    if 8 + riff_size > data.len() {
        issues.push(format!(
            "RIFF size {riff_size} runs {} bytes past the end of the file",
            8 + riff_size - data.len()
        ));
    }
    let list = chunks(data.get(12..end).unwrap_or(&[]), &mut issues);
    let mut lossy = false;
    let mut lossless = false;
    let mut have_vp8x = false;
    for (kind, body) in &list {
        match kind {
            b"VP8X" => {
                have_vp8x = true;
                let flags = body.first().copied().unwrap_or(0);
                w.alpha |= flags & 0x10 != 0;
                w.animated = flags & 0x02 != 0;
                w.width = le24(body, 4).map_or(0, |v| v + 1);
                w.height = le24(body, 7).map_or(0, |v| v + 1);
            }
            b"ANIM" => w.loop_count = le16(body, 4).map(|v| v as u16),
            b"ANMF" => {
                w.frames += 1;
                w.duration_ms += u64::from(le24(body, 12).unwrap_or(0));
                for (k, b) in chunks(body.get(16..).unwrap_or(&[]), &mut issues) {
                    if &k == b"ALPH" {
                        w.alpha = true;
                    }
                    if &k != b"VP8 " && &k != b"VP8L" {
                        continue;
                    }
                    match bitstream(&k, b) {
                        Some((_, _, ll, a)) => {
                            lossless |= ll;
                            lossy |= !ll;
                            w.alpha |= a;
                        }
                        None => issues.push(format!(
                            "frame {}: '{}' chunk without a valid header",
                            w.frames,
                            String::from_utf8_lossy(&k)
                        )),
                    }
                }
            }
            b"ALPH" => w.alpha = true,
            b"VP8 " | b"VP8L" => match bitstream(kind, body) {
                Some((bw, bh, ll, a)) => {
                    if !have_vp8x {
                        w.width = bw;
                        w.height = bh;
                    }
                    lossless |= ll;
                    lossy |= !ll;
                    w.alpha |= a;
                    w.frames = w.frames.max(1);
                }
                None => issues.push(format!(
                    "'{}' chunk without a valid header",
                    String::from_utf8_lossy(kind)
                )),
            },
            b"ICCP" => w.icc_bytes += body.len() as u64,
            b"EXIF" => w.exif_bytes += body.len() as u64,
            b"XMP " => w.xmp_bytes += body.len() as u64,
            _ => {}
        }
    }
    w.coding = match (lossy, lossless) {
        (true, true) => "mixed",
        (false, true) => "lossless",
        (true, false) => "lossy",
        (false, false) => "unknown",
    };
    w.issues = issues;
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = kind.to_vec();
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        v.extend_from_slice(b"WEBP");
        v.extend(body);
        v
    }

    fn vp8l(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let bits = (w - 1) | (h - 1) << 14 | u32::from(alpha) << 28;
        let mut b = vec![0x2f];
        b.extend_from_slice(&bits.to_le_bytes());
        chunk(b"VP8L", &b)
    }

    #[test]
    fn simple_lossless() {
        let w = parse(&riff(&[vp8l(300, 200, true)]));
        assert!(w.issues.is_empty(), "{:?}", w.issues);
        assert_eq!((w.width, w.height), (300, 200));
        assert!(w.alpha);
        assert_eq!(w.coding, "lossless");
        assert_eq!(w.frames, 1);
    }

    #[test]
    fn simple_lossy_key_frame() {
        let mut b = vec![0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a];
        b.extend_from_slice(&(64u16 | 0x4000).to_le_bytes()); // scale bits ignored
        b.extend_from_slice(&48u16.to_le_bytes());
        let w = parse(&riff(&[chunk(b"VP8 ", &b)]));
        assert_eq!((w.width, w.height), (64, 48));
        assert_eq!(w.coding, "lossy");
    }

    #[test]
    fn extended_animation_with_metadata() {
        let mut vp8x = vec![0x02 | 0x20 | 0x04, 0, 0, 0];
        vp8x.extend_from_slice(&[99, 0, 0, 49, 0, 0]); // 100x50
        let mut anim = vec![0u8; 4];
        anim.extend_from_slice(&2u16.to_le_bytes());
        let frame = |ms: u32| {
            let mut b = vec![0u8; 6];
            b.extend_from_slice(&[99, 0, 0, 49, 0, 0]);
            b.extend_from_slice(&ms.to_le_bytes()[..3]);
            b.push(0);
            b.extend(vp8l(100, 50, false));
            chunk(b"ANMF", &b)
        };
        let file = riff(&[
            chunk(b"VP8X", &vp8x),
            chunk(b"ICCP", &[1, 2, 3]),
            chunk(b"ANIM", &anim),
            frame(40),
            frame(60),
            chunk(b"XMP ", b"<x/>"),
        ]);
        assert!(sniff(&file));
        let w = parse(&file);
        assert!(w.issues.is_empty(), "{:?}", w.issues);
        assert!(w.animated);
        assert_eq!((w.width, w.height), (100, 50));
        assert_eq!((w.frames, w.duration_ms), (2, 100));
        assert_eq!(w.loop_count, Some(2));
        assert_eq!((w.icc_bytes, w.xmp_bytes), (3, 4));
    }

    #[test]
    fn oversized_chunk_is_an_issue() {
        let mut file = riff(&[chunk(b"VP8L", &[0x2f, 0, 0, 0, 0])]);
        file[16] = 0xff; // chunk size low byte
        let w = parse(&file);
        assert!(!w.issues.is_empty());
    }
}
