//! Every frame of a codestream: FrameHeader plus TOC, then a jump over the
//! frame's sections to the next header (ISO/IEC 18181-1, "Frames").
//!
//! A frame is `ZeroPadToByte`, FrameHeader, TOC, then the sections the TOC
//! sizes. The TOC is a `permuted` bit, a permutation when set, a byte
//! boundary, one `U32` size per section and another byte boundary. Adding
//! the sizes gives the byte where the next frame starts, so the whole frame
//! list is read without entropy decoding -- except when the TOC is permuted
//! (the permutation is entropy-coded) or an ICC profile precedes the first
//! frame. In both cases the list stops there and says why.
//!
//! Field order and conditions follow libjxl `frame_header.cc`,
//! `loop_filter.cc`, `toc.cc` and `frame_dimensions.h` (commit 9e02071) and
//! jxl-rs `headers/frame_header.rs` / `toc.rs` (commit 87620c4).

use crate::bits::{unpack_signed, BitReader, HeaderError, Result, U32};
use crate::codestream::{FrameEncoding, FrameType, ImageInfo};

/// Stop after this many frames; a real animation rarely has more, and the
/// limit keeps a hostile file from making the report unbounded.
pub const MAX_FRAMES: usize = 65_536;

/// libjxl rejects frames with more TOC entries than this (`toc.cc`).
const MAX_TOC_ENTRIES: u64 = 65_536;

const FLAG_USE_LF_FRAME: u64 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Replace,
    Add,
    Blend,
    AlphaWeightedAdd,
    Mul,
}

impl BlendMode {
    pub fn name(&self) -> &'static str {
        match self {
            BlendMode::Replace => "replace",
            BlendMode::Add => "add",
            BlendMode::Blend => "blend",
            BlendMode::AlphaWeightedAdd => "alpha-weighted-add",
            BlendMode::Mul => "mul",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The preview frame, which precedes the main frames when the image
    /// header declares one.
    pub is_preview: bool,
    pub frame_type: FrameType,
    pub encoding: FrameEncoding,
    /// Byte offset of the frame header within the codestream.
    pub offset: u64,
    /// Header + TOC bytes, and the bytes of the sections the TOC sizes.
    pub header_bytes: u64,
    pub data_bytes: u64,
    pub toc_entries: u32,
    /// The TOC is permuted, so `data_bytes` is unknown (reported as 0).
    pub toc_permuted: bool,
    /// Position on the canvas and coded size (before LF-level scaling).
    pub x0: i32,
    pub y0: i32,
    pub width: u32,
    pub height: u32,
    pub cropped: bool,
    pub upsampling: u32,
    pub lf_level: u32,
    pub num_passes: u32,
    pub blend: BlendMode,
    /// Duration in animation ticks (0 without an animation header).
    pub duration_ticks: u32,
    pub is_last: bool,
    pub save_as_reference: u32,
    pub name: String,
}

impl Frame {
    /// A frame the viewer sees: libjxl/jxl-rs `is_visible`. Zero-duration
    /// regular frames are composited into the next one.
    pub fn is_displayed(&self) -> bool {
        !self.is_preview
            && matches!(
                self.frame_type,
                FrameType::Regular | FrameType::SkipProgressive
            )
            && (self.is_last || self.duration_ticks > 0)
    }

    pub fn total_bytes(&self) -> u64 {
        self.header_bytes + self.data_bytes
    }

    /// Regular and skip-progressive frames carry an origin and blending.
    pub fn is_normal(&self) -> bool {
        matches!(
            self.frame_type,
            FrameType::Regular | FrameType::SkipProgressive
        )
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameList {
    pub frames: Vec<Frame>,
    /// True when the walk reached the frame marked `is_last`.
    pub complete: bool,
    /// Why the list stopped early or is missing sizes, when it did.
    pub note: Option<String>,
    /// The walk could not follow the stream as far as it goes: a header
    /// failed to parse or the codestream is shorter than its frames say.
    pub damaged: bool,
    /// Bytes after the last frame (a well-formed codestream has none).
    pub trailing_bytes: u64,
}

impl FrameList {
    /// Frames the viewer sees (see [`Frame::is_displayed`]).
    pub fn displayed(&self) -> usize {
        self.frames.iter().filter(|f| f.is_displayed()).count()
    }

    pub fn total_ticks(&self) -> u64 {
        self.frames
            .iter()
            .filter(|f| !f.is_preview)
            .map(|f| u64::from(f.duration_ticks))
            .sum()
    }

    /// The first frame that is not the preview.
    pub fn main_frame(&self) -> Option<&Frame> {
        self.frames.iter().find(|f| !f.is_preview)
    }
}

/// Milliseconds for a tick count at `numerator / denominator` ticks per
/// second (libjxl `FrameHeader::duration` arithmetic).
pub fn ticks_to_ms(ticks: u64, numerator: u32, denominator: u32) -> f64 {
    if numerator == 0 {
        return 0.0;
    }
    ticks as f64 * 1000.0 * f64::from(denominator) / f64::from(numerator)
}

/// Walk the frames starting at `start_bit` (the end of the image headers).
pub fn walk(data: &[u8], start_bit: u64, image: &ImageInfo) -> FrameList {
    let mut list = FrameList::default();
    if image.color.want_icc {
        list.note =
            Some("embedded ICC profile is entropy-coded; frame headers not inspected".to_string());
        return list;
    }
    let mut br = BitReader::new(data);
    if let Err(err) = br.skip(start_bit) {
        list.note = Some(format!("frame headers unreadable: {err}"));
        return list;
    }
    let mut expect_preview = image.preview.is_some();
    loop {
        if list.frames.len() >= MAX_FRAMES {
            list.note = Some(format!("stopped after {MAX_FRAMES} frames"));
            return list;
        }
        let parsed = br
            .jump_to_byte_boundary()
            .and_then(|_| read_frame(&mut br, image, expect_preview));
        let (frame, step) = match parsed {
            Ok(Step::Frame(frame)) => (frame, None),
            Ok(Step::Permuted(frame)) => {
                let last = frame.is_last && !frame.is_preview;
                list.frames.push(frame);
                if last {
                    list.complete = true;
                    list.note = Some(
                        "TOC is permuted (entropy-coded section order); frame sizes not read"
                            .to_string(),
                    );
                } else {
                    list.note = Some(
                        "TOC is permuted (entropy-coded section order); later frames not inspected"
                            .to_string(),
                    );
                }
                return list;
            }
            Err(err) => (
                Frame::placeholder(),
                Some(format!(
                    "frame {} header unreadable: {err}",
                    list.frames.len()
                )),
            ),
        };
        if let Some(note) = step {
            list.note = Some(note);
            list.damaged = true;
            return list;
        }
        let end = frame.offset + frame.total_bytes();
        let last = frame.is_last && !frame.is_preview;
        let length = data.len() as u64;
        list.frames.push(frame);
        if end > length {
            list.note = Some(format!(
                "codestream ends {} bytes before frame {} does",
                end - length,
                list.frames.len() - 1
            ));
            list.damaged = true;
            return list;
        }
        if last {
            list.complete = true;
            list.trailing_bytes = length - end;
            return list;
        }
        expect_preview = false;
        // Sections are byte sized, so `end` is a byte boundary.
        br = BitReader::new(data);
        if let Err(err) = br.skip(end * 8) {
            list.note = Some(format!("frame headers unreadable: {err}"));
            return list;
        }
        if end == length {
            list.note = Some(format!(
                "codestream ends after frame {} without a last frame",
                list.frames.len() - 1
            ));
            list.damaged = true;
            return list;
        }
    }
}

enum Step {
    Frame(Frame),
    /// Header read; the permuted TOC hides where the frame ends.
    Permuted(Frame),
}

impl Frame {
    fn placeholder() -> Frame {
        Frame {
            is_preview: false,
            frame_type: FrameType::Regular,
            encoding: FrameEncoding::VarDct,
            offset: 0,
            header_bytes: 0,
            data_bytes: 0,
            toc_entries: 0,
            toc_permuted: false,
            x0: 0,
            y0: 0,
            width: 0,
            height: 0,
            cropped: false,
            upsampling: 1,
            lf_level: 0,
            num_passes: 1,
            blend: BlendMode::Replace,
            duration_ticks: 0,
            is_last: true,
            save_as_reference: 0,
            name: String::new(),
        }
    }
}

fn blend_mode(br: &mut BitReader) -> Result<BlendMode> {
    Ok(
        match br.u32([U32::Val(0), U32::Val(1), U32::Val(2), U32::BitsOffset(2, 3)])? {
            0 => BlendMode::Replace,
            1 => BlendMode::Add,
            2 => BlendMode::Blend,
            3 => BlendMode::AlphaWeightedAdd,
            4 => BlendMode::Mul,
            other => {
                return Err(HeaderError::Invalid(format!("blend mode {other}")));
            }
        },
    )
}

/// BlendingInfo; returns the mode.
fn read_blending(br: &mut BitReader, num_extra: usize, partial: bool) -> Result<BlendMode> {
    let mode = blend_mode(br)?;
    let uses_alpha =
        num_extra > 0 && matches!(mode, BlendMode::Blend | BlendMode::AlphaWeightedAdd);
    if uses_alpha {
        let alpha = br.u32([U32::Val(0), U32::Val(1), U32::Val(2), U32::BitsOffset(3, 3)])?;
        if alpha as usize >= num_extra {
            return Err(HeaderError::Invalid(format!(
                "blending alpha channel {alpha} of {num_extra}"
            )));
        }
    }
    if uses_alpha || mode == BlendMode::Mul {
        br.bool()?; // clamp
    }
    if mode != BlendMode::Replace || partial {
        br.u32([U32::Val(0), U32::Val(1), U32::Val(2), U32::Val(3)])?; // source
    }
    Ok(mode)
}

/// LoopFilter (restoration filter): only skipped over.
fn skip_loop_filter(br: &mut BitReader, modular: bool) -> Result<()> {
    if br.bool()? {
        return Ok(()); // all_default
    }
    if br.bool()? && br.bool()? {
        // gab, then gab_custom: six half floats.
        for _ in 0..6 {
            br.f16()?;
        }
    }
    let epf_iters = br.bits(2)?;
    if epf_iters > 0 {
        if !modular && br.bool()? {
            for _ in 0..8 {
                br.f16()?;
            }
        }
        if br.bool()? {
            for _ in 0..5 {
                br.f16()?;
            }
        }
        if br.bool()? {
            let count = if modular { 3 } else { 4 };
            for _ in 0..count {
                br.f16()?;
            }
        }
        if modular {
            br.f16()?;
        }
    }
    br.extensions()?;
    Ok(())
}

const CROP: [U32; 4] = [
    U32::Bits(8),
    U32::BitsOffset(11, 256),
    U32::BitsOffset(14, 2304),
    U32::BitsOffset(30, 18688),
];

const TOC_ENTRY: [U32; 4] = [
    U32::Bits(10),
    U32::BitsOffset(14, 1024),
    U32::BitsOffset(22, 17408),
    U32::BitsOffset(30, 4_211_712),
];

fn div_ceil(a: u64, b: u64) -> u64 {
    a.div_ceil(b)
}

fn read_frame(br: &mut BitReader, image: &ImageInfo, is_preview: bool) -> Result<Step> {
    let offset = br.position() / 8;
    let num_extra = image.extra_channels.len();
    let (default_w, default_h) = if is_preview {
        image.preview.unwrap_or((image.width, image.height))
    } else {
        (image.width, image.height)
    };
    let have_animation = image.animation.is_some();
    let have_timecodes = image.animation.as_ref().is_some_and(|a| a.have_timecodes);

    let mut f = Frame::placeholder();
    f.is_preview = is_preview;
    f.offset = offset;
    f.width = default_w;
    f.height = default_h;
    let mut group_size_shift = 1u32;
    let mut max_hshift = 0u32;
    let mut max_vshift = 0u32;

    let all_default = br.bool()?;
    if !all_default {
        f.frame_type = match br.bits(2)? {
            0 => FrameType::Regular,
            1 => FrameType::Lf,
            2 => FrameType::ReferenceOnly,
            _ => FrameType::SkipProgressive,
        };
        if is_preview && f.frame_type != FrameType::Regular {
            return Err(HeaderError::Invalid(
                "the preview must be a regular frame".to_string(),
            ));
        }
        f.encoding = if br.bool()? {
            FrameEncoding::Modular
        } else {
            FrameEncoding::VarDct
        };
        let modular = f.encoding == FrameEncoding::Modular;
        let flags = br.u64()?;
        let use_lf_frame = flags & FLAG_USE_LF_FRAME != 0;
        let ycbcr = !image.xyb_encoded && br.bool()?;
        if ycbcr && !use_lf_frame {
            // YCbCrChromaSubsampling: one 2-bit mode per channel.
            const H: [u32; 4] = [0, 1, 1, 0];
            const V: [u32; 4] = [0, 1, 0, 1];
            for _ in 0..3 {
                let mode = br.bits(2)? as usize;
                max_hshift = max_hshift.max(H[mode]);
                max_vshift = max_vshift.max(V[mode]);
            }
        }
        if !use_lf_frame {
            let ups = [U32::Val(1), U32::Val(2), U32::Val(4), U32::Val(8)];
            f.upsampling = br.u32(ups)?;
            for _ in 0..num_extra {
                br.u32(ups)?;
            }
        }
        if modular {
            group_size_shift = br.bits(2)?;
        }
        if !modular && image.xyb_encoded {
            br.bits(3)?; // x_qm_scale
            br.bits(3)?; // b_qm_scale
        }
        if f.frame_type != FrameType::ReferenceOnly {
            f.num_passes =
                br.u32([U32::Val(1), U32::Val(2), U32::Val(3), U32::BitsOffset(3, 4)])?;
            if f.num_passes != 1 {
                let num_ds =
                    br.u32([U32::Val(0), U32::Val(1), U32::Val(2), U32::BitsOffset(1, 3)])?;
                if num_ds > f.num_passes {
                    return Err(HeaderError::Invalid(format!(
                        "{num_ds} downsampling steps for {} passes",
                        f.num_passes
                    )));
                }
                br.skip(2 * u64::from(f.num_passes - 1))?; // shift per pass
                for _ in 0..num_ds {
                    br.u32([U32::Val(1), U32::Val(2), U32::Val(4), U32::Val(8)])?;
                }
                for _ in 0..num_ds {
                    br.u32([U32::Val(0), U32::Val(1), U32::Val(2), U32::Bits(3)])?;
                }
            }
        }
        if f.frame_type == FrameType::Lf {
            f.lf_level = br.u32([U32::Val(1), U32::Val(2), U32::Val(3), U32::Val(4)])?;
        }
        let normal = matches!(
            f.frame_type,
            FrameType::Regular | FrameType::SkipProgressive
        );
        let mut partial = false;
        if f.frame_type != FrameType::Lf && br.bool()? {
            f.cropped = true;
            if normal {
                f.x0 = unpack_signed(br.u32(CROP)?);
                f.y0 = unpack_signed(br.u32(CROP)?);
            }
            f.width = br.u32(CROP)?;
            f.height = br.u32(CROP)?;
            if f.width == 0 || f.height == 0 {
                return Err(HeaderError::Invalid("zero-sized frame crop".to_string()));
            }
            if normal {
                partial = f.x0 > 0
                    || f.y0 > 0
                    || i64::from(f.width) + i64::from(f.x0) < i64::from(default_w)
                    || i64::from(f.height) + i64::from(f.y0) < i64::from(default_h);
            }
        }
        if normal {
            f.blend = read_blending(br, num_extra, partial)?;
            for _ in 0..num_extra {
                read_blending(br, num_extra, partial)?;
            }
            if is_preview && (f.cropped || f.blend != BlendMode::Replace) {
                return Err(HeaderError::Invalid(
                    "the preview cannot be cropped or blended".to_string(),
                ));
            }
            if have_animation {
                f.duration_ticks =
                    br.u32([U32::Val(0), U32::Val(1), U32::Bits(8), U32::Bits(32)])?;
            }
            if have_timecodes {
                br.bits(32)?;
            }
            f.is_last = br.bool()?;
        } else {
            f.is_last = false;
        }
        if f.frame_type != FrameType::Lf && !f.is_last {
            f.save_as_reference = br.u32([U32::Val(0), U32::Val(1), U32::Val(2), U32::Val(3)])?;
        }
        if f.frame_type != FrameType::Lf {
            let can_be_referenced =
                !f.is_last && (f.duration_ticks == 0 || f.save_as_reference != 0);
            if (can_be_referenced && f.blend == BlendMode::Replace && !partial && normal)
                || f.frame_type == FrameType::ReferenceOnly
            {
                br.bool()?; // save_before_color_transform
            }
        }
        f.name = br.string()?;
        skip_loop_filter(br, modular)?;
        br.extensions()?;
    }

    // Frame dimensions (libjxl FrameDimensions::Set).
    let scale = 1u64 << (3 * f.lf_level);
    let xsize_up = div_ceil(u64::from(f.width), scale);
    let ysize_up = div_ceil(u64::from(f.height), scale);
    let ups = u64::from(f.upsampling);
    let xsize = div_ceil(xsize_up, ups);
    let ysize = div_ceil(ysize_up, ups);
    let group_dim = 128u64 << group_size_shift;
    let xblocks = div_ceil(xsize, 8 << max_hshift) << max_hshift;
    let yblocks = div_ceil(ysize, 8 << max_vshift) << max_vshift;
    let groups = div_ceil(xsize, group_dim) * div_ceil(ysize, group_dim);
    let lf_groups = div_ceil(xblocks, group_dim) * div_ceil(yblocks, group_dim);
    let passes = u64::from(f.num_passes);
    let entries = if groups == 1 && passes == 1 {
        1
    } else {
        groups
            .checked_mul(passes)
            .and_then(|v| v.checked_add(2 + lf_groups))
            .unwrap_or(u64::MAX)
    };
    if entries > MAX_TOC_ENTRIES {
        return Err(HeaderError::Invalid(format!(
            "{entries} TOC entries (limit {MAX_TOC_ENTRIES})"
        )));
    }
    f.toc_entries = entries as u32;

    // TOC.
    if br.bool()? {
        f.toc_permuted = true;
        f.header_bytes = br.position().div_ceil(8) - offset;
        return Ok(Step::Permuted(f));
    }
    br.jump_to_byte_boundary()?;
    let mut total = 0u64;
    for _ in 0..entries {
        total += u64::from(br.u32(TOC_ENTRY)?);
    }
    br.jump_to_byte_boundary()?;
    f.header_bytes = br.position() / 8 - offset;
    f.data_bytes = total;
    Ok(Step::Frame(f))
}
