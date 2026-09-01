# jxlpeek

Inspect JPEG XL files without decoding them.

jxlpeek reads what a decoder reads before the first pixel -- the container
boxes and the codestream headers -- and prints it in plain language: is this a
bare codestream or a container, what level does it declare, is it lossy XYB or
a reversible JPEG transcode, what are the stored and displayed dimensions,
does it carry an alpha channel, an animation, an HDR intensity target, how many
bytes go to Exif/XMP and JPEG reconstruction data. Then it runs delivery checks
and turns them into an exit code.

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
  tone         intensity target 255 nits (SDR)
  checks
    PASS structure            container well-formed (7 boxes)
    PASS level                no jxll box; headers fit level 5
    INFO metadata             274 bytes of metadata (7.1% of file, brotli): Exif 84 bytes, XMP 190 bytes; strip for web delivery unless needed
    INFO jpeg-reconstruction  jbrd 479 bytes keeps the original JPEG reconstructible; decoders ignore it for display (cjxl --allow_jpeg_reconstruction=0 drops it)
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
  tone         intensity target 255 nits (SDR)
  checks
    PASS structure            bare codestream (no container); metadata boxes cannot be present
    PASS level                no jxll box; headers fit level 5
    PASS coding               lossy (XYB color space, VarDCT frame)
  verdict      PASS
```

Several files at once, box table included:

```sh
jxlpeek --boxes assets/*.jxl
```

### Checks and exit codes

| check | what it looks at | levels |
| --- | --- | --- |
| `structure` | signature box, `ftyp` second with brand `jxl `, one `jxlc` or an ordered `jxlp` chain with the last part flagged, box sizes inside the file | PASS / FAIL |
| `level` | declared `jxll` level against what the headers need (level 5 limits: 262144 px per side, 2^28 px total, 16-bit samples, modular buffers within 16 bits, 4 extra channels, no CMYK) | PASS / INFO / WARN / FAIL |
| `metadata` | bytes in Exif, XMP (`xml `) and JUMBF boxes, Brotli-wrapped (`brob`) or not | PASS / INFO / WARN over `--max-metadata` (default 16384) |
| `jpeg-reconstruction` | `jbrd` box present | INFO |
| `coding` | XYB or original colors, VarDCT or modular first frame | PASS / INFO when the frame header is unreachable |
| `color` | non-sRGB encodings and embedded ICC profiles | INFO |
| `orientation` | orientation other than 1, stored versus displayed size | INFO |
| `hdr` | PQ/HLG transfer or intensity target above 255 nits | INFO |
| `animation`, `preview`, `alpha` | presence and parameters | INFO |
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
`first_frame`, `coding` (`lossy-xyb`, `modular-original`,
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
   coders. Then it aligns to the byte boundary and reads the first three
   fields of the first FrameHeader to learn whether the frame is VarDCT or
   modular.
4. When the color encoding says an ICC profile follows, the profile is
   entropy-coded and cannot be skipped without a real decoder, so the frame
   header is reported as not inspected rather than guessed.

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

- It does not decode pixels, so it cannot count animation frames, measure
  quality, or read an embedded ICC profile.
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
- Mozilla, "Intent to Ship: JPEG XL", 2026-08-24; Chromium blink-dev,
  "Intent to Ship: JPEG XL decoding support (image/jxl) in blink", 2026-08-24.

## License

MIT
