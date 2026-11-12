# jxlpeek

Inspect JPEG XL files without decoding them.

jxlpeek reads what a decoder reads before the first pixel -- the container
boxes and the codestream headers -- and prints it in plain language: is this a
bare codestream or a container, what level does it declare, is it lossy XYB or
a reversible JPEG transcode, what are the stored and displayed dimensions,
does it carry an alpha channel, an animation, an HDR intensity target, how many
bytes go to Exif/XMP and JPEG reconstruction data. It walks every frame header
to list the frames of an animation with their durations, crops and blend
modes, and it can put a JPEG XL file next to its AVIF and WebP versions in one
table. Then it runs delivery checks and turns them into an exit code.

No libjxl, no image decoding, no network. Single binary.

## Why

Firefox and Chromium both filed their intent to ship JPEG XL on 2026-08-24, on
the memory-safe jxl-rs decoder, and Mozilla expects cross-browser support
before the end of 2026. Image pipelines are going to start emitting `.jxl`,
and the tooling around them has to answer questions the pixels do not:

- Did the encoder keep the JPEG reconstruction data (`jbrd`) that only matters
  if you want the original JPEG bytes back?
- Did Exif/XMP survive into the container, and how much do they weigh?
- Is the file a lossy XYB encode, a lossless modular encode, or a JPEG
  transcode?
- Does the file declare codestream level 10, and if so, why?
- Which size should go into `width`/`height` when the image is stored rotated?

`jxlinfo` from libjxl answers some of these, but it needs libjxl installed and
does not judge. jxlpeek reads a few hundred bytes, needs nothing installed,
and has a `--strict` mode for CI.

## Install

```sh
cargo install --git https://github.com/printemps-tokyo/jxlpeek
```

Or grab a binary from Releases.

## Usage

```sh
jxlpeek photo.jxl
```

```
photo.jxl  3,880 bytes
  format       ISOBMFF container (ISO/IEC 18181-2), boxes: JXL ftyp jxlp jbrd brob(Exif) brob(xml) jxlp
  codestream   3,047 bytes in 2 jxlp parts (78.5% of file)
  image        640x480 stored, 480x640 displayed (orientation 6: rotate 90 CW), 8-bit RGB
  color        RGB, D65, sRGB primaries, sRGB transfer, relative intent
  coding       JPEG transcode: original DCT coefficients, reversible via jbrd
  first frame  regular frame, VarDCT encoding
  frames       1 displayed of 1 coded
  tone         intensity target 255 nits (SDR)
  checks
    PASS structure            container well-formed (7 boxes)
    PASS level                no jxll box; headers fit level 5
    INFO metadata             274 bytes of metadata (7.1% of file, brotli): Exif 84 bytes, XMP 190 bytes; strip for web delivery unless needed
    INFO jpeg-reconstruction  jbrd 479 bytes keeps the original JPEG reconstructible; decoders ignore it for display (cjxl --allow_jpeg_reconstruction=0 drops it)
    PASS frames               1 frame(s) walked to the last one; their TOC sizes add up to the codestream length
    PASS coding               JPEG transcode: original DCT coefficients, reversible via jbrd
    INFO orientation          orientation 6 (rotate 90 CW): stored 640x480, displayed 480x640; use the displayed size in width/height attributes
  verdict      INFO
```

A lossy encode straight from a PNG is shorter:

```
hero.jxl  3,070 bytes
  format       bare codestream (ISO/IEC 18181-1), no container
  image        640x480, 16-bit RGB
  color        RGB, D65, sRGB primaries, sRGB transfer, relative intent
  coding       lossy (XYB color space, VarDCT frame)
  first frame  regular frame, VarDCT encoding
  frames       1 displayed of 1 coded
  tone         intensity target 255 nits (SDR)
  checks
    PASS structure            bare codestream (no container); metadata boxes cannot be present
    PASS level                no jxll box; headers fit level 5
    PASS frames               1 frame(s) walked to the last one; their TOC sizes add up to the codestream length
    PASS coding               lossy (XYB color space, VarDCT frame)
  verdict      PASS
```

Several files at once, box table included:

```sh
jxlpeek --boxes assets/*.jxl
```

### Frames

`--frames` adds one row per frame. An animation:

```
banner.jxl  2,249 bytes
  format       bare codestream (ISO/IEC 18181-1), no container
  image        48x36, 8-bit RGB
  color        RGB, D65, sRGB primaries, sRGB transfer, perceptual intent
  coding       lossy (XYB color space, VarDCT frame)
  first frame  regular frame, VarDCT encoding
  animation    100/1 ticks per second, loops forever
  frames       3 displayed of 3 coded, 0.300 s
  tone         intensity target 255 nits (SDR)
  frame list
        #  type              coding   size         origin         duration  blend          bytes  notes
        0  regular           VarDCT   48x36        0,0              100 ms  replace          751
        1  regular           VarDCT   48x36        0,0              100 ms  replace          743
        2  regular           VarDCT   48x36        0,0              100 ms  replace          745  last
  checks
    PASS structure            bare codestream (no container); metadata boxes cannot be present
    PASS level                no jxll box; headers fit level 5
    PASS frames               3 frame(s) walked to the last one; their TOC sizes add up to the codestream length
    PASS coding               lossy (XYB color space, VarDCT frame)
    INFO animation            animated: 3 displayed frames, 0.300 s per loop (100/1 ticks per second, loops forever)
  verdict      INFO
```

Still images have frames too. A screenshot encoded with patches stores the
repeated glyphs in a reference-only frame that the main frame copies from:

```
  frame list
        #  type              coding   size         origin         duration  blend          bytes  notes
        0  reference-only    modular  30x30        -                     -  -                745  not shown on its own, saved as reference 3
        1  regular           VarDCT   300x100      0,0                   -  replace        1,909  last
```

"Displayed" follows the decoders' rule (libjxl, jxl-rs `is_visible`): a
regular or skip-progressive frame that is the last one or has a non-zero
duration. Zero-duration frames are composited into the next one, and LF and
reference-only frames are never shown on their own. The `coding` line uses
the first displayed frame, so an LF frame or a patch source in front of it no
longer decides what the file is called. The size of each frame is its header,
table of contents and sections; when the sizes add up to exactly the end of the
codestream, the `frames` check passes.

Two things stop the walk, and jxlpeek says which:

- An ICC profile after the image header is entropy-coded, so the first frame
  header cannot be located without decoding it.
- A permuted TOC (cjxl `--group_order=1`, and progressive encodes) stores the
  section order entropy-coded. The frame header is still read, but its size is
  not, so later frames are not reached. When that frame is the last one, the
  frame count is still complete.

A codestream that ends before its frames say it should is a `frames` warning.

### Comparing with AVIF and WebP

```sh
jxlpeek --compare banner.jxl banner.avif banner.webp
```

```
file         banner.jxl                                                  banner.avif                                                   banner.webp
format       JPEG XL                                                     AVIF sequence                                                 WebP animation
size         2,249 bytes                                                 1,561 bytes                                                   4,190 bytes
vs smallest  1.44x                                                       1.00x                                                         2.68x
dimensions   48x36                                                       48x36                                                         48x36
bit depth    8                                                           8                                                             8
alpha        no                                                          no                                                            no
coding       lossy (XYB color space, VarDCT frame)                       AV1 High profile, level 2.0, 4:4:4                            mixed lossy (VP8) and lossless (VP8L) frames
frames       3                                                           3                                                             3
duration     0.300 s                                                     0.300 s                                                       0.300 s
loops        forever                                                     forever                                                       loop count 3
color        RGB, D65, sRGB primaries, sRGB transfer, perceptual intent  BT.709/sRGB, unspecified transfer, BT.601 matrix, full range  sRGB (no ICC profile; WebP has no other color signaling)
metadata     -                                                           -                                                             -
bits/pixel   3.471                                                       2.409                                                         6.466
bits/pixel is file bits over displayed pixels times frames; headers only, nothing decoded
```

(The test fixtures, encoded from the same three 48x36 frames.) Each column
comes from that format's headers only:

- JPEG XL: everything above, frames from the frame walk.
- AVIF: `ftyp`, the primary item's `ispe`, `pixi`, `av1C`, `colr`, `irot`
  and `imir` properties, alpha from an `auxC` alpha item that references the
  primary through `auxl`, Exif and XMP item sizes from `iloc` (as stored,
  including the Exif item's 4-byte header offset). For an image sequence, the
  `pict` track's sample count, `stts` durations over the `mdhd` timescale, and
  the repetition count derived from `elst` the way libavif does
  (`N repetitions` after the first play, or `forever`, also when the count
  would not fit a 32-bit int).
- WebP: `VP8X` flags and canvas, the `VP8` key frame and `VP8L` headers,
  `ANIM` loop count (0 is forever), `ANMF` frame durations and which frames are
  lossy or lossless, `ICCP`/`EXIF`/`XMP ` chunk sizes.

The loop fields are shown as each format defines them (`num_loops` for JPEG
XL, `loop count` for WebP, repetitions for AVIF), because the three do not
count the same thing. Whether an AV1 image is lossless is decided in the AV1
frame headers (quantizer and segmentation), which jxlpeek does not open, so
the AVIF column never claims it. For an image sequence the column describes
the color track as libavif picks it (the first AV1 track without an `auxl`
reference), from that track's sample entry; alpha means an AV1 track
references it through `auxl` and is not marked as another auxiliary type.
Without a `colr` property the color is in the AV1 sequence header, which is
not read, and the column says so. `--compare --json` prints
`{"jxlpeek", "compare": [...], "verdict"}`; a file that is none of the three
formats is reported and makes the exit code 2. `--compare` does not run the
checks and cannot be combined with `--boxes`, `--frames`, `--strict`,
`--max-bytes` or `--max-pixels`.

### Checks and exit codes

| check | what it looks at | levels |
| --- | --- | --- |
| `structure` | signature box, `ftyp` second with brand `jxl `, one `jxlc` or an ordered `jxlp` chain with the last part flagged, box sizes inside the file | PASS / FAIL |
| `level` | declared `jxll` level against what the headers need (level 5 limits: 262144 px per side, 2^28 px total, 16-bit samples, modular buffers within 16 bits, 4 extra channels, no CMYK) | PASS / INFO / WARN / FAIL |
| `metadata` | bytes in Exif, XMP (`xml `) and JUMBF boxes, Brotli-wrapped (`brob`) or not | PASS / INFO / WARN over `--max-metadata` (default 16384) |
| `jpeg-reconstruction` | `jbrd` box present | INFO |
| `frames` | frame headers and TOC sizes walked to the last frame and the end of the codestream | PASS / INFO for bytes after the last frame / WARN when the codestream ends early or a frame header is unreadable |
| `coding` | XYB or original colors, VarDCT or modular first displayed frame | PASS / INFO when the frame header is unreachable |
| `color` | non-sRGB encodings and embedded ICC profiles | INFO |
| `orientation` | orientation other than 1, stored versus displayed size | INFO |
| `hdr` | PQ/HLG transfer or intensity target above 255 nits | INFO |
| `animation`, `preview`, `alpha` | presence and parameters; the animation's displayed frame count and loop length | INFO |
| `bytes`, `pixels` | `--max-bytes`, `--max-pixels` budgets | PASS / FAIL |

Exit code 0 when nothing failed (warnings allowed), 1 when a check failed or,
with `--strict`, warned, 2 when a file could not be read or is not JPEG XL.

```yaml
- name: JPEG XL assets stay within budget
  run: jxlpeek public/img/*.jxl --max-bytes 300000 --max-metadata 4096 --strict
```

### JSON

```sh
jxlpeek photo.jxl --json | jq '.files[0].image.displayed_width'
```

One document on stdout: `jxlpeek` (version), `files[]` and an overall
`verdict`. Each file entry carries `bytes`, `format` (`container` or
`codestream`), `container` (level, `ftyp` minor version, codestream bytes and
part count, `jbrd`/Exif/XMP/JUMBF byte counts, every box with offset, size,
inner type for `brob`, `jxlp` index and last flag, structural `issues`),
`image` (stored and displayed size, orientation, bit depth, channels, extra
channels, color encoding, tone mapping, animation, extensions mask),
`first_frame`, `frames` (`complete`, `damaged`, `note`, `coded`, `displayed`,
`total_ticks`, `total_ms`, `trailing_bytes`, and `list[]` with each frame's
type, encoding, offset, header and data bytes, TOC entries and whether the TOC
is permuted, crop, upsampling, LF level, passes, blend mode, duration in ticks
and ms, `is_last`, `displayed`, reference slot and name), `coding` (`lossy-xyb`, `modular-original`,
`jpeg-transcode-reversible`, ...), `checks[]` and `verdict`. A file that could
not be inspected becomes `{"path", "error", "verdict": "error"}`.

## How it reads a file

1. Signature: `FF 0A` is a bare codestream, `00 00 00 0C 4A 58 4C 20 0D 0A 87 0A`
   a container (the two magic numbers of the `image/jxl` IANA registration).
2. Container: every box is walked with the ISOBMFF size rules (size 1 =
   64-bit large size, size 0 = to end of file). The codestream is
   reassembled from `jxlc` or the `jxlp` parts. The ordering rules libjxl
   enforces (signature first, `ftyp` second, no `jxlc` after `jxlp`,
   sequential `jxlp` indices for format version 0) are reported as
   structural failures.
3. Codestream: an LSB-first bit reader decodes SizeHeader, ImageMetadata
   (bit depth, extra channels, color encoding with custom white point,
   primaries and gamma, tone mapping, animation, preview, intrinsic size,
   extension skipping) and CustomTransformData, exactly as libjxl
   `fields.cc` / `image_metadata.cc` and jxl-rs `headers/*.rs` define the
   coders.
4. Frames: from the byte boundary after the headers, each FrameHeader is read
   field by field (libjxl `frame_header.cc`, `loop_filter.cc`; jxl-rs
   `frame_header.rs`), the number of TOC entries is derived from the frame
   size, group size, chroma subsampling, upsampling, LF level and pass count
   (libjxl `frame_dimensions.h`, `toc.h`), and the TOC sizes are added up to
   jump to the next frame. Nothing is entropy-decoded.
5. When the color encoding says an ICC profile follows, the profile is
   entropy-coded and cannot be skipped without a real decoder, so the frames
   are reported as not inspected rather than guessed.

Everything reported comes from the headers. In particular "lossless" is
never asserted: a modular frame in the original color space is what lossless
encodes look like, but a modular encode may also be quantized. The report
says "lossless unless the encoder quantized", which is all the headers know.

## Level 5 and level 10

The `jxll` box declares the codestream level. Level 5 is the baseline profile;
level 10 lifts most limits (libjxl `JxlEncoderSetCodestreamLevel`). cjxl
writes `jxll = 10` whenever the image needs it, including a case that
surprises people: 16-bit lossless input sets `modular_16bit_sufficient` to
false, and that alone requires level 10 (libjxl `VerifyLevelSettings`).
jxlpeek explains the reason instead of only printing the number, and warns so
that a pipeline targeting decoders of unknown level notices.

## What it does not do

- It does not decode pixels, so it cannot measure quality or read an
  embedded ICC profile, and it cannot follow a permuted TOC (see Frames).
- `--compare` compares what the headers say; it does not judge visual
  quality, so equal sizes are not equal results.
- It does not parse Exif or XMP contents; it reports their size and whether
  they are Brotli-wrapped.
- Thresholds (`--max-metadata` default, the level 10 warning) are policy
  defaults for web delivery, not format rules. Override them per project.
- It reads files, never URLs. Pair it with `curl` if you need to inspect
  what a CDN serves.

## References

- ISO/IEC 18181-1 (codestream), ISO/IEC 18181-2 (file format), via the
  reference implementations: libjxl `lib/jxl/fields.cc`, `decode.cc`,
  `encode.cc`; jxl-rs `jxl/src/headers/`.
- IANA media type registration for `image/jxl` (magic numbers, 2024-03-04).
- Frame headers and TOC: libjxl `lib/jxl/frame_header.cc`, `loop_filter.cc`,
  `toc.cc`, `frame_dimensions.h` (commit 9e02071); jxl-rs
  `jxl/src/headers/frame_header.rs`, `toc.rs` (commit 87620c4).
- AVIF: ISO/IEC 23008-12 and ISO/IEC 14496-12 boxes as read by libavif
  `src/read.c` (commit 90d3b66), including its repetition-count rule.
- WebP: RFC 9649 (WebP Image Format).
- Mozilla, "Intent to Ship: JPEG XL", 2026-08-24; Chromium blink-dev,
  "Intent to Ship: JPEG XL decoding support (image/jxl) in blink", 2026-08-24.

## License

MIT
