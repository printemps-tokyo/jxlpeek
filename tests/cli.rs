//! End-to-end tests: run the built binary against the fixtures in
//! `tests/fixtures` (encoded from synthetic images: the original set with
//! cjxl 0.11.1; `frames_*` and `cmp_*` with cjxl 0.12.0, avifenc 1.4.2 (aom
//! 3.15.2), cwebp and img2webp 1.6.0).

use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_jxlpeek");

fn fixture(name: &str) -> String {
    let p: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect();
    p.to_str().unwrap().to_string()
}

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(["--no-color"])
        .args(args)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn lossy_bare_codestream_passes() {
    let o = run(&[&fixture("lossy.jxl")]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.contains("bare codestream"), "{s}");
    assert!(s.contains("640x480, 16-bit RGB"), "{s}");
    assert!(s.contains("lossy (XYB color space, VarDCT frame)"), "{s}");
    assert!(s.contains("verdict      PASS"), "{s}");
}

#[test]
fn lossless_8bit_fits_level_5() {
    let s = stdout(&run(&[&fixture("lossless.jxl")]));
    assert!(s.contains("64x48, 8-bit RGB"), "{s}");
    assert!(s.contains("modular frame"), "{s}");
    assert!(s.contains("PASS level"), "{s}");
}

#[test]
fn jpeg_transcode_with_metadata_and_orientation() {
    let o = run(&[&fixture("exif6.jxl")]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.contains("brob(Exif) brob(xml)"), "{s}");
    assert!(s.contains("2 jxlp parts"), "{s}");
    assert!(s.contains("orientation 6: rotate 90 CW"), "{s}");
    assert!(s.contains("480x640 displayed"), "{s}");
    assert!(s.contains("reversible via jbrd"), "{s}");
    assert!(s.contains("INFO metadata"), "{s}");
    assert!(s.contains("INFO jpeg-reconstruction"), "{s}");
}

#[test]
fn metadata_budget_turns_into_warning_and_strict_fails() {
    let o = run(&[&fixture("exif6_raw.jxl"), "--max-metadata", "1000"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("WARN metadata"), "{}", stdout(&o));

    let o = run(&[
        &fixture("exif6_raw.jxl"),
        "--max-metadata",
        "1000",
        "--strict",
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("verdict      FAIL"), "{}", stdout(&o));
}

#[test]
fn budgets_fail_with_exit_code_1() {
    let o = run(&[&fixture("lossy.jxl"), "--max-bytes", "1000"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("FAIL bytes"), "{}", stdout(&o));

    let o = run(&[&fixture("lossy.jxl"), "--max-pixels", "100000"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("FAIL pixels"), "{}", stdout(&o));

    let o = run(&[
        &fixture("lossy.jxl"),
        "--max-bytes",
        "4000",
        "--max-pixels",
        "400000",
    ]);
    assert!(o.status.success());
}

#[test]
fn animation_alpha_gray_hdr_and_custom_color() {
    let s = stdout(&run(&[&fixture("anim.jxl")]));
    assert!(
        s.contains("animation    100/1 ticks per second, loops forever"),
        "{s}"
    );
    assert!(s.contains("alpha 8-bit"), "{s}");
    assert!(s.contains("modular frame"), "{s}");

    let s = stdout(&run(&[&fixture("alpha.jxl")]));
    assert!(s.contains("RGB+alpha"), "{s}");
    assert!(s.contains("straight (non-premultiplied)"), "{s}");

    let s = stdout(&run(&[&fixture("gray.jxl")]));
    assert!(s.contains("16-bit grayscale"), "{s}");
    assert!(s.contains("grayscale, D65, sRGB transfer"), "{s}");

    let s = stdout(&run(&[&fixture("hdr.jxl")]));
    assert!(s.contains("intensity target 1000 nits (HDR)"), "{s}");
    assert!(s.contains("INFO hdr"), "{s}");

    let s = stdout(&run(&[&fixture("custom_primaries.jxl")]));
    assert!(s.contains("custom primaries r(0.6300, 0.3400)"), "{s}");
    assert!(s.contains("gamma 0.5553 transfer"), "{s}");
    assert!(s.contains("perceptual intent"), "{s}");
    assert!(s.contains("INFO color"), "{s}");
}

#[test]
fn level_10_requirement_is_explained() {
    // cjxl writes jxll = 10 for 16-bit lossless because modular buffers
    // no longer fit 16 bits; the tool must say so rather than call it a
    // gratuitous declaration.
    let o = run(&[&fixture("sixteen.jxl")]);
    assert!(o.status.success());
    let s = stdout(&o);
    assert!(s.contains("level 10"), "{s}");
    assert!(s.contains("WARN level"), "{s}");
    assert!(s.contains("modular_16bit_sufficient = false"), "{s}");

    let o = run(&[&fixture("sixteen.jxl"), "--strict"]);
    assert_eq!(o.status.code(), Some(1));

    let s = stdout(&run(&[&fixture("lossy_container.jxl")]));
    assert!(s.contains("PASS level                no jxll box"), "{s}");
}

#[test]
fn boxes_listing() {
    let s = stdout(&run(&[&fixture("jpegrecon.jxl"), "--boxes"]));
    assert!(s.contains("'jxlp'  part 0"), "{s}");
    assert!(s.contains("part 1 (last)"), "{s}");
    assert!(s.contains("'jbrd'"), "{s}");
}

#[test]
fn json_output_shape() {
    let o = run(&[&fixture("exif6.jxl"), &fixture("lossy.jxl"), "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.starts_with("{\"jxlpeek\":\""), "{s}");
    assert!(s.contains("\"format\":\"container\""), "{s}");
    assert!(s.contains("\"format\":\"codestream\""), "{s}");
    assert!(s.contains("\"orientation\":6"), "{s}");
    assert!(s.contains("\"displayed_width\":480"), "{s}");
    assert!(
        s.contains("\"coding\":\"jpeg-transcode-reversible\""),
        "{s}"
    );
    assert!(s.contains("\"inner_type\":\"Exif\""), "{s}");
    assert!(s.contains("\"jxlp_last\":true"), "{s}");
    assert!(s.contains("\"is_srgb\":true"), "{s}");
    assert!(s.ends_with("\"verdict\":\"info\"}\n"), "{s}");
    // Balanced brackets: a cheap check that no object is left open.
    assert_eq!(s.matches('{').count(), s.matches('}').count());
    assert_eq!(s.matches('[').count(), s.matches(']').count());
}

#[test]
fn non_jxl_and_broken_files() {
    let dir = std::env::temp_dir().join(format!("jxlpeek-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("not.jxl");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
    let o = run(&[png.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("not a JPEG XL file"), "{}", stderr(&o));

    // Truncated container: the jxlc box claims more bytes than exist.
    let mut data = std::fs::read(fixture("lossy_container.jxl")).unwrap();
    data.truncate(200);
    let cut = dir.join("cut.jxl");
    std::fs::write(&cut, &data).unwrap();
    let o = run(&[cut.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stdout(&o).contains("FAIL structure"), "{}", stdout(&o));

    // JSON keeps going after an unreadable file and reports an error verdict.
    let o = run(&[png.to_str().unwrap(), &fixture("lossy.jxl"), "--json"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stdout(&o).contains("\"verdict\":\"error\"}\n"),
        "{}",
        stdout(&o)
    );

    let missing = dir.join("missing.jxl");
    let o = run(&[missing.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn frame_list_matches_jxlinfo() {
    // jxlinfo -v: three frames of 200 ms.
    let o = run(&["--frames", &fixture("anim.jxl")]);
    let s = stdout(&o);
    assert!(
        s.contains("frames       3 displayed of 3 coded, 0.600 s"),
        "{s}"
    );
    assert_eq!(s.matches("200 ms").count(), 3, "{s}");
    assert!(s.contains("PASS frames"), "{s}");
    assert!(
        s.contains("animated: 3 displayed frames, 0.600 s per loop"),
        "{s}"
    );

    let s = stdout(&run(&["--json", &fixture("anim.jxl")]));
    assert!(s.contains("\"complete\":true"), "{s}");
    assert!(s.contains("\"total_ms\":600"), "{s}");
    assert!(s.contains("\"duration_ms\":200"), "{s}");
}

#[test]
fn helper_frames_do_not_decide_the_coding() {
    // Patches: a reference-only modular frame, then the VarDCT image.
    let s = stdout(&run(&["--frames", &fixture("frames_patches.jxl")]));
    assert!(s.contains("1 displayed of 2 coded"), "{s}");
    assert!(s.contains("reference-only"), "{s}");
    assert!(s.contains("saved as reference 3"), "{s}");
    assert!(s.contains("lossy (XYB color space, VarDCT frame)"), "{s}");

    // --progressive_dc=1: an LF frame precedes the main frame.
    let s = stdout(&run(&["--frames", &fixture("frames_lf.jxl")]));
    assert!(s.contains("LF level 1"), "{s}");
    assert!(s.contains("lossy (XYB color space, VarDCT frame)"), "{s}");
    assert!(s.contains("PASS frames"), "{s}");
}

#[test]
fn permuted_toc_is_reported_not_guessed() {
    let s = stdout(&run(&["--frames", &fixture("frames_permuted.jxl")]));
    assert!(s.contains("TOC is permuted"), "{s}");
    assert!(!s.contains("PASS frames"), "{s}");
    let s = stdout(&run(&["--json", &fixture("frames_permuted.jxl")]));
    assert!(s.contains("\"toc_permuted\":true"), "{s}");
    assert!(s.contains("\"data_bytes\":null"), "{s}");
}

#[test]
fn truncated_animation_warns() {
    let dir = std::env::temp_dir().join(format!("jxlpeek-frames-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let data = std::fs::read(fixture("cmp_anim.jxl")).unwrap();
    let cut = dir.join("cut.jxl");
    std::fs::write(&cut, &data[..data.len() - 100]).unwrap();
    let o = run(&[cut.to_str().unwrap()]);
    let s = stdout(&o);
    assert!(s.contains("WARN frames"), "{s}");
    assert!(s.contains("at least"), "{s}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compare_lines_up_the_three_formats() {
    let o = run(&[
        "--compare",
        &fixture("cmp_anim.jxl"),
        &fixture("cmp_anim.avif"),
        &fixture("cmp_anim.webp"),
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    let row = |label: &str| {
        s.lines()
            .find(|l| l.starts_with(label))
            .unwrap_or_else(|| panic!("no {label} row in {s}"))
            .to_string()
    };
    assert!(row("format").contains("JPEG XL"), "{s}");
    assert!(row("format").contains("AVIF sequence"), "{s}");
    assert!(row("format").contains("WebP animation"), "{s}");
    assert_eq!(row("dimensions").matches("48x36").count(), 3, "{s}");
    assert_eq!(row("frames").matches('3').count(), 3, "{s}");
    assert_eq!(row("duration").matches("0.300 s").count(), 3, "{s}");
    // avifdec: "Repeat Count: Infinite"; webpmux: "Loop Count : 3".
    assert!(row("loops").contains("forever"), "{s}");
    assert!(row("loops").contains("loop count 3"), "{s}");
    // webpmux: two lossy frames and one lossless.
    assert!(
        row("coding").contains("mixed lossy (VP8) and lossless (VP8L)"),
        "{s}"
    );
    assert!(
        row("coding").contains("AV1 High profile, level 2.0, 4:4:4"),
        "{s}"
    );
    assert!(row("vs smallest").contains("1.00x"), "{s}");
}

#[test]
fn compare_json_and_errors() {
    let o = run(&[
        "--compare",
        "--json",
        &fixture("cmp_still.avif"),
        &fixture("cmp_still.webp"),
        &fixture("cmp_still.jxl"),
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.starts_with("{\"jxlpeek\":"), "{s}");
    assert_eq!(s.matches("\"width\":48").count(), 3, "{s}");
    assert!(s.contains("\"format\":\"AVIF\""), "{s}");
    assert!(s.contains("\"bytes\":326"), "{s}");
    assert!(s.contains("\"verdict\":\"ok\""), "{s}");

    let dir = std::env::temp_dir().join(format!("jxlpeek-cmp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("x.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
    let o = run(&[
        "--compare",
        png.to_str().unwrap(),
        &fixture("cmp_still.webp"),
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("not a JPEG XL, AVIF or WebP file"),
        "{}",
        stderr(&o)
    );
    assert!(stdout(&o).contains("WebP"), "{}", stdout(&o));

    // Without --compare, AVIF is still not accepted.
    let o = run(&[&fixture("cmp_still.avif")]);
    assert_eq!(o.status.code(), Some(2));
    // --compare does not combine with the check flags.
    let o = run(&["--compare", "--strict", &fixture("cmp_still.jxl")]);
    assert_eq!(o.status.code(), Some(2));
    std::fs::remove_dir_all(&dir).unwrap();
}
