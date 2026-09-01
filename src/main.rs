//! jxlpeek CLI: inspect JPEG XL files and run delivery checks.

use anyhow::Result;
use clap::Parser;
use jxlpeek::check::{self, Level, Options};
use jxlpeek::inspect;
use jxlpeek::json;
use jxlpeek::report::{self, ColorChoice, Style};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "jxlpeek",
    version,
    about = "Inspect JPEG XL files without decoding them: container boxes, codestream headers, delivery checks",
    after_help = "Exit codes: 0 no failures (warnings allowed unless --strict), 1 a check failed, 2 a file could not be read or is not JPEG XL."
)]
struct Cli {
    /// JPEG XL files (.jxl) to inspect
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Also list every container box with offset and size
    #[arg(long)]
    boxes: bool,
    /// Fail when a file is larger than this many bytes
    #[arg(long, value_name = "BYTES")]
    max_bytes: Option<u64>,
    /// Fail when the displayed image has more pixels than this
    #[arg(long, value_name = "PIXELS")]
    max_pixels: Option<u64>,
    /// Warn when Exif/XMP/JUMBF metadata exceeds this many bytes
    #[arg(long, value_name = "BYTES", default_value_t = check::DEFAULT_MAX_METADATA)]
    max_metadata: u64,
    /// Treat warnings as failures
    #[arg(long)]
    strict: bool,
    /// Machine-readable JSON on stdout
    #[arg(long)]
    json: bool,
    /// Disable ANSI colors
    #[arg(long)]
    no_color: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("jxlpeek: error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let style = Style::new(
        if cli.no_color {
            ColorChoice::Never
        } else {
            ColorChoice::Auto
        },
        std::io::stdout().is_terminal(),
    );
    let opts = Options {
        max_bytes: cli.max_bytes,
        max_pixels: cli.max_pixels,
        max_metadata: cli.max_metadata,
        strict: cli.strict,
    };

    let mut worst = Level::Pass;
    let mut had_error = false;
    let mut docs = Vec::new();
    let mut text = String::new();

    for (i, path) in cli.files.iter().enumerate() {
        let shown = path.display().to_string();
        match inspect::inspect_path(path) {
            Ok(rep) => {
                let checks = check::run(&rep, &opts);
                let verdict = check::verdict(&checks, opts.strict);
                worst = worst.max(verdict);
                if cli.json {
                    docs.push(json::file_document(&rep, &checks, verdict));
                } else {
                    if i > 0 {
                        text.push('\n');
                    }
                    text.push_str(&report::render(&rep, &checks, verdict, cli.boxes, style));
                }
            }
            Err(err) => {
                had_error = true;
                if cli.json {
                    docs.push(json::error_document(&shown, &err.to_string()));
                } else {
                    eprintln!("jxlpeek: {shown}: {err}");
                }
            }
        }
    }

    if cli.json {
        let overall = if had_error { "error" } else { worst.id() };
        println!("{}", json::root_document(VERSION, &docs, overall));
    } else {
        print!("{text}");
    }

    Ok(if had_error {
        ExitCode::from(2)
    } else if worst == Level::Fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}
