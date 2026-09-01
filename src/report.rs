//! Human-readable output.

use crate::check::{Check, Level};
use crate::inspect::Report;
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Never,
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    color: bool,
}

impl Style {
    pub fn new(choice: ColorChoice, is_tty: bool) -> Self {
        Style {
            color: choice == ColorChoice::Auto && is_tty && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn level(&self, level: Level) -> String {
        let code = match level {
            Level::Pass => "32",
            Level::Info => "36",
            Level::Warn => "33",
            Level::Fail => "31;1",
        };
        self.paint(code, level.label())
    }

    fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }
}

/// `3,880 bytes`, `1.2 MB` style: exact below 10 KB, else rounded with the
/// exact count in parentheses is too noisy, so one unit only.
pub fn bytes(n: u64) -> String {
    if n < 10_000 {
        format!("{} bytes", group(n))
    } else if n < 1_000_000 {
        format!("{:.1} KB", n as f64 / 1000.0)
    } else {
        format!("{:.2} MB", n as f64 / 1_000_000.0)
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn render(
    report: &Report,
    checks: &[Check],
    verdict: Level,
    show_boxes: bool,
    style: Style,
) -> String {
    let mut out = String::new();
    let img = &report.codestream.image;
    let _ = writeln!(
        out,
        "{}  {}",
        style.bold(&report.path),
        bytes(report.file_len)
    );

    match &report.container {
        None => {
            let _ = writeln!(
                out,
                "  format       bare codestream (ISO/IEC 18181-1), no container"
            );
        }
        Some(c) => {
            let _ = writeln!(
                out,
                "  format       ISOBMFF container (ISO/IEC 18181-2){}, boxes: {}",
                c.level.map_or(String::new(), |l| format!(", level {l}")),
                c.box_kinds().join(" ")
            );
            let parts = if c.codestream_parts > 1 {
                format!(" in {} jxlp parts", c.codestream_parts)
            } else if c.codestream_parts == 1 {
                " in 1 jxlp part".to_string()
            } else {
                " in one jxlc box".to_string()
            };
            let _ = writeln!(
                out,
                "  codestream   {}{parts} ({:.1}% of file)",
                bytes(report.codestream_len),
                report.codestream_len as f64 * 100.0 / report.file_len.max(1) as f64
            );
        }
    }

    let (dw, dh) = img.displayed_size();
    let mut dims = format!("{}x{}", img.width, img.height);
    if img.orientation != 1 {
        let _ = write!(
            dims,
            " stored, {dw}x{dh} displayed (orientation {}: {})",
            img.orientation,
            crate::codestream::orientation_name(img.orientation)
        );
    }
    let depth = if img.bit_depth.floating_point {
        format!(
            "{}-bit float (exp {})",
            img.bit_depth.bits_per_sample, img.bit_depth.exponent_bits
        )
    } else {
        format!("{}-bit", img.bit_depth.bits_per_sample)
    };
    let channels = match img.color_channels() {
        1 => "grayscale".to_string(),
        _ => "RGB".to_string(),
    };
    let alpha = if img.alpha().is_some() { "+alpha" } else { "" };
    let _ = writeln!(out, "  image        {dims}, {depth} {channels}{alpha}");
    if let Some((iw, ih)) = img.intrinsic_size {
        let _ = writeln!(out, "  intrinsic    {iw}x{ih}");
    }
    let _ = writeln!(out, "  color        {}", img.color.describe());
    let _ = writeln!(out, "  coding       {}", report.coding().describe());
    if let Some(frame) = report.codestream.first_frame {
        let _ = writeln!(
            out,
            "  first frame  {} frame, {} encoding{}",
            frame.frame_type.name(),
            frame.encoding.name(),
            if frame.is_preview { " (preview)" } else { "" }
        );
    } else if let Some(note) = &report.codestream.frame_note {
        let _ = writeln!(out, "  first frame  not inspected: {note}");
    }
    if !img.extra_channels.is_empty() {
        let list: Vec<String> = img
            .extra_channels
            .iter()
            .map(|c| {
                let mut s = format!("{} {}-bit", c.kind.name(), c.bit_depth.bits_per_sample);
                if c.alpha_premultiplied {
                    s.push_str(" premultiplied");
                }
                if !c.name.is_empty() {
                    let _ = write!(s, " \"{}\"", c.name);
                }
                s
            })
            .collect();
        let _ = writeln!(out, "  extra        {}", list.join(", "));
    }
    if let Some(anim) = &img.animation {
        let _ = writeln!(
            out,
            "  animation    {}/{} ticks per second, {}{}",
            anim.tps_numerator,
            anim.tps_denominator,
            if anim.num_loops == 0 {
                "loops forever".to_string()
            } else {
                format!("{} loops", anim.num_loops)
            },
            if anim.have_timecodes {
                ", timecodes"
            } else {
                ""
            }
        );
    }
    if let Some((pw, ph)) = img.preview {
        let _ = writeln!(out, "  preview      {pw}x{ph}");
    }
    let tm = &img.tone_mapping;
    let _ = writeln!(
        out,
        "  tone         intensity target {} nits{}{}",
        tm.intensity_target,
        if tm.intensity_target > 255.0 || img.color.transfer_function.is_hdr() {
            " (HDR)"
        } else {
            " (SDR)"
        },
        if tm.min_nits > 0.0 {
            format!(", min {} nits", tm.min_nits)
        } else {
            String::new()
        }
    );
    if img.extensions != 0 {
        let _ = writeln!(
            out,
            "  extensions   metadata extension mask {:#x}",
            img.extensions
        );
    }

    if show_boxes {
        if let Some(c) = &report.container {
            let _ = writeln!(out, "  boxes");
            let _ = writeln!(out, "    {:>10}  {:>10}  type", "offset", "size");
            for b in &c.boxes {
                let mut extra = String::new();
                if let Some(inner) = &b.inner_kind {
                    let _ = write!(extra, "  brotli-wrapped '{inner}'");
                }
                if let Some((idx, last)) = b.jxlp {
                    let _ = write!(extra, "  part {idx}{}", if last { " (last)" } else { "" });
                }
                if b.to_eof {
                    extra.push_str("  (size 0: to end of file)");
                }
                if b.truncated {
                    extra.push_str("  TRUNCATED");
                }
                let _ = writeln!(
                    out,
                    "    {:>10}  {:>10}  '{}'{extra}",
                    b.offset,
                    b.header_len + b.content_len,
                    b.kind
                );
            }
        }
    }

    let _ = writeln!(out, "  checks");
    for c in checks {
        let _ = writeln!(
            out,
            "    {} {:<20} {}",
            style.level(c.level),
            c.id,
            c.message
        );
    }
    let _ = writeln!(out, "  verdict      {}", style.level(verdict));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatting() {
        assert_eq!(bytes(0), "0 bytes");
        assert_eq!(bytes(3880), "3,880 bytes");
        assert_eq!(bytes(12_345), "12.3 KB");
        assert_eq!(bytes(2_500_000), "2.50 MB");
    }
}
