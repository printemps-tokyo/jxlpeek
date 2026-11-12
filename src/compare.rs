//! `--compare`: one column per file, JPEG XL next to AVIF and WebP, built
//! from each format's headers only.

use crate::avif::{self, Colr};
use crate::inspect::{self, Report};
use crate::report::{bytes, printable};
use crate::webp;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub path: String,
    pub format: &'static str,
    pub bytes: u64,
    /// Displayed size (after orientation or rotation, when known).
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bit_depth: Option<u32>,
    pub alpha: Option<bool>,
    pub coding: String,
    /// Displayed frames; 1 for a still image, `None` when unknown.
    pub frames: Option<u64>,
    pub duration_ms: Option<f64>,
    /// `forever`, a format-specific count, or `None` for a still image.
    pub loops: Option<String>,
    pub color: String,
    pub icc_bytes: Option<u64>,
    pub exif_bytes: u64,
    pub xmp_bytes: u64,
    pub notes: Vec<String>,
}

impl Summary {
    /// Bits per displayed pixel, per frame: file bits / (w * h * frames).
    pub fn bits_per_pixel(&self) -> Option<f64> {
        // In floating point: width * height * frames can exceed u64.
        let pixels = f64::from(self.width?)
            * f64::from(self.height?)
            * self.frames.unwrap_or(1).max(1) as f64;
        (pixels > 0.0).then(|| self.bytes as f64 * 8.0 / pixels)
    }
}

#[derive(Debug)]
pub enum CompareError {
    Io(std::io::Error),
    Unknown,
    Jxl(inspect::InspectError),
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompareError::Io(e) => write!(f, "{e}"),
            CompareError::Unknown => write!(f, "not a JPEG XL, AVIF or WebP file"),
            CompareError::Jxl(e) => write!(f, "{e}"),
        }
    }
}

pub fn summarize_path(path: &Path) -> Result<Summary, CompareError> {
    let data = std::fs::read(path).map_err(CompareError::Io)?;
    summarize_bytes(&path.display().to_string(), &data)
}

pub fn summarize_bytes(path: &str, data: &[u8]) -> Result<Summary, CompareError> {
    let jxl = (data.len() >= 2 && data[..2] == crate::codestream::SIGNATURE)
        || (data.len() >= 12 && data[..12] == crate::container::SIGNATURE);
    if jxl {
        let rep = inspect::inspect_bytes(path, data).map_err(CompareError::Jxl)?;
        return Ok(from_jxl(&rep));
    }
    if avif::sniff(data) {
        return Ok(from_avif(path, data.len() as u64, &avif::parse(data)));
    }
    if webp::sniff(data) {
        return Ok(from_webp(path, data.len() as u64, &webp::parse(data)));
    }
    Err(CompareError::Unknown)
}

fn from_jxl(rep: &Report) -> Summary {
    let img = &rep.codestream.image;
    let list = &rep.codestream.frames;
    let (w, h) = img.displayed_size();
    let mut notes = Vec::new();
    let frames = if list.frames.is_empty() {
        notes.push(format!(
            "frames not read: {}",
            list.note.as_deref().unwrap_or("no frame header")
        ));
        if img.animation.is_none() {
            Some(1)
        } else {
            None
        }
    } else {
        if !list.complete {
            notes.push(format!(
                "frame list incomplete: {}",
                list.note.as_deref().unwrap_or("stopped early")
            ));
        }
        Some(list.displayed() as u64)
    };
    let duration_ms = img.animation.as_ref().and_then(|a| {
        (!list.frames.is_empty()).then(|| {
            crate::frames::ticks_to_ms(list.total_ticks(), a.tps_numerator, a.tps_denominator)
        })
    });
    let loops = img.animation.as_ref().map(|a| {
        if a.num_loops == 0 {
            "forever".to_string()
        } else {
            format!("num_loops {}", a.num_loops)
        }
    });
    let (exif, xmp) = rep
        .container
        .as_ref()
        .map_or((0, 0), |c| (c.exif_len, c.xmp_len));
    if rep.container.as_ref().is_some_and(|c| c.jbrd_len.is_some()) {
        notes.push("carries JPEG reconstruction data (jbrd)".to_string());
    }
    let depth = img.bit_depth.bits_per_sample;
    Summary {
        path: rep.path.clone(),
        format: "JPEG XL",
        bytes: rep.file_len,
        width: Some(w),
        height: Some(h),
        bit_depth: Some(depth),
        alpha: Some(img.alpha().is_some()),
        coding: rep.coding().describe().to_string(),
        frames,
        duration_ms,
        loops,
        color: img.color.describe(),
        // The ICC profile is entropy-coded; its size is not in a header.
        icc_bytes: None,
        exif_bytes: exif,
        xmp_bytes: xmp,
        notes,
    }
}

fn from_avif(path: &str, len: u64, a: &avif::Avif) -> Summary {
    let mut notes: Vec<String> = a.issues.clone();
    // An image sequence is described by its color track's sample entry, as
    // libavif decodes it; the still item is only a fallback.
    let seq = a.sequence.as_ref();
    let av1 = seq.and_then(|s| s.av1).or(a.av1);
    let colr = seq
        .and_then(|s| s.color.clone())
        .or_else(|| a.color.clone());
    let bit_depth = seq.and_then(|s| s.bit_depth).or(a.bit_depth);
    let coding = match &av1 {
        Some(c) => format!(
            "AV1 {} profile, level {}{}, {}",
            c.profile_name(),
            c.level(),
            if c.tier == 1 { " high tier" } else { "" },
            c.chroma()
        ),
        None => "AV1 (no av1C property found)".to_string(),
    };
    if a.grid {
        notes.push("primary item is a grid of tiles".to_string());
    }
    if a.rotation != 0 {
        notes.push(format!("rotated {} degrees counterclockwise", a.rotation));
    }
    if a.mirrored {
        notes.push("mirrored".to_string());
    }
    let (mut w, mut h) = match seq {
        Some(s) if s.width.is_some_and(|v| v > 0) && s.height.is_some_and(|v| v > 0) => {
            (s.width, s.height)
        }
        _ => (a.width, a.height),
    };
    if seq.is_none() && (a.rotation == 90 || a.rotation == 270) {
        std::mem::swap(&mut w, &mut h);
    }
    let mut frames = Some(1);
    let mut duration_ms = None;
    let mut loops = None;
    let mut alpha = a.alpha;
    if let Some(seq) = &a.sequence {
        frames = Some(seq.frames);
        duration_ms = seq.duration_ms;
        loops = Some(match seq.repetitions {
            None => "not signaled (no edit list)".to_string(),
            Some(None) => "forever".to_string(),
            Some(Some(n)) => format!("{n} repetitions"),
        });
        alpha = seq.alpha_track;
    }
    let (color, icc) = match &colr {
        Some(Colr::Nclx {
            primaries,
            transfer,
            matrix,
            full_range,
        }) => (
            format!(
                "{}, {} transfer, {} matrix, {} range",
                avif::primaries_name(*primaries),
                avif::transfer_name(*transfer),
                avif::matrix_name(*matrix),
                if *full_range { "full" } else { "limited" }
            ),
            None,
        ),
        Some(Colr::Icc(n)) => ("embedded ICC profile".to_string(), Some(*n)),
        None => (
            "no colr property (the AV1 sequence header, not read here, carries the color)"
                .to_string(),
            None,
        ),
    };
    Summary {
        path: path.to_string(),
        format: if a.sequence.is_some() {
            "AVIF sequence"
        } else {
            "AVIF"
        },
        bytes: len,
        width: w,
        height: h,
        bit_depth,
        alpha: Some(alpha),
        coding,
        frames,
        duration_ms,
        loops,
        color,
        icc_bytes: icc,
        exif_bytes: a.exif_bytes,
        xmp_bytes: a.xmp_bytes,
        notes,
    }
}

fn from_webp(path: &str, len: u64, w: &webp::Webp) -> Summary {
    let coding = match w.coding {
        "lossy" => "lossy (VP8)".to_string(),
        "lossless" => "lossless (VP8L)".to_string(),
        "mixed" => "mixed lossy (VP8) and lossless (VP8L) frames".to_string(),
        other => other.to_string(),
    };
    Summary {
        path: path.to_string(),
        format: if w.animated { "WebP animation" } else { "WebP" },
        bytes: len,
        width: (w.width > 0).then_some(w.width),
        height: (w.height > 0).then_some(w.height),
        bit_depth: Some(8),
        alpha: Some(w.alpha),
        coding,
        frames: Some(w.frames.max(u64::from(!w.animated))),
        duration_ms: w.animated.then_some(w.duration_ms as f64),
        loops: w.loop_count.filter(|_| w.animated).map(|n| {
            if n == 0 {
                "forever".to_string()
            } else {
                format!("loop count {n}")
            }
        }),
        color: if w.icc_bytes > 0 {
            "embedded ICC profile".to_string()
        } else {
            "sRGB (no ICC profile; WebP has no other color signaling)".to_string()
        },
        icc_bytes: (w.icc_bytes > 0).then_some(w.icc_bytes),
        exif_bytes: w.exif_bytes,
        xmp_bytes: w.xmp_bytes,
        notes: w.issues.clone(),
    }
}

fn file_name(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .map_or(path.to_string(), |n| n.to_string_lossy().into_owned());
    printable(&name)
}

fn metadata(s: &Summary) -> String {
    let mut parts = Vec::new();
    if let Some(n) = s.icc_bytes {
        parts.push(format!("ICC {}", bytes(n)));
    }
    if s.exif_bytes > 0 {
        parts.push(format!("Exif {}", bytes(s.exif_bytes)));
    }
    if s.xmp_bytes > 0 {
        parts.push(format!("XMP {}", bytes(s.xmp_bytes)));
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(", ")
    }
}

/// One row per property, one column per file.
pub fn render(list: &[Summary]) -> String {
    let smallest = list.iter().map(|s| s.bytes).min().unwrap_or(0).max(1);
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("file", list.iter().map(|s| file_name(&s.path)).collect()),
        (
            "format",
            list.iter().map(|s| s.format.to_string()).collect(),
        ),
        ("size", list.iter().map(|s| bytes(s.bytes)).collect()),
        (
            "vs smallest",
            list.iter()
                .map(|s| format!("{:.2}x", s.bytes as f64 / smallest as f64))
                .collect(),
        ),
        (
            "dimensions",
            list.iter()
                .map(|s| match (s.width, s.height) {
                    (Some(w), Some(h)) => format!("{w}x{h}"),
                    _ => "?".to_string(),
                })
                .collect(),
        ),
        (
            "bit depth",
            list.iter()
                .map(|s| s.bit_depth.map_or("?".to_string(), |d| d.to_string()))
                .collect(),
        ),
        (
            "alpha",
            list.iter()
                .map(|s| match s.alpha {
                    Some(true) => "yes".to_string(),
                    Some(false) => "no".to_string(),
                    None => "?".to_string(),
                })
                .collect(),
        ),
        ("coding", list.iter().map(|s| s.coding.clone()).collect()),
        (
            "frames",
            list.iter()
                .map(|s| s.frames.map_or("?".to_string(), |n| n.to_string()))
                .collect(),
        ),
        (
            "duration",
            list.iter()
                .map(|s| {
                    s.duration_ms
                        .map_or("-".to_string(), |ms| format!("{:.3} s", ms / 1000.0))
                })
                .collect(),
        ),
        (
            "loops",
            list.iter()
                .map(|s| s.loops.clone().unwrap_or_else(|| "-".to_string()))
                .collect(),
        ),
        ("color", list.iter().map(|s| s.color.clone()).collect()),
        ("metadata", list.iter().map(metadata).collect()),
        (
            "bits/pixel",
            list.iter()
                .map(|s| {
                    s.bits_per_pixel()
                        .map_or("?".to_string(), |b| format!("{b:.3}"))
                })
                .collect(),
        ),
    ];
    let label_w = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..list.len())
        .map(|i| {
            rows.iter()
                .map(|(_, cells)| cells[i].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for (label, cells) in &rows {
        let _ = write!(out, "{label:<label_w$}");
        for (i, cell) in cells.iter().enumerate() {
            let pad = widths[i].saturating_sub(cell.chars().count());
            if i + 1 == cells.len() {
                let _ = write!(out, "  {cell}");
            } else {
                let _ = write!(out, "  {cell}{}", " ".repeat(pad));
            }
        }
        out.push('\n');
    }
    let notes: Vec<String> = list
        .iter()
        .flat_map(|s| {
            s.notes
                .iter()
                .map(move |n| format!("  {}: {}", file_name(&s.path), printable(n)))
        })
        .collect();
    if !notes.is_empty() {
        out.push_str("notes\n");
        for n in notes {
            out.push_str(&n);
            out.push('\n');
        }
    }
    out.push_str(
        "bits/pixel is file bits over displayed pixels times frames; headers only, nothing decoded\n",
    );
    out
}

pub fn summary_json(s: &Summary) -> String {
    use crate::json::quote;
    let opt = |v: Option<u32>| v.map_or("null".to_string(), |n| n.to_string());
    format!(
        "{{\"path\":{},\"format\":{},\"bytes\":{},\"width\":{},\"height\":{},\"bit_depth\":{},\"alpha\":{},\"coding\":{},\"frames\":{},\"duration_ms\":{},\"loops\":{},\"color\":{},\"icc_bytes\":{},\"exif_bytes\":{},\"xmp_bytes\":{},\"bits_per_pixel\":{},\"notes\":[{}]}}",
        quote(&s.path),
        quote(s.format),
        s.bytes,
        opt(s.width),
        opt(s.height),
        opt(s.bit_depth),
        s.alpha.map_or("null".to_string(), |a| a.to_string()),
        quote(&s.coding),
        s.frames.map_or("null".to_string(), |n| n.to_string()),
        s.duration_ms
            .map_or("null".to_string(), |ms| format!("{}", (ms * 1000.0).round() / 1000.0)),
        s.loops.as_deref().map_or("null".to_string(), quote),
        quote(&s.color),
        s.icc_bytes.map_or("null".to_string(), |n| n.to_string()),
        s.exif_bytes,
        s.xmp_bytes,
        s.bits_per_pixel()
            .map_or("null".to_string(), |b| format!("{}", (b * 10000.0).round() / 10000.0)),
        s.notes.iter().map(|n| quote(n)).collect::<Vec<_>>().join(",")
    )
}
