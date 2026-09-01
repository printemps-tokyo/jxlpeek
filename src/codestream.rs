//! Codestream headers: SizeHeader, ImageMetadata, CustomTransformData and the
//! first bits of the first FrameHeader (ISO/IEC 18181-1).
//!
//! Field order, conditions and coders follow libjxl `image_metadata.cc` /
//! `color_encoding_internal.cc` and jxl-rs `headers/*.rs`. Defaults apply
//! whenever a field is skipped by its `all_default` flag or condition.

use crate::bits::{unpack_signed, BitReader, HeaderError, Result, U32};

/// Bare codestream signature (ISO/IEC 18181-1).
pub const SIGNATURE: [u8; 2] = [0xff, 0x0a];

#[derive(Debug, Clone, PartialEq)]
pub struct Codestream {
    pub image: ImageInfo,
    /// First frame header bits, or `None` when they are not reachable
    /// without decoding (an entropy-coded ICC profile precedes them) or the
    /// stream is truncated. `frame_note` explains why.
    pub first_frame: Option<FrameInfo>,
    pub frame_note: Option<String>,
    /// Bits consumed by SizeHeader + ImageMetadata + CustomTransformData.
    pub header_bits: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    /// Stored (pre-orientation) dimensions.
    pub width: u32,
    pub height: u32,
    /// Orientation 1..=8 (EXIF numbering).
    pub orientation: u32,
    pub intrinsic_size: Option<(u32, u32)>,
    pub preview: Option<(u32, u32)>,
    pub animation: Option<Animation>,
    pub bit_depth: BitDepth,
    pub modular_16bit_sufficient: bool,
    pub extra_channels: Vec<ExtraChannel>,
    /// True when the image is stored in the XYB color space, which is how
    /// lossy (VarDCT) encodes are made. False means the original color
    /// encoding is preserved (lossless, or a JPEG transcode).
    pub xyb_encoded: bool,
    pub color: ColorEncoding,
    pub tone_mapping: ToneMapping,
    /// Extension bit mask of ImageMetadata (0 when none).
    pub extensions: u64,
}

impl ImageInfo {
    /// Dimensions after the decoder applies `orientation`.
    pub fn displayed_size(&self) -> (u32, u32) {
        if self.orientation >= 5 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    pub fn color_channels(&self) -> u32 {
        if self.color.color_space == ColorSpace::Gray {
            1
        } else {
            3
        }
    }

    pub fn alpha(&self) -> Option<&ExtraChannel> {
        self.extra_channels
            .iter()
            .find(|c| c.kind == ExtraChannelType::Alpha)
    }
}

pub fn orientation_name(orientation: u32) -> &'static str {
    match orientation {
        1 => "normal",
        2 => "flip horizontal",
        3 => "rotate 180",
        4 => "flip vertical",
        5 => "transpose",
        6 => "rotate 90 CW",
        7 => "anti-transpose",
        8 => "rotate 90 CCW",
        _ => "unknown",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub tps_numerator: u32,
    pub tps_denominator: u32,
    /// 0 means loop forever.
    pub num_loops: u32,
    pub have_timecodes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitDepth {
    pub floating_point: bool,
    pub bits_per_sample: u32,
    pub exponent_bits: u32,
}

impl Default for BitDepth {
    fn default() -> Self {
        BitDepth {
            floating_point: false,
            bits_per_sample: 8,
            exponent_bits: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraChannelType {
    Alpha,
    Depth,
    SpotColor,
    SelectionMask,
    Black,
    Cfa,
    Thermal,
    Reserved(u32),
    Unknown,
    Optional,
    Other(u32),
}

impl ExtraChannelType {
    fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Alpha,
            1 => Self::Depth,
            2 => Self::SpotColor,
            3 => Self::SelectionMask,
            4 => Self::Black,
            5 => Self::Cfa,
            6 => Self::Thermal,
            7..=14 => Self::Reserved(v),
            15 => Self::Unknown,
            16 => Self::Optional,
            _ => Self::Other(v),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Self::Alpha => "alpha".into(),
            Self::Depth => "depth".into(),
            Self::SpotColor => "spot color".into(),
            Self::SelectionMask => "selection mask".into(),
            Self::Black => "black (CMYK K)".into(),
            Self::Cfa => "CFA".into(),
            Self::Thermal => "thermal".into(),
            Self::Reserved(v) => format!("reserved({v})"),
            Self::Unknown => "unknown".into(),
            Self::Optional => "optional".into(),
            Self::Other(v) => format!("type {v}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtraChannel {
    pub kind: ExtraChannelType,
    pub bit_depth: BitDepth,
    pub dim_shift: u32,
    pub name: String,
    /// Alpha only: premultiplied (associated) alpha.
    pub alpha_premultiplied: bool,
    pub spot_color: Option<[f32; 4]>,
    pub cfa_channel: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Rgb,
    Gray,
    Xyb,
    Unknown,
    Other(u32),
}

impl ColorSpace {
    fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Rgb,
            1 => Self::Gray,
            2 => Self::Xyb,
            3 => Self::Unknown,
            _ => Self::Other(v),
        }
    }
    pub fn name(&self) -> String {
        match self {
            Self::Rgb => "RGB".into(),
            Self::Gray => "grayscale".into(),
            Self::Xyb => "XYB".into(),
            Self::Unknown => "unknown".into(),
            Self::Other(v) => format!("color space {v}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhitePoint {
    D65,
    Custom(f64, f64),
    E,
    Dci,
    Other(u32),
}

impl WhitePoint {
    pub fn name(&self) -> String {
        match self {
            Self::D65 => "D65".into(),
            Self::Custom(x, y) => format!("custom white ({x:.4}, {y:.4})"),
            Self::E => "E".into(),
            Self::Dci => "DCI".into(),
            Self::Other(v) => format!("white point {v}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Primaries {
    Srgb,
    Custom([(f64, f64); 3]),
    Bt2100,
    P3,
    Other(u32),
}

impl Primaries {
    pub fn name(&self) -> String {
        match self {
            Self::Srgb => "sRGB primaries".into(),
            Self::Custom(p) => format!(
                "custom primaries r({:.4}, {:.4}) g({:.4}, {:.4}) b({:.4}, {:.4})",
                p[0].0, p[0].1, p[1].0, p[1].1, p[2].0, p[2].1
            ),
            Self::Bt2100 => "BT.2100 primaries".into(),
            Self::P3 => "P3 primaries".into(),
            Self::Other(v) => format!("primaries {v}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransferFunction {
    Bt709,
    Unknown,
    Linear,
    Srgb,
    Pq,
    Dci,
    Hlg,
    /// Stored as gamma * 1e7 in the stream; kept here as the exponent.
    Gamma(f64),
    Other(u32),
}

impl TransferFunction {
    fn from_raw(v: u32) -> Self {
        match v {
            1 => Self::Bt709,
            2 => Self::Unknown,
            8 => Self::Linear,
            13 => Self::Srgb,
            16 => Self::Pq,
            17 => Self::Dci,
            18 => Self::Hlg,
            _ => Self::Other(v),
        }
    }
    pub fn name(&self) -> String {
        match self {
            Self::Bt709 => "BT.709 transfer".into(),
            Self::Unknown => "unknown transfer".into(),
            Self::Linear => "linear transfer".into(),
            Self::Srgb => "sRGB transfer".into(),
            Self::Pq => "PQ transfer (HDR)".into(),
            Self::Dci => "DCI transfer".into(),
            Self::Hlg => "HLG transfer (HDR)".into(),
            Self::Gamma(g) => format!("gamma {g:.4} transfer"),
            Self::Other(v) => format!("transfer function {v}"),
        }
    }
    pub fn is_hdr(&self) -> bool {
        matches!(self, Self::Pq | Self::Hlg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderingIntent {
    Perceptual,
    Relative,
    Saturation,
    Absolute,
    Other(u32),
}

impl RenderingIntent {
    fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Perceptual,
            1 => Self::Relative,
            2 => Self::Saturation,
            3 => Self::Absolute,
            _ => Self::Other(v),
        }
    }
    pub fn name(&self) -> String {
        match self {
            Self::Perceptual => "perceptual".into(),
            Self::Relative => "relative".into(),
            Self::Saturation => "saturation".into(),
            Self::Absolute => "absolute".into(),
            Self::Other(v) => format!("intent {v}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColorEncoding {
    /// An ICC profile follows the headers and defines the color encoding.
    pub want_icc: bool,
    pub color_space: ColorSpace,
    pub white_point: WhitePoint,
    pub primaries: Primaries,
    pub transfer_function: TransferFunction,
    pub rendering_intent: RenderingIntent,
}

impl Default for ColorEncoding {
    fn default() -> Self {
        ColorEncoding {
            want_icc: false,
            color_space: ColorSpace::Rgb,
            white_point: WhitePoint::D65,
            primaries: Primaries::Srgb,
            transfer_function: TransferFunction::Srgb,
            rendering_intent: RenderingIntent::Relative,
        }
    }
}

impl ColorEncoding {
    /// True for the plain sRGB encoding browsers need no conversion for.
    pub fn is_srgb(&self) -> bool {
        !self.want_icc
            && matches!(self.color_space, ColorSpace::Rgb | ColorSpace::Gray)
            && self.white_point == WhitePoint::D65
            && (self.color_space == ColorSpace::Gray || self.primaries == Primaries::Srgb)
            && self.transfer_function == TransferFunction::Srgb
    }

    pub fn describe(&self) -> String {
        if self.want_icc {
            return format!("{}, embedded ICC profile", self.color_space.name());
        }
        match self.color_space {
            ColorSpace::Xyb => "XYB".to_string(),
            ColorSpace::Gray => format!(
                "grayscale, {}, {}, {} intent",
                self.white_point.name(),
                self.transfer_function.name(),
                self.rendering_intent.name()
            ),
            _ => format!(
                "{}, {}, {}, {}, {} intent",
                self.color_space.name(),
                self.white_point.name(),
                self.primaries.name(),
                self.transfer_function.name(),
                self.rendering_intent.name()
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToneMapping {
    pub intensity_target: f32,
    pub min_nits: f32,
    pub relative_to_max_display: bool,
    pub linear_below: f32,
}

impl Default for ToneMapping {
    fn default() -> Self {
        ToneMapping {
            intensity_target: 255.0,
            min_nits: 0.0,
            relative_to_max_display: false,
            linear_below: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Regular,
    Lf,
    ReferenceOnly,
    SkipProgressive,
}

impl FrameType {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Lf => "LF",
            Self::ReferenceOnly => "reference-only",
            Self::SkipProgressive => "skip-progressive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameEncoding {
    VarDct,
    Modular,
}

impl FrameEncoding {
    pub fn name(&self) -> &'static str {
        match self {
            Self::VarDct => "VarDCT",
            Self::Modular => "modular",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// The first frame is the preview when the image declares one.
    pub is_preview: bool,
    pub frame_type: FrameType,
    pub encoding: FrameEncoding,
}

/// Parse the headers of a bare codestream (must start with `FF 0A`).
pub fn parse(data: &[u8]) -> Result<Codestream> {
    if data.len() < 2 || data[0..2] != SIGNATURE {
        return Err(HeaderError::Invalid(
            "missing codestream signature FF 0A".to_string(),
        ));
    }
    let mut br = BitReader::new(data);
    br.read(16)?;
    let (width, height) = read_size(&mut br)?;
    let image = read_image_metadata(&mut br, width, height)?;
    read_custom_transform_data(&mut br, image.xyb_encoded)?;
    let header_bits = br.position();

    let (first_frame, frame_note) = if image.color.want_icc {
        (
            None,
            Some("embedded ICC profile is entropy-coded; frame headers not inspected".to_string()),
        )
    } else {
        match read_first_frame(&mut br, image.preview.is_some()) {
            Ok(frame) => (Some(frame), None),
            Err(err) => (None, Some(format!("first frame header unreadable: {err}"))),
        }
    };
    Ok(Codestream {
        image,
        first_frame,
        frame_note,
        header_bits,
    })
}

fn ratio_to_xsize(ysize: u32, ratio: u32) -> Option<u64> {
    let y = u64::from(ysize);
    Some(match ratio {
        1 => y,
        2 => y * 12 / 10,
        3 => y * 4 / 3,
        4 => y * 3 / 2,
        5 => y * 16 / 9,
        6 => y * 5 / 4,
        7 => y * 2,
        _ => return None,
    })
}

/// SizeHeader.
fn read_size(br: &mut BitReader) -> Result<(u32, u32)> {
    let small = br.bool()?;
    let ysize = if small {
        (br.bits(5)? + 1) * 8
    } else {
        1 + br.u32([U32::Bits(9), U32::Bits(13), U32::Bits(18), U32::Bits(30)])?
    };
    let ratio = br.bits(3)?;
    let xsize = match ratio_to_xsize(ysize, ratio) {
        Some(x) => x,
        None if small => u64::from((br.bits(5)? + 1) * 8),
        None => u64::from(1 + br.u32([U32::Bits(9), U32::Bits(13), U32::Bits(18), U32::Bits(30)])?),
    };
    let xsize =
        u32::try_from(xsize).map_err(|_| HeaderError::Invalid("width exceeds 2^32".to_string()))?;
    if xsize == 0 || ysize == 0 {
        return Err(HeaderError::Invalid("zero image dimension".to_string()));
    }
    Ok((xsize, ysize))
}

/// PreviewHeader.
fn read_preview(br: &mut BitReader) -> Result<(u32, u32)> {
    let div8 = br.bool()?;
    let ysize = if div8 {
        br.u32([
            U32::Val(16),
            U32::Val(32),
            U32::BitsOffset(5, 1),
            U32::BitsOffset(9, 33),
        ])? * 8
    } else {
        1 + br.u32([
            U32::Bits(6),
            U32::BitsOffset(8, 64),
            U32::BitsOffset(10, 320),
            U32::BitsOffset(12, 1344),
        ])?
    };
    let ratio = br.bits(3)?;
    let xsize = match ratio_to_xsize(ysize, ratio) {
        Some(x) => x as u32,
        None if div8 => {
            br.u32([
                U32::Val(16),
                U32::Val(32),
                U32::BitsOffset(5, 1),
                U32::BitsOffset(9, 33),
            ])? * 8
        }
        None => {
            1 + br.u32([
                U32::Bits(6),
                U32::BitsOffset(8, 64),
                U32::BitsOffset(10, 320),
                U32::BitsOffset(12, 1344),
            ])?
        }
    };
    Ok((xsize, ysize))
}

fn read_bit_depth(br: &mut BitReader) -> Result<BitDepth> {
    let floating_point = br.bool()?;
    if floating_point {
        let bits_per_sample = br.u32([
            U32::Val(32),
            U32::Val(16),
            U32::Val(24),
            U32::BitsOffset(6, 1),
        ])?;
        let exponent_bits = br.bits(4)? + 1;
        // libjxl BitDepth::VisitFields bounds.
        if !(2..=8).contains(&exponent_bits) {
            return Err(HeaderError::Invalid(format!(
                "exponent_bits_per_sample {exponent_bits} outside 2..=8"
            )));
        }
        let mantissa = i64::from(bits_per_sample) - i64::from(exponent_bits) - 1;
        if !(2..=23).contains(&mantissa) {
            return Err(HeaderError::Invalid(format!(
                "float bits_per_sample {bits_per_sample} leaves {mantissa} mantissa bits"
            )));
        }
        Ok(BitDepth {
            floating_point,
            bits_per_sample,
            exponent_bits,
        })
    } else {
        let bits_per_sample = br.u32([
            U32::Val(8),
            U32::Val(10),
            U32::Val(12),
            U32::BitsOffset(6, 1),
        ])?;
        if bits_per_sample > 31 {
            return Err(HeaderError::Invalid(format!(
                "bits_per_sample {bits_per_sample} exceeds 31"
            )));
        }
        Ok(BitDepth {
            floating_point,
            bits_per_sample,
            exponent_bits: 0,
        })
    }
}

fn read_extra_channel(br: &mut BitReader) -> Result<ExtraChannel> {
    let all_default = br.bool()?;
    if all_default {
        return Ok(ExtraChannel {
            kind: ExtraChannelType::Alpha,
            bit_depth: BitDepth::default(),
            dim_shift: 0,
            name: String::new(),
            alpha_premultiplied: false,
            spot_color: None,
            cfa_channel: None,
        });
    }
    let kind = ExtraChannelType::from_raw(br.enum_value()?);
    let bit_depth = read_bit_depth(br)?;
    let dim_shift = br.u32([U32::Val(0), U32::Val(3), U32::Val(4), U32::BitsOffset(3, 1)])?;
    let name = br.string()?;
    // libjxl ExtraChannelInfo::VisitFields rejects these.
    if dim_shift > 3 {
        return Err(HeaderError::Invalid(format!(
            "extra channel dim_shift {dim_shift} too large"
        )));
    }
    if matches!(
        kind,
        ExtraChannelType::Reserved(_) | ExtraChannelType::Unknown | ExtraChannelType::Other(_)
    ) {
        return Err(HeaderError::Invalid(format!(
            "unknown extra channel type {}",
            kind.name()
        )));
    }
    let alpha_premultiplied = if kind == ExtraChannelType::Alpha {
        br.bool()?
    } else {
        false
    };
    let spot_color = if kind == ExtraChannelType::SpotColor {
        Some([br.f16()?, br.f16()?, br.f16()?, br.f16()?])
    } else {
        None
    };
    let cfa_channel = if kind == ExtraChannelType::Cfa {
        Some(br.u32([
            U32::Val(1),
            U32::Bits(2),
            U32::BitsOffset(4, 3),
            U32::BitsOffset(8, 19),
        ])?)
    } else {
        None
    };
    Ok(ExtraChannel {
        kind,
        bit_depth,
        dim_shift,
        name,
        alpha_premultiplied,
        spot_color,
        cfa_channel,
    })
}

const CUSTOM_XY: [U32; 4] = [
    U32::Bits(19),
    U32::BitsOffset(19, 524_288),
    U32::BitsOffset(20, 1_048_576),
    U32::BitsOffset(21, 2_097_152),
];

fn read_custom_xy(br: &mut BitReader) -> Result<(f64, f64)> {
    let x = unpack_signed(br.u32(CUSTOM_XY)?);
    let y = unpack_signed(br.u32(CUSTOM_XY)?);
    Ok((f64::from(x) / 1e6, f64::from(y) / 1e6))
}

fn read_color_encoding(br: &mut BitReader) -> Result<ColorEncoding> {
    let all_default = br.bool()?;
    if all_default {
        return Ok(ColorEncoding::default());
    }
    let want_icc = br.bool()?;
    let color_space = ColorSpace::from_raw(br.enum_value()?);
    let mut enc = ColorEncoding {
        want_icc,
        color_space,
        ..ColorEncoding::default()
    };
    if !want_icc && color_space != ColorSpace::Xyb {
        enc.white_point = match br.enum_value()? {
            1 => WhitePoint::D65,
            2 => {
                let (x, y) = read_custom_xy(br)?;
                WhitePoint::Custom(x, y)
            }
            10 => WhitePoint::E,
            11 => WhitePoint::Dci,
            v => WhitePoint::Other(v),
        };
    }
    if !want_icc && color_space != ColorSpace::Xyb && color_space != ColorSpace::Gray {
        enc.primaries = match br.enum_value()? {
            1 => Primaries::Srgb,
            2 => {
                let r = read_custom_xy(br)?;
                let g = read_custom_xy(br)?;
                let b = read_custom_xy(br)?;
                Primaries::Custom([r, g, b])
            }
            9 => Primaries::Bt2100,
            11 => Primaries::P3,
            v => Primaries::Other(v),
        };
    }
    if !want_icc {
        if color_space != ColorSpace::Xyb {
            let have_gamma = br.bool()?;
            enc.transfer_function = if have_gamma {
                let raw = br.bits(24)?;
                // libjxl: gamma is stored times 1e7 and must lie in (0, 1].
                if raw == 0 || raw > 10_000_000 {
                    return Err(HeaderError::Invalid(format!("invalid gamma {raw}")));
                }
                TransferFunction::Gamma(f64::from(raw) / 1e7)
            } else {
                TransferFunction::from_raw(br.enum_value()?)
            };
        }
        enc.rendering_intent = RenderingIntent::from_raw(br.enum_value()?);
    }
    Ok(enc)
}

fn read_tone_mapping(br: &mut BitReader) -> Result<ToneMapping> {
    let all_default = br.bool()?;
    if all_default {
        return Ok(ToneMapping::default());
    }
    let tm = ToneMapping {
        intensity_target: br.f16()?,
        min_nits: br.f16()?,
        relative_to_max_display: br.bool()?,
        linear_below: br.f16()?,
    };
    // libjxl ToneMapping::VisitFields bounds.
    if tm.intensity_target <= 0.0 {
        return Err(HeaderError::Invalid(
            "intensity_target must be positive".into(),
        ));
    }
    if tm.min_nits < 0.0 || tm.min_nits > tm.intensity_target {
        return Err(HeaderError::Invalid(format!(
            "min_nits {} outside 0..=intensity_target {}",
            tm.min_nits, tm.intensity_target
        )));
    }
    if tm.linear_below < 0.0 || (tm.relative_to_max_display && tm.linear_below > 1.0) {
        return Err(HeaderError::Invalid(format!(
            "invalid linear_below {}",
            tm.linear_below
        )));
    }
    Ok(tm)
}

fn read_animation(br: &mut BitReader) -> Result<Animation> {
    Ok(Animation {
        tps_numerator: br.u32([
            U32::Val(100),
            U32::Val(1000),
            U32::BitsOffset(10, 1),
            U32::BitsOffset(30, 1),
        ])?,
        tps_denominator: br.u32([
            U32::Val(1),
            U32::Val(1001),
            U32::BitsOffset(8, 1),
            U32::BitsOffset(10, 1),
        ])?,
        num_loops: br.u32([U32::Val(0), U32::Bits(3), U32::Bits(16), U32::Bits(32)])?,
        have_timecodes: br.bool()?,
    })
}

/// ImageMetadata.
fn read_image_metadata(br: &mut BitReader, width: u32, height: u32) -> Result<ImageInfo> {
    let mut info = ImageInfo {
        width,
        height,
        orientation: 1,
        intrinsic_size: None,
        preview: None,
        animation: None,
        bit_depth: BitDepth::default(),
        modular_16bit_sufficient: true,
        extra_channels: Vec::new(),
        xyb_encoded: true,
        color: ColorEncoding::default(),
        tone_mapping: ToneMapping::default(),
        extensions: 0,
    };
    let all_default = br.bool()?;
    if all_default {
        return Ok(info);
    }
    let extra_fields = br.bool()?;
    if extra_fields {
        info.orientation = br.bits(3)? + 1;
        if br.bool()? {
            info.intrinsic_size = Some(read_size(br)?);
        }
        if br.bool()? {
            info.preview = Some(read_preview(br)?);
        }
        if br.bool()? {
            info.animation = Some(read_animation(br)?);
        }
    }
    info.bit_depth = read_bit_depth(br)?;
    info.modular_16bit_sufficient = br.bool()?;
    let num_extra = br.u32([
        U32::Val(0),
        U32::Val(1),
        U32::BitsOffset(4, 2),
        U32::BitsOffset(12, 1),
    ])?;
    if num_extra > 256 {
        return Err(HeaderError::Invalid(format!(
            "{num_extra} extra channels (limit 256)"
        )));
    }
    for _ in 0..num_extra {
        info.extra_channels.push(read_extra_channel(br)?);
    }
    info.xyb_encoded = br.bool()?;
    info.color = read_color_encoding(br)?;
    if extra_fields {
        info.tone_mapping = read_tone_mapping(br)?;
    }
    info.extensions = br.extensions()?;
    Ok(info)
}

/// CustomTransformData: only skipped over, nothing in it is reported.
fn read_custom_transform_data(br: &mut BitReader, xyb_encoded: bool) -> Result<()> {
    let all_default = br.bool()?;
    if all_default {
        return Ok(());
    }
    if xyb_encoded {
        // OpsinInverseMatrix: 9 + 3 + 4 half floats unless all_default.
        let all_default = br.bool()?;
        if !all_default {
            br.skip(16 * 16)?;
        }
    }
    let mask = br.bits(3)?;
    if mask & 1 != 0 {
        br.skip(15 * 16)?;
    }
    if mask & 2 != 0 {
        br.skip(55 * 16)?;
    }
    if mask & 4 != 0 {
        br.skip(210 * 16)?;
    }
    Ok(())
}

/// The first three fields of the first FrameHeader. Frames start on a byte
/// boundary after the image headers (and ICC profile, when present).
fn read_first_frame(br: &mut BitReader, has_preview: bool) -> Result<FrameInfo> {
    br.jump_to_byte_boundary()?;
    let all_default = br.bool()?;
    if all_default {
        return Ok(FrameInfo {
            is_preview: has_preview,
            frame_type: FrameType::Regular,
            encoding: FrameEncoding::VarDct,
        });
    }
    let frame_type = match br.bits(2)? {
        0 => FrameType::Regular,
        1 => FrameType::Lf,
        2 => FrameType::ReferenceOnly,
        _ => FrameType::SkipProgressive,
    };
    let encoding = if br.bits(1)? == 0 {
        FrameEncoding::VarDct
    } else {
        FrameEncoding::Modular
    };
    Ok(FrameInfo {
        is_preview: has_preview,
        frame_type,
        encoding,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LSB-first writer for hand-built headers.
    #[derive(Default)]
    struct W {
        bytes: Vec<u8>,
        nbits: u64,
    }

    impl W {
        fn bits(&mut self, n: u32, value: u64) -> &mut Self {
            for i in 0..n {
                let bit = ((value >> i) & 1) as u8;
                if self.nbits.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                let last = self.bytes.len() - 1;
                self.bytes[last] |= bit << (self.nbits % 8);
                self.nbits += 1;
            }
            self
        }
        fn u32_sel(&mut self, selector: u64, n: u32, value: u64) -> &mut Self {
            self.bits(2, selector);
            if n > 0 {
                self.bits(n, value);
            }
            self
        }
        fn pad(&mut self) -> &mut Self {
            while !self.nbits.is_multiple_of(8) {
                self.bits(1, 0);
            }
            self
        }
        fn finish(&self) -> Vec<u8> {
            self.bytes.clone()
        }
    }

    fn signature(w: &mut W) {
        w.bits(8, 0xff).bits(8, 0x0a);
    }

    #[test]
    fn minimal_all_default_header() {
        // 64x64 small size, ratio 1:1, all_default metadata, all_default
        // transform data, then an all_default frame header.
        let mut w = W::default();
        signature(&mut w);
        w.bits(1, 1) // small
            .bits(5, 7) // ysize_div8 - 1 = 7 -> 64
            .bits(3, 1) // ratio 1:1
            .bits(1, 1) // metadata all_default
            .bits(1, 1) // transform all_default
            .pad()
            .bits(1, 1); // frame all_default
        let cs = parse(&w.finish()).unwrap();
        assert_eq!((cs.image.width, cs.image.height), (64, 64));
        assert!(cs.image.xyb_encoded);
        assert_eq!(cs.image.bit_depth.bits_per_sample, 8);
        assert!(cs.image.color.is_srgb());
        let frame = cs.first_frame.unwrap();
        assert_eq!(frame.encoding, FrameEncoding::VarDct);
        assert_eq!(frame.frame_type, FrameType::Regular);
    }

    #[test]
    fn size_ratio_table() {
        assert_eq!(ratio_to_xsize(480, 3), Some(640));
        assert_eq!(ratio_to_xsize(200, 4), Some(300));
        assert_eq!(ratio_to_xsize(1080, 5), Some(1920));
        assert_eq!(ratio_to_xsize(100, 0), None);
    }

    #[test]
    fn orientation_and_animation_fields() {
        let mut w = W::default();
        signature(&mut w);
        // Large size: 1000x500 -> not small; ysize-1 = 499 fits Bits(9).
        w.bits(1, 0).u32_sel(0, 9, 499).bits(3, 7); // ratio 2:1 -> xsize 1000
        w.bits(1, 0) // metadata not all_default
            .bits(1, 1) // extra_fields
            .bits(3, 5) // orientation 6 (rotate 90 CW)
            .bits(1, 0) // no intrinsic size
            .bits(1, 0) // no preview
            .bits(1, 1) // animation
            .u32_sel(1, 0, 0) // tps_numerator = 1000
            .u32_sel(0, 0, 0) // tps_denominator = 1
            .u32_sel(1, 3, 2) // num_loops = 2
            .bits(1, 0); // no timecodes
        w.bits(1, 0) // bit depth: integer
            .u32_sel(0, 0, 0); // 8 bits
        w.bits(1, 1); // modular_16bit_sufficient
        w.u32_sel(1, 0, 0); // one extra channel
        w.bits(1, 1); // extra channel all_default (alpha, 8 bit)
        w.bits(1, 0); // xyb_encoded false
        w.bits(1, 1); // color all_default
        w.bits(1, 1); // tone mapping all_default
        w.u32_sel(0, 0, 0); // extensions = 0 (U64 selector 0)
        w.bits(1, 1); // transform all_default
        w.pad();
        w.bits(1, 0).bits(2, 0).bits(1, 1); // frame: regular, modular
        let cs = parse(&w.finish()).unwrap();
        let img = &cs.image;
        assert_eq!((img.width, img.height), (1000, 500));
        assert_eq!(img.orientation, 6);
        assert_eq!(img.displayed_size(), (500, 1000));
        let anim = img.animation.as_ref().unwrap();
        assert_eq!((anim.tps_numerator, anim.tps_denominator), (1000, 1));
        assert_eq!(anim.num_loops, 2);
        assert_eq!(img.extra_channels.len(), 1);
        assert_eq!(img.alpha().unwrap().kind, ExtraChannelType::Alpha);
        assert!(!img.xyb_encoded);
        assert_eq!(cs.first_frame.unwrap().encoding, FrameEncoding::Modular);
    }

    #[test]
    fn icc_blocks_frame_inspection() {
        let mut w = W::default();
        signature(&mut w);
        w.bits(1, 1).bits(5, 0).bits(3, 1); // 8x8
        w.bits(1, 0) // not all_default
            .bits(1, 0) // no extra fields
            .bits(1, 0)
            .u32_sel(0, 0, 0) // 8-bit
            .bits(1, 1) // modular_16bit_sufficient
            .u32_sel(0, 0, 0) // no extra channels
            .bits(1, 1) // xyb
            .bits(1, 0) // color not all_default
            .bits(1, 1) // want_icc
            .u32_sel(0, 0, 0) // color space RGB
            .u32_sel(0, 0, 0) // extensions
            .bits(1, 1); // transform all_default
        let cs = parse(&w.finish()).unwrap();
        assert!(cs.image.color.want_icc);
        assert!(cs.first_frame.is_none());
        assert!(cs.frame_note.unwrap().contains("ICC"));
    }

    #[test]
    fn rejects_non_codestream() {
        assert!(parse(b"\x89PNG").is_err());
        assert!(parse(&[0xff, 0x0a]).is_err());
    }
}
