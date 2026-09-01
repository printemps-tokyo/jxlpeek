//! Delivery checks: what a web page, a CDN or a CI gate wants to know about
//! a JPEG XL file before shipping it.
//!
//! Levels: `Fail` means a decoder may reject the file or a declared budget is
//! exceeded; `Warn` means it will decode but costs more than it should;
//! `Info` is a fact worth knowing (orientation, HDR, animation); `Pass` is
//! the check that found nothing.

use crate::codestream::{ExtraChannelType, ImageInfo};
use crate::inspect::{Coding, Report};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Pass,
    Info,
    Warn,
    Fail,
}

impl Level {
    pub fn label(&self) -> &'static str {
        match self {
            Level::Pass => "PASS",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }
    pub fn id(&self) -> &'static str {
        match self {
            Level::Pass => "pass",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: &'static str,
    pub level: Level,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Fail when the file is larger than this many bytes.
    pub max_bytes: Option<u64>,
    /// Fail when the displayed image has more pixels than this.
    pub max_pixels: Option<u64>,
    /// Warn when Exif/XMP/JUMBF boxes exceed this many bytes.
    pub max_metadata: u64,
    /// Treat warnings as failures.
    pub strict: bool,
}

pub const DEFAULT_MAX_METADATA: u64 = 16 * 1024;

impl Default for Options {
    fn default() -> Self {
        Options {
            max_bytes: None,
            max_pixels: None,
            max_metadata: DEFAULT_MAX_METADATA,
            strict: false,
        }
    }
}

/// Level 5 limits, from libjxl `JxlEncoderSetCodestreamLevel` documentation
/// and `VerifyLevelSettings` in `encode.cc`: at most 262144 pixels per side
/// and 268435456 in total, 16-bit samples, modular buffers that fit 16 bits,
/// 4 extra channels, no CMYK (black channel). ICC size also has a limit
/// (4 MiB) that this tool cannot see without decoding.
pub const LEVEL5_MAX_SIDE: u64 = 262_144;
pub const LEVEL5_MAX_PIXELS: u64 = 268_435_456;
pub const LEVEL5_MAX_BITS: u32 = 16;
pub const LEVEL5_MAX_EXTRA: usize = 4;

/// Why the headers need a level 10 decoder, or `None` when level 5 suffices.
pub fn level10_reasons(img: &ImageInfo) -> Vec<String> {
    let mut reasons = Vec::new();
    let (w, h) = (u64::from(img.width), u64::from(img.height));
    if w > LEVEL5_MAX_SIDE || h > LEVEL5_MAX_SIDE {
        reasons.push(format!("{w}x{h} exceeds {LEVEL5_MAX_SIDE} px per side"));
    }
    if w * h > LEVEL5_MAX_PIXELS {
        reasons.push(format!("{} pixels exceed {LEVEL5_MAX_PIXELS}", w * h));
    }
    if img.bit_depth.bits_per_sample > LEVEL5_MAX_BITS {
        reasons.push(format!(
            "{}-bit samples exceed {LEVEL5_MAX_BITS} bits",
            img.bit_depth.bits_per_sample
        ));
    }
    if !img.modular_16bit_sufficient {
        reasons.push(
            "modular buffers need more than 16 bits (modular_16bit_sufficient = false)".to_string(),
        );
    }
    if img.extra_channels.len() > LEVEL5_MAX_EXTRA {
        reasons.push(format!(
            "{} extra channels exceed {LEVEL5_MAX_EXTRA}",
            img.extra_channels.len()
        ));
    }
    if img
        .extra_channels
        .iter()
        .any(|c| c.kind == ExtraChannelType::Black)
    {
        reasons.push("CMYK (black extra channel)".to_string());
    }
    if img
        .extra_channels
        .iter()
        .any(|c| c.bit_depth.bits_per_sample > LEVEL5_MAX_BITS)
    {
        reasons.push("an extra channel exceeds 16 bits".to_string());
    }
    reasons
}

pub fn run(report: &Report, opts: &Options) -> Vec<Check> {
    let mut out = Vec::new();
    let img = &report.codestream.image;

    // Structure.
    match &report.container {
        None => out.push(Check {
            id: "structure",
            level: Level::Pass,
            message: "bare codestream (no container); metadata boxes cannot be present".into(),
        }),
        Some(c) if c.issues.is_empty() => out.push(Check {
            id: "structure",
            level: Level::Pass,
            message: format!("container well-formed ({} boxes)", c.boxes.len()),
        }),
        Some(c) => {
            for issue in &c.issues {
                out.push(Check {
                    id: "structure",
                    level: Level::Fail,
                    message: issue.clone(),
                });
            }
        }
    }

    // Level.
    let reasons = level10_reasons(img);
    let declared = report.container.as_ref().and_then(|c| c.level);
    match (reasons.is_empty(), declared) {
        (false, Some(10)) => out.push(Check {
            id: "level",
            level: Level::Warn,
            message: format!(
                "needs a level 10 decoder: {}; level 5 is the baseline profile, so check the target decoder or re-encode within level 5 limits (cjxl --codestream_level=5)",
                reasons.join(", ")
            ),
        }),
        (false, _) => out.push(Check {
            id: "level",
            level: Level::Fail,
            message: format!(
                "headers exceed level 5 limits ({}) but the file declares {}",
                reasons.join(", "),
                declared.map_or("no level (5 assumed)".to_string(), |l| format!("level {l}"))
            ),
        }),
        (true, Some(10)) => out.push(Check {
            id: "level",
            level: Level::Info,
            message: "declares level 10 although the headers fit level 5; a level-5-only decoder may refuse it on the declaration alone".into(),
        }),
        (true, _) => out.push(Check {
            id: "level",
            level: Level::Pass,
            message: match declared {
                Some(l) => format!("declares level {l}; headers fit level 5"),
                None => "no jxll box; headers fit level 5".into(),
            },
        }),
    }

    // Metadata weight.
    if let Some(c) = &report.container {
        let meta = c.metadata_len();
        if meta > 0 {
            let share = meta as f64 * 100.0 / report.file_len.max(1) as f64;
            let mut parts = Vec::new();
            if c.exif_len > 0 {
                parts.push(format!("Exif {}", crate::report::bytes(c.exif_len)));
            }
            if c.xmp_len > 0 {
                parts.push(format!("XMP {}", crate::report::bytes(c.xmp_len)));
            }
            if c.jumb_len > 0 {
                parts.push(format!("JUMBF {}", crate::report::bytes(c.jumb_len)));
            }
            let compressed = if c.brotli_boxes > 0 { ", brotli" } else { "" };
            let level = if meta > opts.max_metadata {
                Level::Warn
            } else {
                Level::Info
            };
            out.push(Check {
                id: "metadata",
                level,
                message: format!(
                    "{} of metadata ({share:.1}% of file{compressed}): {}; strip for web delivery unless needed",
                    crate::report::bytes(meta),
                    parts.join(", ")
                ),
            });
        } else {
            out.push(Check {
                id: "metadata",
                level: Level::Pass,
                message: "no Exif/XMP/JUMBF boxes".into(),
            });
        }
        if c.other_len > 0 {
            out.push(Check {
                id: "metadata",
                level: Level::Info,
                message: format!(
                    "{} in boxes this tool does not classify",
                    crate::report::bytes(c.other_len)
                ),
            });
        }
        if let Some(jbrd) = c.jbrd_len {
            out.push(Check {
                id: "jpeg-reconstruction",
                level: Level::Info,
                message: format!(
                    "jbrd {} keeps the original JPEG reconstructible; decoders ignore it for display (cjxl --allow_jpeg_reconstruction=0 drops it)",
                    crate::report::bytes(jbrd)
                ),
            });
        }
    }

    // Coding.
    let coding = report.coding();
    let coding_level = match coding {
        Coding::XybOnly | Coding::OriginalOnly => Level::Info,
        _ => Level::Pass,
    };
    let mut coding_msg = coding.describe().to_string();
    if report.codestream.first_frame.is_some_and(|f| f.is_preview) {
        coding_msg.push_str("; the first frame is the preview, the main frame was not inspected");
    }
    if let Some(note) = &report.codestream.frame_note {
        coding_msg.push_str("; ");
        coding_msg.push_str(note);
    }
    out.push(Check {
        id: "coding",
        level: coding_level,
        message: coding_msg,
    });

    // Color.
    if img.color.want_icc {
        out.push(Check {
            id: "color",
            level: Level::Info,
            message: "embedded ICC profile; browsers convert on decode, which is fine but not free"
                .into(),
        });
    } else if !img.color.is_srgb() && img.color.color_space != crate::codestream::ColorSpace::Xyb {
        out.push(Check {
            id: "color",
            level: Level::Info,
            message: format!("non-sRGB color encoding: {}", img.color.describe()),
        });
    }

    // Orientation.
    if img.orientation != 1 {
        let (dw, dh) = img.displayed_size();
        out.push(Check {
            id: "orientation",
            level: Level::Info,
            message: format!(
                "orientation {} ({}): stored {}x{}, displayed {}x{}; use the displayed size in width/height attributes",
                img.orientation,
                crate::codestream::orientation_name(img.orientation),
                img.width,
                img.height,
                dw,
                dh
            ),
        });
    }

    // HDR.
    let tm = &img.tone_mapping;
    if img.color.transfer_function.is_hdr() || tm.intensity_target > 255.0 {
        out.push(Check {
            id: "hdr",
            level: Level::Info,
            message: format!(
                "HDR signaling: {}, intensity target {} nits; SDR displays tone-map",
                img.color.transfer_function.name(),
                tm.intensity_target
            ),
        });
    }

    // Animation and preview.
    if let Some(anim) = &img.animation {
        let loops = if anim.num_loops == 0 {
            "loops forever".to_string()
        } else {
            format!("{} loops", anim.num_loops)
        };
        out.push(Check {
            id: "animation",
            level: Level::Info,
            message: format!(
                "animated ({}/{} ticks per second, {loops}); frame count needs a full parse",
                anim.tps_numerator, anim.tps_denominator
            ),
        });
    }
    if let Some((pw, ph)) = img.preview {
        out.push(Check {
            id: "preview",
            level: Level::Info,
            message: format!("embedded preview frame {pw}x{ph}; extra bytes browsers do not show"),
        });
    }

    // Alpha.
    if let Some(alpha) = img.alpha() {
        out.push(Check {
            id: "alpha",
            level: Level::Info,
            message: format!(
                "alpha channel, {}-bit, {}",
                alpha.bit_depth.bits_per_sample,
                if alpha.alpha_premultiplied {
                    "premultiplied"
                } else {
                    "straight (non-premultiplied)"
                }
            ),
        });
    }

    // Budgets.
    if let Some(max) = opts.max_bytes {
        let level = if report.file_len > max {
            Level::Fail
        } else {
            Level::Pass
        };
        out.push(Check {
            id: "bytes",
            level,
            message: format!(
                "{} (budget {})",
                crate::report::bytes(report.file_len),
                crate::report::bytes(max)
            ),
        });
    }
    if let Some(max) = opts.max_pixels {
        let (dw, dh) = img.displayed_size();
        let px = u64::from(dw) * u64::from(dh);
        let level = if px > max { Level::Fail } else { Level::Pass };
        out.push(Check {
            id: "pixels",
            level,
            message: format!("{dw}x{dh} = {px} pixels (budget {max})"),
        });
    }

    out
}

/// Worst level across the checks, with `strict` promoting warnings.
pub fn verdict(checks: &[Check], strict: bool) -> Level {
    let worst = checks.iter().map(|c| c.level).max().unwrap_or(Level::Pass);
    if strict && worst == Level::Warn {
        Level::Fail
    } else {
        worst
    }
}
