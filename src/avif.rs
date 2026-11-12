//! AVIF header summary for `--compare`: the HEIF item boxes (ISO/IEC
//! 23008-12) and, for image sequences, the ISOBMFF track boxes (ISO/IEC
//! 14496-12). Nothing is decoded; the AV1 bitstream is never opened.
//!
//! Box layouts and the loop-count rule follow libavif `src/read.c`
//! (commit 90d3b66): `iloc`, `ipma`, `av1C`, `colr`, `tkhd`, `mdhd`, `elst`,
//! `stsz`, `stts`, `tref`, and the 78-byte VisualSampleEntry. For an image
//! sequence the color and alpha tracks are chosen the way libavif chooses
//! them, and the track's own sample-entry properties describe it.

use std::collections::{BTreeMap, BTreeSet};

/// What the AVIF headers say.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Avif {
    pub major_brand: String,
    pub brands: Vec<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bit_depth: Option<u32>,
    pub alpha: bool,
    /// From `av1C`: profile, level index, tier, chroma subsampling.
    pub av1: Option<Av1Config>,
    pub color: Option<Colr>,
    pub rotation: u32,
    pub mirrored: bool,
    pub exif_bytes: u64,
    pub xmp_bytes: u64,
    /// The primary item is a `grid` of tiles.
    pub grid: bool,
    /// The image-sequence track, when the file has one.
    pub sequence: Option<Sequence>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1Config {
    pub profile: u8,
    pub level_idx: u8,
    pub tier: u8,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub monochrome: bool,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
}

impl Av1Config {
    pub fn depth(&self) -> u32 {
        if self.twelve_bit {
            12
        } else if self.high_bitdepth {
            10
        } else {
            8
        }
    }

    pub fn chroma(&self) -> &'static str {
        if self.monochrome {
            "4:0:0"
        } else {
            match (self.subsampling_x, self.subsampling_y) {
                (true, true) => "4:2:0",
                (true, false) => "4:2:2",
                (false, false) => "4:4:4",
                (false, true) => "invalid subsampling",
            }
        }
    }

    pub fn profile_name(&self) -> &'static str {
        match self.profile {
            0 => "Main",
            1 => "High",
            2 => "Professional",
            _ => "reserved",
        }
    }

    /// AV1 levels: seq_level_idx = (major - 2) * 4 + minor; 31 means no
    /// level constraint (AV1 spec, Annex A).
    pub fn level(&self) -> String {
        if self.level_idx == 31 {
            "unconstrained".to_string()
        } else {
            format!("{}.{}", 2 + self.level_idx / 4, self.level_idx % 4)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Colr {
    Nclx {
        primaries: u16,
        transfer: u16,
        matrix: u16,
        full_range: bool,
    },
    Icc(u64),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sequence {
    pub track_id: u32,
    pub frames: u64,
    /// Total media duration in milliseconds (sum of `stts` deltas).
    pub duration_ms: Option<f64>,
    /// libavif repetition count: `None` unknown (no edit list), `Some(None)`
    /// forever, `Some(Some(n))` n repetitions after the first play.
    pub repetitions: Option<Option<u64>>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// From the color track's sample entry.
    pub av1: Option<Av1Config>,
    pub color: Option<Colr>,
    pub bit_depth: Option<u32>,
    /// An alpha track references this color track (`tref/auxl`).
    pub alpha_track: bool,
}

/// A parsed box: type and payload (after the header).
struct Bx<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 2)
        .map(|s| u32::from(u16::from_be_bytes([s[0], s[1]])))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8).map(|s| {
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        u64::from_be_bytes(a)
    })
}

/// Big-endian unsigned of 0, 4 or 8 bytes (iloc field sizes).
fn be_n(b: &[u8], at: usize, n: usize) -> Option<u64> {
    match n {
        0 => Some(0),
        4 => be32(b, at).map(u64::from),
        8 => be64(b, at),
        _ => None,
    }
}

fn fourcc(k: &[u8]) -> String {
    k.iter()
        .map(|&c| {
            if c.is_ascii_graphic() || c == b' ' {
                c as char
            } else {
                '?'
            }
        })
        .collect()
}

/// Split a byte range into boxes. A size past the end is an issue and stops
/// the walk.
fn boxes<'a>(data: &'a [u8], issues: &mut Vec<String>) -> Vec<Bx<'a>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let Some(size32) = be32(data, at) else {
            issues.push(format!("truncated box header at offset {at}"));
            break;
        };
        let Some(kind) = data.get(at + 4..at + 8) else {
            issues.push(format!("truncated box header at offset {at}"));
            break;
        };
        let (header, size) = match size32 {
            0 => (8usize, (data.len() - at) as u64),
            1 => match be64(data, at + 8) {
                Some(s) => (16, s),
                None => {
                    issues.push(format!("truncated large box size at offset {at}"));
                    break;
                }
            },
            s => (8, u64::from(s)),
        };
        let remaining = (data.len() - at) as u64;
        if size < header as u64 || size > remaining {
            issues.push(format!(
                "box '{}' at offset {at} declares {size} bytes, {remaining} remain",
                fourcc(kind)
            ));
            break;
        }
        let mut k = [0u8; 4];
        k.copy_from_slice(kind);
        out.push(Bx {
            kind: k,
            body: &data[at + header..at + size as usize],
        });
        at += size as usize;
    }
    out
}

fn find<'a, 'b>(list: &'b [Bx<'a>], kind: &[u8; 4]) -> Option<&'b Bx<'a>> {
    list.iter().find(|b| &b.kind == kind)
}

/// FullBox version byte and the payload after version + flags.
fn full(body: &[u8]) -> Option<(u8, u32, &[u8])> {
    let vf = be32(body, 0)?;
    Some(((vf >> 24) as u8, vf & 0x00ff_ffff, &body[4..]))
}

fn cstr(b: &[u8]) -> (&[u8], &[u8]) {
    match b.iter().position(|&c| c == 0) {
        Some(i) => (&b[..i], &b[i + 1..]),
        None => (b, &[]),
    }
}

#[derive(Default)]
struct Item {
    kind: [u8; 4],
    content_type: Vec<u8>,
    bytes: u64,
    props: Vec<usize>,
}

type Items = BTreeMap<u32, Item>;

fn item(items: &mut Items, id: u32) -> &mut Item {
    items.entry(id).or_default()
}

const ALPHA_URNS: [&[u8]; 2] = [
    b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha",
    b"urn:mpeg:hevc:2015:auxid:1",
];

/// Is `data` an AVIF file (an `ftyp` box naming `avif` or `avis`)?
pub fn sniff(data: &[u8]) -> bool {
    if data.get(4..8) != Some(b"ftyp") {
        return false;
    }
    let Some(size) = be32(data, 0) else {
        return false;
    };
    let end = (size as usize).min(data.len());
    if end < 16 {
        return false;
    }
    let body = &data[8..end];
    body.chunks_exact(4)
        .enumerate()
        .any(|(i, b)| i != 1 && (b == b"avif" || b == b"avis"))
}

pub fn parse(data: &[u8]) -> Avif {
    let mut a = Avif::default();
    let mut issues = Vec::new();
    let top = boxes(data, &mut issues);
    if let Some(ftyp) = find(&top, b"ftyp") {
        a.major_brand = fourcc(ftyp.body.get(0..4).unwrap_or(&[]));
        a.brands = ftyp
            .body
            .get(8..)
            .unwrap_or(&[])
            .chunks_exact(4)
            .map(fourcc)
            .collect();
    }
    if let Some(meta) = find(&top, b"meta") {
        parse_meta(meta.body, &mut a, &mut issues);
    } else {
        issues.push("no 'meta' box".to_string());
    }
    if let Some(moov) = find(&top, b"moov") {
        a.sequence = parse_moov(moov.body, &mut issues);
    }
    a.issues = issues;
    a
}

fn parse_meta(body: &[u8], a: &mut Avif, issues: &mut Vec<String>) {
    let Some((_, _, rest)) = full(body) else {
        issues.push("truncated 'meta' box".to_string());
        return;
    };
    let children = boxes(rest, issues);
    let primary = find(&children, b"pitm").and_then(|b| {
        let (v, _, p) = full(b.body)?;
        if v == 0 {
            be16(p, 0)
        } else {
            be32(p, 0)
        }
    });
    let mut items = Items::new();

    if let Some(iinf) = find(&children, b"iinf") {
        if let Some((v, _, p)) = full(iinf.body) {
            let skip = if v == 0 { 2 } else { 4 };
            for infe in boxes(p.get(skip..).unwrap_or(&[]), issues) {
                if &infe.kind != b"infe" {
                    continue;
                }
                let Some((iv, _, q)) = full(infe.body) else {
                    continue;
                };
                if iv < 2 {
                    continue; // versions 0 and 1 carry no item_type
                }
                let (id, at) = if iv == 2 {
                    (be16(q, 0), 2)
                } else {
                    (be32(q, 0), 4)
                };
                let Some(id) = id else { continue };
                let Some(kind) = q.get(at + 2..at + 6) else {
                    continue;
                };
                let it = item(&mut items, id);
                it.kind.copy_from_slice(kind);
                let (_, after_name) = cstr(q.get(at + 6..).unwrap_or(&[]));
                if kind == b"mime" {
                    it.content_type = cstr(after_name).0.to_vec();
                }
            }
        }
    }

    if let Some(iloc) = find(&children, b"iloc") {
        if let Err(msg) = parse_iloc(iloc.body, &mut items) {
            issues.push(format!("'iloc': {msg}"));
        }
    }

    let mut props: Vec<Bx> = Vec::new();
    if let Some(iprp) = find(&children, b"iprp") {
        let inner = boxes(iprp.body, issues);
        if let Some(ipco) = find(&inner, b"ipco") {
            props = boxes(ipco.body, issues);
        }
        for ipma in inner.iter().filter(|b| &b.kind == b"ipma") {
            if let Err(msg) = parse_ipma(ipma.body, props.len(), &mut items) {
                issues.push(format!("'ipma': {msg}"));
            }
        }
    }

    // Items that name the primary as the target of an `auxl` reference.
    let mut aux_of_primary: BTreeSet<u32> = BTreeSet::new();
    if let Some(iref) = find(&children, b"iref") {
        if let Some((v, _, p)) = full(iref.body) {
            let w = if v == 0 { 2 } else { 4 };
            for r in boxes(p, issues) {
                if &r.kind != b"auxl" {
                    continue;
                }
                let from = if w == 2 {
                    be16(r.body, 0)
                } else {
                    be32(r.body, 0)
                };
                let count = be16(r.body, w).unwrap_or(0) as usize;
                let to: Vec<u32> = (0..count)
                    .filter_map(|i| {
                        let at = w + 2 + i * w;
                        if w == 2 {
                            be16(r.body, at)
                        } else {
                            be32(r.body, at)
                        }
                    })
                    .collect();
                if let Some(from) = from {
                    if primary.is_some_and(|p| to.contains(&p)) {
                        aux_of_primary.insert(from);
                    }
                }
            }
        }
    }

    let Some(primary) = primary else {
        issues.push("no primary item ('pitm')".to_string());
        return;
    };
    let empty = Item::default();
    let prim = items.get(&primary).unwrap_or(&empty);
    a.grid = &prim.kind == b"grid";
    for &pi in &prim.props {
        let Some(p) = props.get(pi) else { continue };
        match &p.kind {
            b"ispe" => {
                if let Some((_, _, q)) = full(p.body) {
                    a.width = be32(q, 0);
                    a.height = be32(q, 4);
                }
            }
            b"pixi" => {
                if let Some((_, _, q)) = full(p.body) {
                    a.bit_depth = q.get(1).map(|&d| u32::from(d));
                }
            }
            b"av1C" => a.av1 = parse_av1c(p.body),
            b"colr" => {
                // An item may carry one nclx and one ICC colr; keep the ICC.
                let c = parse_colr(p.body);
                if !matches!(a.color, Some(Colr::Icc(_))) {
                    a.color = c.or(a.color.take());
                }
            }
            b"irot" => a.rotation = u32::from(p.body.first().copied().unwrap_or(0) & 3) * 90,
            b"imir" => a.mirrored = true,
            _ => {}
        }
    }
    if a.av1.is_none() {
        // A grid's tiles carry the av1C; they all share one configuration.
        a.av1 = items
            .iter()
            .filter(|(_, it)| &it.kind == b"av01")
            .flat_map(|(_, it)| it.props.iter())
            .filter_map(|&pi| props.get(pi))
            .find(|p| &p.kind == b"av1C")
            .and_then(|p| parse_av1c(p.body));
    }
    if a.bit_depth.is_none() {
        a.bit_depth = a.av1.map(|c| c.depth());
    }
    a.alpha = items.iter().any(|(id, it)| {
        aux_of_primary.contains(id)
            && it.props.iter().filter_map(|&pi| props.get(pi)).any(|p| {
                &p.kind == b"auxC"
                    && full(p.body).is_some_and(|(_, _, q)| ALPHA_URNS.contains(&cstr(q).0))
            })
    });
    for it in items.values() {
        if &it.kind == b"Exif" {
            a.exif_bytes = a.exif_bytes.saturating_add(it.bytes);
        } else if &it.kind == b"mime" && it.content_type == b"application/rdf+xml" {
            a.xmp_bytes = a.xmp_bytes.saturating_add(it.bytes);
        }
    }
}

fn parse_iloc(body: &[u8], items: &mut Items) -> Result<(), String> {
    let (v, _, p) = full(body).ok_or("truncated")?;
    if v > 2 {
        return Err(format!("version {v}"));
    }
    let b0 = *p.first().ok_or("truncated")?;
    let b1 = *p.get(1).ok_or("truncated")?;
    let offset_size = usize::from(b0 >> 4);
    let length_size = usize::from(b0 & 15);
    let base_size = usize::from(b1 >> 4);
    let index_size = if v >= 1 { usize::from(b1 & 15) } else { 0 };
    for n in [offset_size, length_size, base_size, index_size] {
        if n != 0 && n != 4 && n != 8 {
            return Err(format!("field size {n}"));
        }
    }
    let mut at = 2usize;
    let count = if v < 2 {
        let c = be16(p, at).ok_or("truncated")?;
        at += 2;
        c
    } else {
        let c = be32(p, at).ok_or("truncated")?;
        at += 4;
        c
    };
    for _ in 0..count {
        let id = if v < 2 {
            let c = be16(p, at).ok_or("truncated")?;
            at += 2;
            c
        } else {
            let c = be32(p, at).ok_or("truncated")?;
            at += 4;
            c
        };
        if v >= 1 {
            at += 2; // reserved + construction_method
        }
        at += 2; // data_reference_index
        at += base_size;
        let extents = be16(p, at).ok_or("truncated")?;
        at += 2;
        let mut total = 0u64;
        for _ in 0..extents {
            at += index_size;
            at += offset_size;
            let len = be_n(p, at, length_size).ok_or("truncated")?;
            at += length_size;
            total = total.saturating_add(len);
        }
        if at > p.len() {
            return Err("truncated".to_string());
        }
        item(items, id).bytes = total;
    }
    Ok(())
}

fn parse_ipma(body: &[u8], nprops: usize, items: &mut Items) -> Result<(), String> {
    let (v, flags, p) = full(body).ok_or("truncated")?;
    let wide = flags & 1 != 0;
    let count = be32(p, 0).ok_or("truncated")?;
    let mut at = 4usize;
    for _ in 0..count {
        let id = if v < 1 {
            let c = be16(p, at).ok_or("truncated")?;
            at += 2;
            c
        } else {
            let c = be32(p, at).ok_or("truncated")?;
            at += 4;
            c
        };
        let n = *p.get(at).ok_or("truncated")?;
        at += 1;
        for _ in 0..n {
            let index = if wide {
                let c = be16(p, at).ok_or("truncated")? & 0x7fff;
                at += 2;
                c
            } else {
                let c = u32::from(*p.get(at).ok_or("truncated")? & 0x7f);
                at += 1;
                c
            } as usize;
            if index == 0 {
                continue;
            }
            if index > nprops {
                return Err(format!("property index {index} of {nprops}"));
            }
            item(items, id).props.push(index - 1);
        }
    }
    Ok(())
}

fn parse_av1c(body: &[u8]) -> Option<Av1Config> {
    let b = body.get(0..4)?;
    if b[0] != 0x81 {
        return None; // marker 1, version 1
    }
    Some(Av1Config {
        profile: b[1] >> 5,
        level_idx: b[1] & 31,
        tier: b[2] >> 7,
        high_bitdepth: b[2] & 0x40 != 0,
        twelve_bit: b[2] & 0x20 != 0,
        monochrome: b[2] & 0x10 != 0,
        subsampling_x: b[2] & 0x08 != 0,
        subsampling_y: b[2] & 0x04 != 0,
    })
}

fn parse_colr(body: &[u8]) -> Option<Colr> {
    let kind = body.get(0..4)?;
    match kind {
        b"nclx" => Some(Colr::Nclx {
            primaries: be16(body, 4)? as u16,
            transfer: be16(body, 6)? as u16,
            matrix: be16(body, 8)? as u16,
            full_range: body.get(10)? & 0x80 != 0,
        }),
        b"rICC" | b"prof" => Some(Colr::Icc((body.len() - 4) as u64)),
        _ => None,
    }
}

/// What one `trak` box says.
#[derive(Default)]
struct Track {
    id: u32,
    /// `Some(None)` is an indefinite duration.
    duration: Option<Option<u64>>,
    width: Option<u32>,
    height: Option<u32>,
    timescale: Option<u32>,
    auxl: Vec<u32>,
    /// The first sample entry is an AV1 one (`av01`).
    av1_entry: bool,
    av1: Option<Av1Config>,
    color: Option<Colr>,
    depth: Option<u32>,
    /// `auxi` aux type, when the sample entry carries one.
    auxi: Option<Vec<u8>>,
    frames: u64,
    media_ticks: Option<u64>,
    /// See [`Sequence::repetitions`].
    repetitions: Option<Option<u64>>,
}

fn parse_trak(body: &[u8], issues: &mut Vec<String>) -> Track {
    let mut t = Track::default();
    let children = boxes(body, issues);
    if let Some(tkhd) = find(&children, b"tkhd") {
        if let Some((v, _, p)) = full(tkhd.body) {
            let (id, dur, dim_at) = if v == 1 {
                (
                    be32(p, 16),
                    be64(p, 24).map(|d| (d != u64::MAX).then_some(d)),
                    32 + 52,
                )
            } else {
                (
                    be32(p, 8),
                    be32(p, 16).map(|d| (d != u32::MAX).then_some(u64::from(d))),
                    20 + 52,
                )
            };
            t.id = id.unwrap_or(0);
            t.duration = dur;
            t.width = be32(p, dim_at).map(|w| w >> 16);
            t.height = be32(p, dim_at + 4).map(|h| h >> 16);
        }
    }
    if let Some(tref) = find(&children, b"tref") {
        for r in boxes(tref.body, issues) {
            if &r.kind == b"auxl" {
                t.auxl.extend(
                    r.body
                        .chunks_exact(4)
                        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                        .filter(|&id| id != 0),
                );
            }
        }
    }
    let mdia = find(&children, b"mdia").map(|b| boxes(b.body, issues));
    if let Some(mdia) = mdia {
        if let Some(mdhd) = find(&mdia, b"mdhd") {
            if let Some((v, _, p)) = full(mdhd.body) {
                t.timescale = if v == 1 { be32(p, 16) } else { be32(p, 8) };
            }
        }
        let minf = find(&mdia, b"minf").map(|b| boxes(b.body, issues));
        let stbl = minf
            .as_ref()
            .and_then(|m| find(m, b"stbl"))
            .map(|b| boxes(b.body, issues));
        if let Some(stbl) = stbl {
            if let Some(stsd) = find(&stbl, b"stsd") {
                if let Some((_, _, p)) = full(stsd.body) {
                    // entry_count, then sample entries as boxes.
                    let entries = boxes(p.get(4..).unwrap_or(&[]), issues);
                    if let Some(entry) = entries.first() {
                        t.av1_entry = &entry.kind == b"av01";
                        if t.av1_entry && entry.body.len() >= 78 {
                            for prop in boxes(&entry.body[78..], issues) {
                                match &prop.kind {
                                    b"av1C" => t.av1 = parse_av1c(prop.body),
                                    b"colr" => {
                                        let c = parse_colr(prop.body);
                                        if !matches!(t.color, Some(Colr::Icc(_))) {
                                            t.color = c.or(t.color.take());
                                        }
                                    }
                                    b"pixi" => {
                                        t.depth = full(prop.body)
                                            .and_then(|(_, _, q)| q.get(1))
                                            .map(|&d| u32::from(d));
                                    }
                                    b"auxi" => {
                                        t.auxi =
                                            full(prop.body).map(|(_, _, q)| cstr(q).0.to_vec());
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            if let Some(stsz) = find(&stbl, b"stsz") {
                if let Some((_, _, p)) = full(stsz.body) {
                    t.frames = u64::from(be32(p, 4).unwrap_or(0));
                }
            }
            if let Some(stts) = find(&stbl, b"stts") {
                if let Some((_, _, p)) = full(stts.body) {
                    let n = be32(p, 0).unwrap_or(0) as usize;
                    let mut ticks = 0u64;
                    for i in 0..n {
                        let (Some(c), Some(d)) = (be32(p, 4 + i * 8), be32(p, 8 + i * 8)) else {
                            issues.push("truncated 'stts' box".to_string());
                            break;
                        };
                        ticks = ticks.saturating_add(u64::from(c) * u64::from(d));
                    }
                    t.media_ticks = Some(ticks);
                }
            }
        }
    }
    // Repetition count (libavif avifParseTrackBox): no edit list is unknown,
    // a non-repeating list plays once, a repeating one repeats until the
    // track duration, and an indefinite or out-of-range count is forever.
    if let Some(edts) = find(&children, b"edts") {
        let e = boxes(edts.body, issues);
        if let Some(elst) = find(&e, b"elst") {
            if let Some((v, flags, p)) = full(elst.body) {
                if flags & 1 == 0 {
                    t.repetitions = Some(Some(0));
                } else {
                    let segment = if v == 1 {
                        be64(p, 4)
                    } else {
                        be32(p, 4).map(u64::from)
                    };
                    t.repetitions = match (t.duration, segment) {
                        (Some(None), _) => Some(None),
                        (Some(Some(td)), Some(seg)) if seg > 0 && td > 0 => {
                            let n = td.div_ceil(seg) - 1;
                            Some((n <= i32::MAX as u64).then_some(n))
                        }
                        _ => None,
                    };
                }
            }
        }
    }
    t
}

/// The image sequence, with the color track and alpha track chosen as
/// libavif `avifDecoderReset` chooses them: the first AV1 track with an ID
/// and no `auxl` reference is the color track; an AV1 track whose `auxl`
/// points at it and whose `auxi` (if any) names the alpha URN is its alpha.
fn parse_moov(body: &[u8], issues: &mut Vec<String>) -> Option<Sequence> {
    let children = boxes(body, issues);
    let tracks: Vec<Track> = children
        .iter()
        .filter(|b| &b.kind == b"trak")
        .map(|b| parse_trak(b.body, issues))
        .collect();
    let color = tracks
        .iter()
        .find(|t| t.id != 0 && t.av1_entry && t.auxl.is_empty())?;
    let alpha_track = tracks.iter().any(|t| {
        t.id != 0
            && t.av1_entry
            && t.auxl.contains(&color.id)
            && t.auxi
                .as_deref()
                .is_none_or(|urn| ALPHA_URNS.contains(&urn))
    });
    Some(Sequence {
        track_id: color.id,
        frames: color.frames,
        duration_ms: match (color.media_ticks, color.timescale) {
            (Some(ticks), Some(ts)) if ts > 0 => Some(ticks as f64 * 1000.0 / f64::from(ts)),
            _ => None,
        },
        repetitions: color.repetitions,
        width: color.width,
        height: color.height,
        av1: color.av1,
        color: color.color.clone(),
        bit_depth: color.depth.or(color.av1.map(|c| c.depth())),
        alpha_track,
    })
}

/// ITU-T H.273 code points, the ones AVIF files commonly use.
pub fn primaries_name(code: u16) -> String {
    match code {
        1 => "BT.709/sRGB".to_string(),
        2 => "unspecified".to_string(),
        9 => "BT.2020".to_string(),
        12 => "Display P3".to_string(),
        other => format!("primaries {other}"),
    }
}

pub fn transfer_name(code: u16) -> String {
    match code {
        1 => "BT.709".to_string(),
        2 => "unspecified".to_string(),
        8 => "linear".to_string(),
        13 => "sRGB".to_string(),
        16 => "PQ".to_string(),
        18 => "HLG".to_string(),
        other => format!("transfer {other}"),
    }
}

pub fn matrix_name(code: u16) -> String {
    match code {
        0 => "identity (RGB)".to_string(),
        1 => "BT.709".to_string(),
        2 => "unspecified".to_string(),
        5 | 6 => "BT.601".to_string(),
        9 => "BT.2020 NCL".to_string(),
        other => format!("matrix {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn fullbox(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut b = (u32::from(version) << 24 | flags).to_be_bytes().to_vec();
        b.extend_from_slice(body);
        bx(kind, &b)
    }

    #[test]
    fn av1c_fields() {
        // Profile 1, level idx 8 (4.0), main tier, 10-bit, 4:4:4.
        let c = parse_av1c(&[0x81, 0x28, 0x40, 0x00]).unwrap();
        assert_eq!(c.profile_name(), "High");
        assert_eq!(c.level(), "4.0");
        assert_eq!(c.depth(), 10);
        assert_eq!(c.chroma(), "4:4:4");
        let c = parse_av1c(&[0x81, 0x1f, 0x0c, 0x00]).unwrap();
        assert_eq!(c.level(), "unconstrained");
        assert_eq!(c.chroma(), "4:2:0");
        assert!(parse_av1c(&[0x01, 0, 0, 0]).is_none());
    }

    #[test]
    fn items_properties_alpha_and_metadata() {
        let ftyp = bx(b"ftyp", b"avif\0\0\0\0mif1avif");
        let pitm = fullbox(b"pitm", 0, 0, &1u16.to_be_bytes());
        let infe = |id: u16, kind: &[u8; 4], extra: &[u8]| {
            let mut b = id.to_be_bytes().to_vec();
            b.extend_from_slice(&[0, 0]);
            b.extend_from_slice(kind);
            b.push(0); // empty name
            b.extend_from_slice(extra);
            fullbox(b"infe", 2, 0, &b)
        };
        let mut iinf_body = 4u16.to_be_bytes().to_vec();
        iinf_body.extend(infe(1, b"av01", &[]));
        iinf_body.extend(infe(2, b"av01", &[]));
        iinf_body.extend(infe(3, b"Exif", &[]));
        iinf_body.extend(infe(4, b"mime", b"application/rdf+xml\0"));
        let iinf = fullbox(b"iinf", 0, 0, &iinf_body);
        // iloc v1: offset 4, length 4, base 0, index 0; four items.
        let mut iloc_body = vec![0x44, 0x00];
        iloc_body.extend_from_slice(&4u16.to_be_bytes());
        for (id, len) in [(1u16, 1000u32), (2, 300), (3, 50), (4, 70)] {
            iloc_body.extend_from_slice(&id.to_be_bytes());
            iloc_body.extend_from_slice(&[0, 0]); // construction method 0
            iloc_body.extend_from_slice(&[0, 0]); // data reference
            iloc_body.extend_from_slice(&1u16.to_be_bytes()); // one extent
            iloc_body.extend_from_slice(&0u32.to_be_bytes());
            iloc_body.extend_from_slice(&len.to_be_bytes());
        }
        let iloc = fullbox(b"iloc", 1, 0, &iloc_body);
        let mut ispe = 640u32.to_be_bytes().to_vec();
        ispe.extend_from_slice(&480u32.to_be_bytes());
        let mut ipco = fullbox(b"ispe", 0, 0, &ispe);
        ipco.extend(bx(b"av1C", &[0x81, 0x00, 0x0c, 0x00]));
        ipco.extend(fullbox(b"pixi", 0, 0, &[3, 8, 8, 8]));
        ipco.extend(fullbox(
            b"auxC",
            0,
            0,
            b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0",
        ));
        ipco.extend(bx(b"irot", &[1]));
        // ipma with 15-bit indices (flags 1).
        let mut ipma_body = 2u32.to_be_bytes().to_vec();
        ipma_body.extend_from_slice(&1u16.to_be_bytes());
        ipma_body.push(4);
        for i in [1u16, 2, 3, 5] {
            ipma_body.extend_from_slice(&(0x8000 | i).to_be_bytes());
        }
        ipma_body.extend_from_slice(&2u16.to_be_bytes());
        ipma_body.push(2);
        for i in [2u16, 4] {
            ipma_body.extend_from_slice(&i.to_be_bytes());
        }
        let mut iprp_body = bx(b"ipco", &ipco);
        iprp_body.extend(fullbox(b"ipma", 0, 1, &ipma_body));
        let iprp = bx(b"iprp", &iprp_body);
        let mut auxl = 2u16.to_be_bytes().to_vec();
        auxl.extend_from_slice(&1u16.to_be_bytes());
        auxl.extend_from_slice(&1u16.to_be_bytes());
        let iref = fullbox(b"iref", 0, 0, &bx(b"auxl", &auxl));
        let mut meta_body = Vec::new();
        for b in [pitm, iinf, iloc, iref, iprp] {
            meta_body.extend(b);
        }
        let mut file = ftyp;
        file.extend(fullbox(b"meta", 0, 0, &meta_body));

        assert!(sniff(&file));
        let a = parse(&file);
        assert!(a.issues.is_empty(), "{:?}", a.issues);
        assert_eq!((a.width, a.height), (Some(640), Some(480)));
        assert_eq!(a.bit_depth, Some(8));
        assert_eq!(a.av1.unwrap().chroma(), "4:2:0");
        assert!(a.alpha);
        assert_eq!(a.rotation, 90);
        assert_eq!((a.exif_bytes, a.xmp_bytes), (50, 70));
        assert!(a.sequence.is_none());
    }

    #[test]
    fn repetitions_follow_libavif() {
        // tkhd duration 300, elst segment 100, repeating: 2 repetitions.
        let mut tkhd = vec![0u8; 8];
        tkhd.extend_from_slice(&1u32.to_be_bytes()); // track_ID
        tkhd.extend_from_slice(&[0u8; 4]);
        tkhd.extend_from_slice(&300u32.to_be_bytes());
        tkhd.extend(vec![0u8; 52]);
        tkhd.extend_from_slice(&(48u32 << 16).to_be_bytes());
        tkhd.extend_from_slice(&(36u32 << 16).to_be_bytes());
        let mut elst = 1u32.to_be_bytes().to_vec();
        elst.extend_from_slice(&100u32.to_be_bytes());
        elst.extend(vec![0u8; 8]);
        let mut mdhd = vec![0u8; 8];
        mdhd.extend_from_slice(&10u32.to_be_bytes());
        mdhd.extend_from_slice(&3u32.to_be_bytes());
        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(b"pict");
        let mut stsz = 0u32.to_be_bytes().to_vec();
        stsz.extend_from_slice(&3u32.to_be_bytes());
        let mut stts = 1u32.to_be_bytes().to_vec();
        stts.extend_from_slice(&3u32.to_be_bytes());
        stts.extend_from_slice(&1u32.to_be_bytes());
        // stsd with one av01 VisualSampleEntry carrying av1C (10-bit 4:2:0).
        let mut entry = vec![0u8; 78];
        entry.extend(bx(b"av1C", &[0x81, 0x00, 0x4c, 0x00]));
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend(bx(b"av01", &entry));
        let mut stbl = fullbox(b"stsd", 0, 0, &stsd);
        stbl.extend(fullbox(b"stsz", 0, 0, &stsz));
        stbl.extend(fullbox(b"stts", 0, 0, &stts));
        let minf = bx(b"stbl", &stbl);
        let mut mdia = fullbox(b"mdhd", 0, 0, &mdhd);
        mdia.extend(fullbox(b"hdlr", 0, 0, &hdlr));
        mdia.extend(bx(b"minf", &minf));
        let mut trak = fullbox(b"tkhd", 0, 0, &tkhd);
        trak.extend(bx(b"edts", &fullbox(b"elst", 0, 1, &elst)));
        trak.extend(bx(b"mdia", &mdia));
        let mut moov = bx(b"trak", &trak);
        let mut issues = Vec::new();
        let s = parse_moov(&moov, &mut issues).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(s.track_id, 1);
        assert_eq!(s.frames, 3);
        assert_eq!(s.duration_ms, Some(300.0));
        assert_eq!(s.repetitions, Some(Some(2)));
        assert_eq!((s.width, s.height), (Some(48), Some(36)));
        assert_eq!(s.bit_depth, Some(10));
        assert_eq!(s.av1.unwrap().chroma(), "4:2:0");
        assert!(!s.alpha_track);

        // An auxiliary track that is not alpha does not count; an alpha one
        // (auxi with the alpha URN) that references track 1 does.
        let aux_trak = |id: u32, urn: &[u8]| {
            let mut th = tkhd.clone();
            th[8..12].copy_from_slice(&id.to_be_bytes());
            let mut e = vec![0u8; 78];
            let mut auxi = urn.to_vec();
            auxi.push(0);
            e.extend(fullbox(b"auxi", 0, 0, &auxi));
            let mut sd = 1u32.to_be_bytes().to_vec();
            sd.extend(bx(b"av01", &e));
            let mut st = fullbox(b"stsd", 0, 0, &sd);
            st.extend(fullbox(b"stsz", 0, 0, &stsz));
            let mut md = fullbox(b"mdhd", 0, 0, &mdhd);
            md.extend(fullbox(b"hdlr", 0, 0, &hdlr));
            md.extend(bx(b"minf", &bx(b"stbl", &st)));
            let mut tk = fullbox(b"tkhd", 0, 0, &th);
            tk.extend(bx(b"tref", &bx(b"auxl", &1u32.to_be_bytes())));
            tk.extend(bx(b"mdia", &md));
            bx(b"trak", &tk)
        };
        let mut with_depth = moov.clone();
        with_depth.extend(aux_trak(2, b"urn:example:depth"));
        assert!(!parse_moov(&with_depth, &mut issues).unwrap().alpha_track);
        moov.extend(aux_trak(3, b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha"));
        assert!(parse_moov(&moov, &mut issues).unwrap().alpha_track);
    }

    #[test]
    fn bad_sizes_are_issues_not_panics() {
        let mut issues = Vec::new();
        let data = [0, 0, 0, 200, b'f', b't', b'y', b'p', 0, 0];
        assert!(boxes(&data, &mut issues).is_empty());
        assert_eq!(issues.len(), 1);
        let mut items = Items::new();
        assert!(parse_iloc(&[0, 0, 0, 0, 0x33, 0x00], &mut items).is_err());
    }

    #[test]
    fn huge_extent_lengths_saturate() {
        // Two Exif items of u64::MAX bytes each must not overflow.
        let mut items = Items::new();
        let mut body = vec![1, 0, 0, 0, 0x08, 0x00];
        body.extend_from_slice(&2u16.to_be_bytes());
        for id in [1u16, 2] {
            body.extend_from_slice(&id.to_be_bytes());
            body.extend_from_slice(&[0, 0, 0, 0]);
            body.extend_from_slice(&1u16.to_be_bytes());
            body.extend_from_slice(&u64::MAX.to_be_bytes());
        }
        parse_iloc(&body, &mut items).unwrap();
        assert_eq!(items[&1].bytes, u64::MAX);
    }
}
