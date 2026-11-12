//! Tie the container walker and the codestream parser together into one
//! report per file.

use crate::codestream::{self, Codestream, FrameEncoding};
use crate::container::{self, Container};
use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub enum InspectError {
    Io(std::io::Error),
    /// Neither signature matched.
    NotJxl,
    /// The container has no usable codestream.
    NoCodestream(Vec<String>),
    HeaderOnly(crate::bits::HeaderError),
}

impl fmt::Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InspectError::Io(e) => write!(f, "{e}"),
            InspectError::NotJxl => write!(
                f,
                "not a JPEG XL file (expected FF 0A or the 12-byte signature box)"
            ),
            InspectError::NoCodestream(issues) => {
                write!(f, "container holds no codestream: {}", issues.join("; "))
            }
            InspectError::HeaderOnly(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InspectError {}

impl From<std::io::Error> for InspectError {
    fn from(e: std::io::Error) -> Self {
        InspectError::Io(e)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub path: String,
    pub file_len: u64,
    /// `None` for a bare codestream.
    pub container: Option<Container>,
    pub codestream_len: u64,
    pub codestream: Codestream,
}

/// How the pixels are coded, as far as the headers tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coding {
    /// XYB color space, VarDCT frame: the normal lossy mode.
    LossyXyb,
    /// XYB color space, modular frame (lossy or near-lossless modular).
    ModularXyb,
    /// Original colors, modular frame: lossless unless the encoder quantized.
    ModularOriginal,
    /// Original colors, VarDCT frame with reconstruction data: a reversible
    /// JPEG transcode.
    JpegTranscodeReversible,
    /// Original colors, VarDCT frame, no reconstruction data: JPEG DCT
    /// coefficients kept, the original JPEG bytes not recoverable.
    JpegTranscode,
    /// Only the color encoding is known (frame header not reachable, or the
    /// first frame is a preview).
    XybOnly,
    OriginalOnly,
}

impl Coding {
    pub fn id(&self) -> &'static str {
        match self {
            Coding::LossyXyb => "lossy-xyb",
            Coding::ModularXyb => "modular-xyb",
            Coding::ModularOriginal => "modular-original",
            Coding::JpegTranscodeReversible => "jpeg-transcode-reversible",
            Coding::JpegTranscode => "jpeg-transcode",
            Coding::XybOnly => "xyb",
            Coding::OriginalOnly => "original",
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Coding::LossyXyb => "lossy (XYB color space, VarDCT frame)",
            Coding::ModularXyb => "XYB color space, modular frame",
            Coding::ModularOriginal => {
                "original colors kept, modular frame (lossless unless the encoder quantized)"
            }
            Coding::JpegTranscodeReversible => {
                "JPEG transcode: original DCT coefficients, reversible via jbrd"
            }
            Coding::JpegTranscode => {
                "original colors kept, VarDCT frame (JPEG transcode without reconstruction data)"
            }
            Coding::XybOnly => "lossy (XYB color space); frame type not inspected",
            Coding::OriginalOnly => "original colors kept; frame type not inspected",
        }
    }
}

impl Report {
    pub fn is_container(&self) -> bool {
        self.container.is_some()
    }

    pub fn coding(&self) -> Coding {
        let xyb = self.codestream.image.xyb_encoded;
        let jbrd = self.container.as_ref().and_then(|c| c.jbrd_len).is_some();
        // A preview frame says nothing about how the main image is coded.
        // A preview, an LF frame or a patch source says nothing about how the
        // displayed image is coded, so prefer the first displayed frame.
        let frames = &self.codestream.frames;
        let main_frame = frames
            .frames
            .iter()
            .find(|f| f.is_displayed())
            .map(|f| f.encoding)
            .or_else(|| {
                self.codestream
                    .first_frame
                    .filter(|f| !f.is_preview)
                    .map(|f| f.encoding)
            })
            .or_else(|| frames.main_frame().map(|f| f.encoding));
        match main_frame {
            Some(FrameEncoding::VarDct) if xyb => Coding::LossyXyb,
            Some(FrameEncoding::Modular) if xyb => Coding::ModularXyb,
            Some(FrameEncoding::Modular) => Coding::ModularOriginal,
            Some(FrameEncoding::VarDct) if jbrd => Coding::JpegTranscodeReversible,
            Some(FrameEncoding::VarDct) => Coding::JpegTranscode,
            None if xyb => Coding::XybOnly,
            None => Coding::OriginalOnly,
        }
    }
}

/// Inspect in-memory file bytes.
pub fn inspect_bytes(path: &str, data: &[u8]) -> Result<Report, InspectError> {
    let file_len = data.len() as u64;
    if data.len() >= 2 && data[..2] == codestream::SIGNATURE {
        let cs = codestream::parse(data).map_err(InspectError::HeaderOnly)?;
        return Ok(Report {
            path: path.to_string(),
            file_len,
            container: None,
            codestream_len: file_len,
            codestream: cs,
        });
    }
    if data.len() >= 12 && data[..12] == container::SIGNATURE {
        let c = container::parse(data);
        if c.codestream.is_empty() {
            return Err(InspectError::NoCodestream(c.issues));
        }
        let cs = codestream::parse(&c.codestream).map_err(InspectError::HeaderOnly)?;
        let codestream_len = c.codestream.len() as u64;
        return Ok(Report {
            path: path.to_string(),
            file_len,
            container: Some(c),
            codestream_len,
            codestream: cs,
        });
    }
    Err(InspectError::NotJxl)
}

pub fn inspect_path(path: &Path) -> Result<Report, InspectError> {
    let data = std::fs::read(path)?;
    inspect_bytes(&path.display().to_string(), &data)
}
