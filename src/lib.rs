//! jxlpeek: inspect JPEG XL files without decoding them.
//!
//! The crate reads only what a decoder reads before the first pixel: the
//! ISOBMFF container boxes (ISO/IEC 18181-2) and the codestream headers
//! (ISO/IEC 18181-1 SizeHeader, ImageMetadata, and every FrameHeader with
//! its table of contents). Nothing is entropy-decoded, so parsing is
//! O(header size) and needs no libjxl.
//!
//! Field layouts follow the reference implementations (libjxl `fields.h`,
//! `image_metadata.cc`; jxl-rs `headers/*.rs`), which are the normative
//! bitstream in practice. Where this crate cannot go further without a full
//! decoder (an entropy-coded ICC profile precedes the frames), it says so
//! instead of guessing.

pub mod avif;
pub mod bits;
pub mod check;
pub mod codestream;
pub mod compare;
pub mod container;
pub mod frames;
pub mod inspect;
pub mod json;
pub mod report;
pub mod webp;
