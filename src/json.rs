//! JSON output, written by hand: jxlpeek only ever writes JSON, so a few
//! helpers replace `serde` in the dependency tree. Document shape is in
//! the README.

use crate::check::{Check, Level};
use crate::inspect::Report;
use std::fmt::Write as _;

pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn opt_u64(v: Option<u64>) -> String {
    v.map_or("null".to_string(), |n| n.to_string())
}

fn f32_json(v: f32) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".to_string()
    }
}

pub fn file_document(report: &Report, checks: &[Check], verdict: Level) -> String {
    let img = &report.codestream.image;
    let mut o = String::new();
    o.push('{');
    let _ = write!(o, "\"path\":{},", quote(&report.path));
    let _ = write!(o, "\"bytes\":{},", report.file_len);
    let _ = write!(
        o,
        "\"format\":{},",
        if report.is_container() {
            "\"container\""
        } else {
            "\"codestream\""
        }
    );
    match &report.container {
        None => o.push_str("\"container\":null,"),
        Some(c) => {
            o.push_str("\"container\":{");
            let _ = write!(o, "\"level\":{},", opt_u64(c.level.map(u64::from)));
            let _ = write!(
                o,
                "\"ftyp_minor_version\":{},",
                opt_u64(c.ftyp_minor_version.map(u64::from))
            );
            let _ = write!(o, "\"codestream_bytes\":{},", report.codestream_len);
            let _ = write!(o, "\"codestream_parts\":{},", c.codestream_parts);
            let _ = write!(o, "\"jbrd_bytes\":{},", opt_u64(c.jbrd_len));
            let _ = write!(o, "\"exif_bytes\":{},", c.exif_len);
            let _ = write!(o, "\"xmp_bytes\":{},", c.xmp_len);
            let _ = write!(o, "\"jumbf_bytes\":{},", c.jumb_len);
            let _ = write!(o, "\"other_bytes\":{},", c.other_len);
            let _ = write!(o, "\"brotli_boxes\":{},", c.brotli_boxes);
            o.push_str("\"boxes\":[");
            for (i, b) in c.boxes.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                let _ = write!(
                    o,
                    "{{\"type\":{},\"offset\":{},\"size\":{},\"content_bytes\":{},\"inner_type\":{},\"jxlp_index\":{},\"jxlp_last\":{},\"to_eof\":{},\"truncated\":{}}}",
                    quote(&b.kind),
                    b.offset,
                    b.header_len + b.content_len,
                    b.content_len,
                    b.inner_kind.as_deref().map_or("null".to_string(), quote),
                    opt_u64(b.jxlp.map(|(i, _)| u64::from(i))),
                    b.jxlp.map_or("null".to_string(), |(_, l)| l.to_string()),
                    b.to_eof,
                    b.truncated
                );
            }
            o.push_str("],");
            o.push_str("\"issues\":[");
            for (i, issue) in c.issues.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                o.push_str(&quote(issue));
            }
            o.push_str("]},");
        }
    }

    o.push_str("\"image\":{");
    let (dw, dh) = img.displayed_size();
    let _ = write!(o, "\"width\":{},\"height\":{},", img.width, img.height);
    let _ = write!(o, "\"displayed_width\":{dw},\"displayed_height\":{dh},");
    let _ = write!(
        o,
        "\"orientation\":{},\"orientation_name\":{},",
        img.orientation,
        quote(crate::codestream::orientation_name(img.orientation))
    );
    let _ = write!(
        o,
        "\"intrinsic_size\":{},",
        img.intrinsic_size
            .map_or("null".to_string(), |(w, h)| format!("[{w},{h}]"))
    );
    let _ = write!(
        o,
        "\"preview\":{},",
        img.preview
            .map_or("null".to_string(), |(w, h)| format!("[{w},{h}]"))
    );
    let _ = write!(
        o,
        "\"bits_per_sample\":{},\"float_samples\":{},\"exponent_bits\":{},",
        img.bit_depth.bits_per_sample, img.bit_depth.floating_point, img.bit_depth.exponent_bits
    );
    let _ = write!(o, "\"color_channels\":{},", img.color_channels());
    let _ = write!(o, "\"xyb_encoded\":{},", img.xyb_encoded);
    let _ = write!(
        o,
        "\"modular_16bit_sufficient\":{},",
        img.modular_16bit_sufficient
    );
    o.push_str("\"extra_channels\":[");
    for (i, c) in img.extra_channels.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(
            o,
            "{{\"type\":{},\"bits_per_sample\":{},\"dim_shift\":{},\"name\":{},\"alpha_premultiplied\":{}}}",
            quote(&c.kind.name()),
            c.bit_depth.bits_per_sample,
            c.dim_shift,
            quote(&c.name),
            c.alpha_premultiplied
        );
    }
    o.push_str("],");
    let col = &img.color;
    let _ = write!(
        o,
        "\"color\":{{\"description\":{},\"want_icc\":{},\"color_space\":{},\"white_point\":{},\"primaries\":{},\"transfer_function\":{},\"rendering_intent\":{},\"is_srgb\":{}}},",
        quote(&col.describe()),
        col.want_icc,
        quote(&col.color_space.name()),
        quote(&col.white_point.name()),
        quote(&col.primaries.name()),
        quote(&col.transfer_function.name()),
        quote(&col.rendering_intent.name()),
        col.is_srgb()
    );
    let tm = &img.tone_mapping;
    let _ = write!(
        o,
        "\"tone_mapping\":{{\"intensity_target\":{},\"min_nits\":{},\"relative_to_max_display\":{},\"linear_below\":{}}},",
        f32_json(tm.intensity_target),
        f32_json(tm.min_nits),
        tm.relative_to_max_display,
        f32_json(tm.linear_below)
    );
    match &img.animation {
        None => o.push_str("\"animation\":null,"),
        Some(a) => {
            let _ = write!(
                o,
                "\"animation\":{{\"tps_numerator\":{},\"tps_denominator\":{},\"num_loops\":{},\"have_timecodes\":{}}},",
                a.tps_numerator, a.tps_denominator, a.num_loops, a.have_timecodes
            );
        }
    }
    let _ = write!(o, "\"extensions\":{}", img.extensions);
    o.push_str("},");

    match report.codestream.first_frame {
        None => o.push_str("\"first_frame\":null,"),
        Some(f) => {
            let _ = write!(
                o,
                "\"first_frame\":{{\"frame_type\":{},\"encoding\":{},\"is_preview\":{}}},",
                quote(f.frame_type.name()),
                quote(f.encoding.name()),
                f.is_preview
            );
        }
    }
    let _ = write!(
        o,
        "\"frame_note\":{},",
        report
            .codestream
            .frame_note
            .as_deref()
            .map_or("null".to_string(), quote)
    );
    let coding = report.coding();
    let _ = write!(
        o,
        "\"coding\":{},\"coding_description\":{},",
        quote(coding.id()),
        quote(coding.describe())
    );

    o.push_str("\"checks\":[");
    for (i, c) in checks.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(
            o,
            "{{\"id\":{},\"level\":{},\"message\":{}}}",
            quote(c.id),
            quote(c.level.id()),
            quote(&c.message)
        );
    }
    o.push_str("],");
    let _ = write!(o, "\"verdict\":{}", quote(verdict.id()));
    o.push('}');
    o
}

pub fn error_document(path: &str, error: &str) -> String {
    format!(
        "{{\"path\":{},\"error\":{},\"verdict\":\"error\"}}",
        quote(path),
        quote(error)
    )
}

pub fn root_document(version: &str, files: &[String], verdict: &str) -> String {
    format!(
        "{{\"jxlpeek\":{},\"files\":[{}],\"verdict\":{}}}",
        quote(version),
        files.join(","),
        quote(verdict)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(quote("\u{01}"), "\"\\u0001\"");
    }
}
