//! ISOBMFF container walker (ISO/IEC 18181-2).
//!
//! A JPEG XL container is a sequence of boxes: 4-byte big-endian size,
//! 4-byte type, contents. Size 1 means an 8-byte large size follows; size 0
//! means the box runs to end of file. The first box is the 12-byte signature
//! box, the second must be `ftyp` with major brand `jxl ` (libjxl
//! `decode.cc` enforces both). The codestream sits in one `jxlc` box or in
//! a sequence of `jxlp` boxes (4-byte index, high bit marks the last part).

/// Container signature box, byte for byte.
pub const SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxInfo {
    /// Four-character box type, non-printables escaped.
    pub kind: String,
    pub offset: u64,
    pub header_len: u64,
    pub content_len: u64,
    /// Declared with size 0 (runs to end of file).
    pub to_eof: bool,
    /// Declared size ran past the end of the file.
    pub truncated: bool,
    /// For `brob`: the type of the Brotli-compressed box inside.
    pub inner_kind: Option<String>,
    /// For `jxlp`: (index, is_last).
    pub jxlp: Option<(u32, bool)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Container {
    pub boxes: Vec<BoxInfo>,
    /// The codestream reassembled from `jxlc` or the `jxlp` parts.
    pub codestream: Vec<u8>,
    /// Number of `jxlp` parts (0 when a single `jxlc` box holds it).
    pub codestream_parts: usize,
    /// `jxll` box contents (5 or 10).
    pub level: Option<u8>,
    pub ftyp_minor_version: Option<u32>,
    pub compatible_brands: Vec<String>,
    pub jbrd_len: Option<u64>,
    /// Bytes of Exif boxes (compressed size when wrapped in `brob`).
    pub exif_len: u64,
    pub xmp_len: u64,
    pub jumb_len: u64,
    /// Bytes of boxes this tool does not know.
    pub other_len: u64,
    pub brotli_boxes: usize,
    /// Structural problems, in file order. Any entry means a decoder may
    /// refuse the file.
    pub issues: Vec<String>,
}

impl Container {
    pub fn metadata_len(&self) -> u64 {
        self.exif_len + self.xmp_len + self.jumb_len
    }

    pub fn box_kinds(&self) -> Vec<String> {
        self.boxes
            .iter()
            .map(|b| match &b.inner_kind {
                Some(inner) => format!("brob({})", inner.trim_end()),
                None => b.kind.trim_end().to_string(),
            })
            .collect()
    }
}

fn kind_string(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        if (0x20..0x7f).contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("\\x{b:02x}"));
        }
    }
    s
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Walk every box in `data`, which must start with the signature box.
pub fn parse(data: &[u8]) -> Container {
    let mut c = Container::default();
    let len = data.len() as u64;
    if data.len() < 12 || data[..12] != SIGNATURE {
        c.issues
            .push("file does not start with the JPEG XL signature box".to_string());
        return c;
    }
    let mut pos: u64 = 0;
    let mut parts: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut have_jxlc = false;
    let mut saw_last_part = false;
    let mut index = 0usize;

    while pos < len {
        if len - pos < 8 {
            c.issues.push(format!(
                "{} trailing bytes after the last box (not a box header)",
                len - pos
            ));
            break;
        }
        let p = pos as usize;
        let size32 = be32(&data[p..p + 4]);
        let kind_bytes = &data[p + 4..p + 8];
        let kind = kind_string(kind_bytes);
        let (header_len, mut content_len, to_eof) = match size32 {
            0 => (8u64, len - pos - 8, true),
            1 => {
                if len - pos < 16 {
                    c.issues
                        .push(format!("box '{kind}' at {pos}: large size field cut off"));
                    break;
                }
                let large = be64(&data[p + 8..p + 16]);
                if large < 16 {
                    c.issues
                        .push(format!("box '{kind}' at {pos}: invalid large size {large}"));
                    break;
                }
                (16u64, large - 16, false)
            }
            s if s < 8 => {
                c.issues
                    .push(format!("box '{kind}' at {pos}: invalid size {s}"));
                break;
            }
            s => (8u64, u64::from(s) - 8, false),
        };
        let mut truncated = false;
        let fits = pos
            .checked_add(header_len)
            .and_then(|v| v.checked_add(content_len))
            .is_some_and(|end| end <= len);
        if !fits {
            truncated = true;
            c.issues.push(format!(
                "box '{kind}' at {pos} declares {} content bytes but only {} remain",
                content_len,
                len - pos - header_len
            ));
            content_len = len - pos - header_len;
        }
        let start = (pos + header_len) as usize;
        let content = &data[start..start + content_len as usize];

        let mut info = BoxInfo {
            kind: kind.clone(),
            offset: pos,
            header_len,
            content_len,
            to_eof,
            truncated,
            inner_kind: None,
            jxlp: None,
        };

        if index == 0 && (kind_bytes != b"JXL " || header_len + content_len != 12) {
            c.issues
                .push("first box must be the 12-byte signature box".to_string());
        }
        if index == 1 && kind_bytes != b"ftyp" {
            c.issues
                .push(format!("second box must be 'ftyp', found '{kind}'"));
        }

        match kind_bytes {
            b"JXL " => {}
            b"ftyp" => {
                if content.len() < 8 {
                    c.issues.push("ftyp box shorter than 8 bytes".to_string());
                } else {
                    if &content[..4] != b"jxl " {
                        c.issues.push(format!(
                            "ftyp major brand must be 'jxl ', found '{}'",
                            kind_string(&content[..4])
                        ));
                    }
                    let version = be32(&content[4..8]);
                    c.ftyp_minor_version = Some(version);
                    if version > 1 {
                        c.issues.push(format!(
                            "unknown file format version {version} (known: 0, 1)"
                        ));
                    }
                    if !(content.len() - 8).is_multiple_of(4) {
                        c.issues.push(format!(
                            "ftyp compatible brand list is {} bytes, not a multiple of 4",
                            content.len() - 8
                        ));
                    }
                    for chunk in content[8..].chunks(4) {
                        if chunk.len() == 4 {
                            c.compatible_brands.push(kind_string(chunk));
                        }
                    }
                }
            }
            b"jxll" => {
                if content.len() != 1 {
                    c.issues.push(format!(
                        "jxll box must hold exactly 1 byte, found {}",
                        content.len()
                    ));
                } else {
                    c.level = Some(content[0]);
                    if content[0] != 5 && content[0] != 10 {
                        c.issues.push(format!(
                            "jxll declares level {}, only 5 and 10 exist",
                            content[0]
                        ));
                    }
                }
            }
            b"jxlc" => {
                if have_jxlc || !parts.is_empty() {
                    c.issues.push(
                        "only one jxlc box is allowed, and never together with jxlp".to_string(),
                    );
                } else {
                    have_jxlc = true;
                    c.codestream = content.to_vec();
                }
            }
            b"jxlp" => {
                if content.len() < 4 {
                    c.issues
                        .push(format!("jxlp box at {pos} has no index field"));
                } else {
                    let raw = be32(&content[..4]);
                    let idx = raw & 0x7fff_ffff;
                    let last = raw & 0x8000_0000 != 0;
                    info.jxlp = Some((idx, last));
                    if have_jxlc {
                        c.issues.push("jxlp box after a jxlc box".to_string());
                    }
                    if saw_last_part {
                        c.issues
                            .push(format!("jxlp index {idx} follows a part flagged as last"));
                    }
                    if parts.iter().any(|(i, _)| *i == idx) {
                        c.issues.push(format!("duplicate jxlp index {idx}"));
                    } else if idx as usize != parts.len() && c.ftyp_minor_version == Some(0) {
                        c.issues.push(format!(
                            "jxlp index {idx} is out of order (expected {})",
                            parts.len()
                        ));
                    }
                    if last {
                        saw_last_part = true;
                    }
                    parts.push((idx, content[4..].to_vec()));
                }
            }
            b"jbrd" => {
                c.jbrd_len = Some(content_len);
            }
            b"Exif" => {
                c.exif_len += content_len;
                if content.len() < 4 {
                    c.issues
                        .push("Exif box lacks the 4-byte TIFF header offset".to_string());
                }
            }
            b"xml " => {
                c.xmp_len += content_len;
            }
            b"jumb" => {
                c.jumb_len += content_len;
            }
            b"brob" => {
                c.brotli_boxes += 1;
                if content.len() < 4 {
                    c.issues
                        .push("brob box lacks the inner box type".to_string());
                } else {
                    let inner = &content[..4];
                    info.inner_kind = Some(kind_string(inner));
                    match inner {
                        b"Exif" => c.exif_len += content_len,
                        b"xml " => c.xmp_len += content_len,
                        b"jumb" => c.jumb_len += content_len,
                        b"jxlc" | b"jxlp" | b"jbrd" | b"ftyp" | b"jxll" | b"JXL " | b"brob" => {
                            c.issues
                                .push(format!("brob must not wrap a '{}' box", kind_string(inner)));
                        }
                        _ => c.other_len += content_len,
                    }
                }
            }
            b"jxli" => {
                // Frame index box; informational only.
                c.other_len += content_len;
            }
            _ => {
                c.other_len += content_len;
            }
        }

        c.boxes.push(info);
        index += 1;
        pos += header_len + content_len;
        if truncated {
            break;
        }
    }

    if !parts.is_empty() {
        parts.sort_by_key(|(i, _)| *i);
        for (expected, (idx, _)) in parts.iter().enumerate() {
            if *idx != expected as u32 {
                c.issues.push(format!(
                    "jxlp indices are not contiguous from 0 (missing index {expected})"
                ));
                break;
            }
        }
        c.codestream_parts = parts.len();
        c.codestream = parts.into_iter().flat_map(|(_, bytes)| bytes).collect();
        if !saw_last_part {
            c.issues
                .push("no jxlp box is flagged as the last part (high index bit)".to_string());
        }
    } else if !have_jxlc {
        c.issues
            .push("no codestream box (jxlc or jxlp) found".to_string());
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&((content.len() as u32) + 8).to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(content);
        v
    }

    fn ftyp() -> Vec<u8> {
        boxed(b"ftyp", b"jxl \0\0\0\0jxl ")
    }

    #[test]
    fn single_jxlc() {
        let mut f = SIGNATURE.to_vec();
        f.extend(ftyp());
        f.extend(boxed(b"jxll", &[5]));
        f.extend(boxed(b"jxlc", &[0xff, 0x0a, 1, 2, 3]));
        let c = parse(&f);
        assert!(c.issues.is_empty(), "{:?}", c.issues);
        assert_eq!(c.codestream, vec![0xff, 0x0a, 1, 2, 3]);
        assert_eq!(c.level, Some(5));
        assert_eq!(c.codestream_parts, 0);
        assert_eq!(c.box_kinds(), vec!["JXL", "ftyp", "jxll", "jxlc"]);
    }

    #[test]
    fn jxlp_parts_are_joined_and_last_flag_checked() {
        let mut f = SIGNATURE.to_vec();
        f.extend(ftyp());
        f.extend(boxed(b"jxlp", &[0, 0, 0, 0, 0xff, 0x0a]));
        f.extend(boxed(b"Exif", &[0, 0, 0, 0, b'M', b'M', 0, 42]));
        f.extend(boxed(b"jxlp", &[0x80, 0, 0, 1, 9, 9]));
        let c = parse(&f);
        assert!(c.issues.is_empty(), "{:?}", c.issues);
        assert_eq!(c.codestream, vec![0xff, 0x0a, 9, 9]);
        assert_eq!(c.codestream_parts, 2);
        assert_eq!(c.exif_len, 8);

        let mut f = SIGNATURE.to_vec();
        f.extend(ftyp());
        f.extend(boxed(b"jxlp", &[0, 0, 0, 0, 0xff, 0x0a]));
        let c = parse(&f);
        assert!(c.issues.iter().any(|i| i.contains("last part")));
    }

    #[test]
    fn brob_inner_type_and_size_zero_box() {
        let mut f = SIGNATURE.to_vec();
        f.extend(ftyp());
        f.extend(boxed(b"brob", b"xml \x0b\x02\x80"));
        // size 0: jxlc runs to end of file
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(b"jxlc");
        f.extend_from_slice(&[0xff, 0x0a, 7]);
        let c = parse(&f);
        assert!(c.issues.is_empty(), "{:?}", c.issues);
        assert_eq!(c.box_kinds(), vec!["JXL", "ftyp", "brob(xml)", "jxlc"]);
        assert_eq!(c.xmp_len, 7);
        assert_eq!(c.brotli_boxes, 1);
        assert!(c.boxes[3].to_eof);
        assert_eq!(c.codestream, vec![0xff, 0x0a, 7]);
    }

    #[test]
    fn structural_problems_are_reported() {
        let mut f = SIGNATURE.to_vec();
        f.extend(boxed(b"jxlc", &[0xff, 0x0a]));
        let c = parse(&f);
        assert!(c.issues.iter().any(|i| i.contains("ftyp")));

        let mut f = SIGNATURE.to_vec();
        f.extend(ftyp());
        f.extend(boxed(b"jxlc", &[0xff, 0x0a]));
        f.extend(boxed(b"jxlc", &[0xff, 0x0a]));
        let c = parse(&f);
        assert!(c.issues.iter().any(|i| i.contains("only one jxlc")));

        let mut f = SIGNATURE.to_vec();
        f.extend(ftyp());
        let mut big = boxed(b"jxlc", &[0xff, 0x0a]);
        big[3] = 200; // declare more than exists
        f.extend(big);
        let c = parse(&f);
        assert!(c.boxes.last().unwrap().truncated);
        assert!(c.issues.iter().any(|i| i.contains("declares")));
    }

    #[test]
    fn huge_largesize_does_not_overflow() {
        let mut f = SIGNATURE.to_vec();
        f.extend(ftyp());
        f.extend_from_slice(&1u32.to_be_bytes());
        f.extend_from_slice(b"jxlc");
        f.extend_from_slice(&u64::MAX.to_be_bytes());
        f.extend_from_slice(&[0xff, 0x0a]);
        let c = parse(&f);
        assert!(c.boxes.last().unwrap().truncated);
        assert!(c.issues.iter().any(|i| i.contains("declares")));
    }

    #[test]
    fn jxlp_gap_is_an_issue_even_when_out_of_order_is_allowed() {
        let mut f = SIGNATURE.to_vec();
        f.extend(boxed(b"ftyp", b"jxl \0\0\0\x01jxl "));
        f.extend(boxed(b"jxlp", &[0x80, 0, 0, 1, 0xff, 0x0a]));
        let c = parse(&f);
        assert!(
            c.issues.iter().any(|i| i.contains("contiguous")),
            "{:?}",
            c.issues
        );

        let mut f = SIGNATURE.to_vec();
        f.extend(boxed(b"ftyp", b"jxl \0\0\0\0jxl\0x"));
        f.extend(boxed(b"jxlc", &[0xff, 0x0a]));
        let c = parse(&f);
        assert!(
            c.issues.iter().any(|i| i.contains("multiple of 4")),
            "{:?}",
            c.issues
        );
    }

    #[test]
    fn not_a_container() {
        let c = parse(b"\xff\x0a\x00");
        assert!(!c.issues.is_empty());
    }
}
