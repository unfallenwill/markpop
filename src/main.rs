//! Command-line interface: read Markdown, write SVG.
//!
//! Kept deliberately dependency-free. Argument parsing lives in
//! [`parse_args`] so it can be unit tested without spawning a process.

use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;

use markdown2svg::{Options, OutputFormat, RenderError, Theme};

const USAGE: &str = "\
markdown2svg — render Markdown directly to SVG, PNG or WebP

USAGE:
    markdown2svg [OPTIONS] [INPUT]

ARGS:
    <INPUT>    Markdown file to render. Omit or use `-` to read stdin.

OPTIONS:
    -o, --output <FILE>       Write the output to FILE instead of stdout
    -f, --format <NAME>       Output format: svg (default), png or webp
    -z, --zoom <PERCENT>      Output scale as a percentage (default: 100)
        --theme <NAME>        Color theme: light (default), dark or print
        --width <PX>          Canvas width in pixels (default: 800)
        --font-size <PX>      Base font size in pixels (default: 16)
        --line-height <MULT>  Line height multiplier (default: 1.6)
        --letter-spacing <EM> Extra tracking in em units (default: 0)
        --page-height <PX>    Split into pages of this height; writes one
                              file per page (requires --output)
        --font <FILE>         TrueType font used for png/webp output
        --toc                 Prepend a table of contents
    -h, --help                Print this help
    -V, --version             Print version information

EXAMPLES:
    markdown2svg README.md -o README.svg
    markdown2svg README.md --format png -o README.png
    markdown2svg README.md --format webp -o README.webp
    markdown2svg doc.md --theme print --page-height 1123 -o page.svg
    cat doc.md | markdown2svg --theme dark --zoom 150 > doc.svg
";

/// Parsed command line.
#[derive(Debug, Clone, PartialEq)]
struct Cli {
    input: Option<String>,
    output: Option<String>,
    zoom: f32,
    theme: Theme,
    width: Option<f32>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    letter_spacing: Option<f32>,
    page_height: Option<f32>,
    format: OutputFormat,
    font: Option<String>,
    toc: bool,
    help: bool,
    version: bool,
}

impl Default for Cli {
    fn default() -> Cli {
        Cli {
            input: None,
            output: None,
            zoom: 1.0,
            theme: Theme::light(),
            width: None,
            font_size: None,
            line_height: None,
            letter_spacing: None,
            page_height: None,
            format: OutputFormat::Svg,
            font: None,
            toc: false,
            help: false,
            version: false,
        }
    }
}

impl Cli {
    /// Turns the CLI into render options.
    fn options(&self) -> Options {
        let mut theme = self.theme.clone();
        if let Some(w) = self.width {
            theme.width = w;
        }
        if let Some(s) = self.font_size {
            theme.font_size = s;
        }
        if let Some(l) = self.line_height {
            theme.line_height = l;
        }
        if let Some(s) = self.letter_spacing {
            theme.letter_spacing = s;
        }
        Options {
            theme,
            zoom: self.zoom,
            page_height: self.page_height,
            toc: self.toc,
            toc_title: "Contents".to_string(),
            format: self.format,
            font_file: self.font.clone(),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_args(&args) {
        Ok(cli) => cli,
        Err(msg) => {
            eprintln!("error: {msg}\n");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    if cli.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("markdown2svg {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Executes a parsed command line.
fn run(cli: &Cli) -> Result<(), String> {
    let markdown = read_input(cli.input.as_deref())?;
    let pages =
        markdown2svg::render_bytes_pages(&markdown, &cli.options()).map_err(render_message)?;
    write_pages(cli.output.as_deref(), &pages, cli.format.extension())
}

fn read_input(path: Option<&str>) -> Result<String, String> {
    match path {
        None | Some("-") => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("cannot read stdin: {e}"))?;
            Ok(buf)
        }
        Some(path) => {
            std::fs::read_to_string(path).map_err(|e| format!("cannot read `{path}`: {e}"))
        }
    }
}

/// Writes one or more pages, numbering them when paginated.
fn write_pages(path: Option<&str>, pages: &[Vec<u8>], ext: &str) -> Result<(), String> {
    match path {
        None | Some("-") => {
            if pages.len() > 1 {
                return Err(format!(
                    "{} pages were produced; use `--output` to write them to files",
                    pages.len()
                ));
            }
            write_output(None, &pages[0])
        }
        Some(path) => {
            if pages.len() == 1 {
                return write_output(Some(path), &pages[0]);
            }
            for (i, page) in pages.iter().enumerate() {
                let target = numbered_path(path, i + 1, ext);
                write_output(Some(&target), page)?;
            }
            Ok(())
        }
    }
}

/// `dir/name.svg` + page 2 → `dir/name-2.svg` (or `-2.png` for PNG output).
fn numbered_path(base: &str, index: usize, ext: &str) -> String {
    let path = Path::new(base);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "page".to_string());
    let name = format!("{stem}-{index}.{ext}");
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(name).to_string_lossy().to_string(),
        _ => name,
    }
}

fn write_output(path: Option<&str>, data: &[u8]) -> Result<(), String> {
    match path {
        None | Some("-") => {
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            lock.write_all(data)
                .and_then(|()| lock.flush())
                .map_err(|e| format!("cannot write to stdout: {e}"))
        }
        Some(path) => std::fs::write(path, data).map_err(|e| format!("cannot write `{path}`: {e}")),
    }
}

/// Human-readable message for an option validation failure.
fn render_message(err: RenderError) -> String {
    match err {
        RenderError::InvalidZoom(_) => {
            format!("{err}\n      hint: --zoom expects a positive percentage, e.g. 150")
        }
        RenderError::InvalidCanvasWidth(_) => {
            format!("{err}\n      hint: --width must be at least 160 pixels")
        }
        RenderError::InvalidPageHeight(_) => {
            format!("{err}\n      hint: --page-height must be at least 176 pixels")
        }
        RenderError::FontUnavailable => {
            format!("{err}\n      hint: install a system font, or pass one with --font <FILE.ttf>")
        }
    }
}

/// Parses command-line arguments (excluding the program name).
fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-h" | "--help" => {
                cli.help = true;
                i += 1;
            }
            "-V" | "--version" => {
                cli.version = true;
                i += 1;
            }
            "-o" | "--output" => {
                cli.output = Some(take_value(args, &mut i, arg)?);
            }
            "-z" | "--zoom" => {
                let raw = take_value(args, &mut i, arg)?;
                cli.zoom = parse_zoom(&raw)?;
            }
            "--theme" => {
                let raw = take_value(args, &mut i, arg)?;
                cli.theme = Theme::by_name(&raw).ok_or_else(|| {
                    format!("unknown theme `{raw}` (expected light, dark or print)")
                })?;
            }
            "--width" => {
                cli.width = Some(parse_number(&take_value(args, &mut i, arg)?, "width")?);
            }
            "--font-size" => {
                let v = parse_number(&take_value(args, &mut i, arg)?, "font size")?;
                if v <= 0.0 {
                    return Err("invalid font size: must be greater than 0".into());
                }
                cli.font_size = Some(v);
            }
            "--line-height" => {
                let v = parse_number(&take_value(args, &mut i, arg)?, "line height")?;
                if v <= 0.0 {
                    return Err("invalid line height: must be greater than 0".into());
                }
                cli.line_height = Some(v);
            }
            "--letter-spacing" => {
                let v = parse_number(&take_value(args, &mut i, arg)?, "letter spacing")?;
                if v < 0.0 {
                    return Err("invalid letter spacing: must not be negative".into());
                }
                cli.letter_spacing = Some(v);
            }
            "--page-height" => {
                let v = parse_number(&take_value(args, &mut i, arg)?, "page height")?;
                if v <= 0.0 {
                    return Err("invalid page height: must be greater than 0".into());
                }
                cli.page_height = Some(v);
            }
            "-f" | "--format" => {
                let raw = take_value(args, &mut i, arg)?;
                cli.format = OutputFormat::by_name(&raw)
                    .ok_or_else(|| format!("unknown format `{raw}` (expected svg, png or webp)"))?;
            }
            "--font" => {
                cli.font = Some(take_value(args, &mut i, arg)?);
            }
            "--toc" => {
                cli.toc = true;
                i += 1;
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option `{other}`"));
            }
            other => {
                if cli.input.is_some() {
                    return Err(format!("unexpected extra argument `{other}`"));
                }
                cli.input = Some(other.to_string());
                i += 1;
            }
        }
    }
    Ok(cli)
}

/// Consumes the value following a flag.
fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    match args.get(*i + 1) {
        Some(v) => {
            *i += 2;
            Ok(v.clone())
        }
        None => Err(format!("option `{flag}` needs a value")),
    }
}

/// Parses a plain number, reporting which option it belonged to.
fn parse_number(raw: &str, what: &str) -> Result<f32, String> {
    let value: f32 = raw
        .parse()
        .map_err(|_| format!("invalid {what} `{raw}` (expected a number)"))?;
    if !value.is_finite() {
        return Err(format!("invalid {what} `{raw}` (expected a finite number)"));
    }
    Ok(value)
}

/// Parses a zoom percentage (`100`, `150`, `12.5`, `150%`) into a factor.
fn parse_zoom(raw: &str) -> Result<f32, String> {
    let trimmed = raw.trim().trim_end_matches('%');
    let percent: f32 = trimmed
        .parse()
        .map_err(|_| format!("invalid zoom `{raw}` (expected a percentage, e.g. 150)"))?;
    if !percent.is_finite() || percent <= 0.0 {
        return Err(format!("invalid zoom `{raw}`: must be greater than 0"));
    }
    Ok(percent / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults() {
        let cli = parse_args(&[]).unwrap();
        assert_eq!(cli.zoom, 1.0);
        assert_eq!(cli.input, None);
        assert_eq!(cli.output, None);
        assert_eq!(cli.theme, Theme::light());
        assert!(!cli.help);
        assert!(!cli.toc);
        assert_eq!(cli.page_height, None);
    }

    #[test]
    fn positional_input() {
        let cli = parse_args(&args(&["README.md"])).unwrap();
        assert_eq!(cli.input.as_deref(), Some("README.md"));
    }

    #[test]
    fn input_and_output() {
        let cli = parse_args(&args(&["in.md", "-o", "out.svg"])).unwrap();
        assert_eq!(cli.input.as_deref(), Some("in.md"));
        assert_eq!(cli.output.as_deref(), Some("out.svg"));
    }

    #[test]
    fn long_option_forms() {
        let cli = parse_args(&args(&["--output", "o.svg", "--zoom", "150"])).unwrap();
        assert_eq!(cli.output.as_deref(), Some("o.svg"));
        assert_eq!(cli.zoom, 1.5);
    }

    #[test]
    fn zoom_accepts_percent_sign_and_decimals() {
        assert_eq!(parse_zoom("100").unwrap(), 1.0);
        assert_eq!(parse_zoom("100%").unwrap(), 1.0);
        assert_eq!(parse_zoom("12.5").unwrap(), 0.125);
    }

    #[test]
    fn zoom_rejects_bad_values() {
        for bad in ["0", "-5", "abc", "NaN", "inf"] {
            let err = parse_zoom(bad).unwrap_err();
            assert!(err.contains("zoom"), "{bad}: {err}");
        }
    }

    #[test]
    fn theme_selection() {
        assert_eq!(
            parse_args(&args(&["--theme", "dark"])).unwrap().theme,
            Theme::dark()
        );
        assert_eq!(
            parse_args(&args(&["--theme", "print"])).unwrap().theme,
            Theme::print()
        );
        assert!(parse_args(&args(&["--theme", "neon"])).is_err());
    }

    #[test]
    fn width_override() {
        let cli = parse_args(&args(&["--width", "1200"])).unwrap();
        assert_eq!(cli.options().theme.width, 1200.0);
        assert!(parse_args(&args(&["--width", "wide"])).is_err());
    }

    #[test]
    fn typography_flags_reach_the_theme() {
        let cli = parse_args(&args(&[
            "--font-size",
            "18",
            "--line-height",
            "1.8",
            "--letter-spacing",
            "0.02",
        ]))
        .unwrap();
        let theme = cli.options().theme;
        assert_eq!(theme.font_size, 18.0);
        assert_eq!(theme.line_height, 1.8);
        assert!((theme.letter_spacing - 0.02).abs() < 1e-6);
    }

    #[test]
    fn typography_flags_validate_input() {
        for (flag, value) in [
            ("--font-size", "0"),
            ("--font-size", "big"),
            ("--line-height", "-1"),
            ("--letter-spacing", "-0.1"),
            ("--page-height", "0"),
            ("--page-height", "tall"),
        ] {
            assert!(
                parse_args(&args(&[flag, value])).is_err(),
                "{flag} {value} should fail"
            );
        }
    }

    #[test]
    fn page_height_and_toc_flags() {
        let cli = parse_args(&args(&["--page-height", "1123", "--toc"])).unwrap();
        assert_eq!(cli.page_height, Some(1123.0));
        assert!(cli.toc);
        assert_eq!(cli.options().page_height, Some(1123.0));
        assert!(cli.options().toc);
    }

    #[test]
    fn missing_values_are_errors() {
        for flag in [
            "-o",
            "--output",
            "-z",
            "--zoom",
            "--theme",
            "--width",
            "--font-size",
            "--line-height",
            "--letter-spacing",
            "--page-height",
        ] {
            let err = parse_args(&args(&[flag])).unwrap_err();
            assert!(err.contains("needs a value"), "{flag}: {err}");
        }
    }

    #[test]
    fn unknown_flags_are_errors() {
        let err = parse_args(&args(&["--nope"])).unwrap_err();
        assert!(err.contains("unknown option"), "{err}");
    }

    #[test]
    fn extra_positional_is_an_error() {
        assert!(parse_args(&args(&["a.md", "b.md"])).is_err());
    }

    #[test]
    fn help_and_version_flags() {
        assert!(parse_args(&args(&["-h"])).unwrap().help);
        assert!(parse_args(&args(&["--help"])).unwrap().help);
        assert!(parse_args(&args(&["-V"])).unwrap().version);
        assert!(parse_args(&args(&["--version"])).unwrap().version);
    }

    #[test]
    fn dash_means_stdin() {
        let cli = parse_args(&args(&["-"])).unwrap();
        assert_eq!(cli.input.as_deref(), Some("-"));
    }

    #[test]
    fn zoom_flag_is_applied_to_options() {
        let cli = parse_args(&args(&["-z", "250"])).unwrap();
        assert_eq!(cli.options().zoom, 2.5);
    }

    #[test]
    fn numbered_paths_keep_directory_and_extension() {
        // Normalize separators so the assertion is platform independent.
        let norm = |p: &str| Path::new(p).to_string_lossy().replace('\\', "/");
        assert_eq!(norm(&numbered_path("out.svg", 2, "svg")), "out-2.svg");
        assert_eq!(
            norm(&numbered_path("docs/out.svg", 3, "svg")),
            "docs/out-3.svg"
        );
        assert_eq!(norm(&numbered_path("out", 1, "svg")), "out-1.svg");
        assert_eq!(
            norm(&numbered_path("/tmp/x.html", 4, "html")),
            "/tmp/x-4.html"
        );
    }

    // --- end-to-end through files -------------------------------------

    /// A unique scratch path for a test file.
    fn scratch(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("markdown2svg-test-{}-{name}", std::process::id()));
        path
    }

    #[test]
    fn run_renders_a_file_to_a_file() {
        let input = scratch("in.md");
        let output = scratch("out.svg");
        std::fs::write(&input, "# Hi\n\nbody").unwrap();
        let cli = parse_args(&args(&[
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--zoom",
            "200",
        ]))
        .unwrap();

        run(&cli).unwrap();

        let svg = std::fs::read_to_string(&output).unwrap();
        assert!(svg.contains("Hi") && svg.contains("body"));
        assert!(svg.contains(r#"width="1600""#), "zoom was not applied");
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn run_writes_one_file_per_page() {
        let input = scratch("paged.md");
        let output = scratch("paged.svg");
        let body = (0..40)
            .map(|i| format!("paragraph {i} with enough words to take a line\n\n"))
            .collect::<String>();
        std::fs::write(&input, body).unwrap();
        let cli = parse_args(&args(&[
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--page-height",
            "400",
        ]))
        .unwrap();

        run(&cli).unwrap();

        for i in 1..=3 {
            let path = numbered_path(output.to_str().unwrap(), i, "svg");
            let svg =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing page {i}: {e}"));
            assert!(svg.contains(r#"height="400""#));
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(&input);
    }

    #[test]
    fn multi_page_output_to_stdout_is_refused() {
        let pages = vec![b"<svg/>".to_vec(), b"<svg/>".to_vec()];
        let err = write_pages(None, &pages, "svg").unwrap_err();
        assert!(err.contains("--output"), "{err}");
    }

    #[test]
    fn toc_flag_adds_contents_to_the_output() {
        let input = scratch("toc.md");
        let output = scratch("toc.svg");
        std::fs::write(&input, "# Alpha\n\n## Beta\n").unwrap();
        let cli = parse_args(&args(&[
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--toc",
        ]))
        .unwrap();
        run(&cli).unwrap();
        let svg = std::fs::read_to_string(&output).unwrap();
        assert!(svg.contains("Contents") && svg.contains("Alpha") && svg.contains("Beta"));
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn missing_input_file_reports_the_path() {
        let cli = Cli {
            input: Some("markdown2svg-does-not-exist.md".into()),
            ..Cli::default()
        };
        let err = run(&cli).unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
        assert!(err.contains("markdown2svg-does-not-exist.md"), "{err}");
    }

    #[test]
    fn unwritable_output_reports_the_path() {
        let input = scratch("in2.md");
        std::fs::write(&input, "x").unwrap();
        let cli = Cli {
            input: Some(input.to_str().unwrap().to_string()),
            output: Some("markdown2svg-no-such-dir/out.svg".into()),
            ..Cli::default()
        };
        let err = run(&cli).unwrap_err();
        assert!(err.contains("cannot write"), "{err}");
        let _ = std::fs::remove_file(&input);
    }

    #[test]
    fn invalid_zoom_is_reported_with_a_hint() {
        let input = scratch("in3.md");
        std::fs::write(&input, "x").unwrap();
        let cli = Cli {
            input: Some(input.to_str().unwrap().to_string()),
            zoom: 0.0,
            ..Cli::default()
        };
        let err = run(&cli).unwrap_err();
        assert!(err.contains("zoom") && err.contains("hint"), "{err}");
        let _ = std::fs::remove_file(&input);
    }

    #[test]
    fn narrow_canvas_is_reported_with_a_hint() {
        let msg = render_message(RenderError::InvalidCanvasWidth(10.0));
        assert!(msg.contains("width") && msg.contains("hint"), "{msg}");
    }

    #[test]
    fn tiny_page_height_is_reported_with_a_hint() {
        let msg = render_message(RenderError::InvalidPageHeight(10.0));
        assert!(msg.contains("page-height") && msg.contains("hint"), "{msg}");
    }

    // --- output formats -------------------------------------------------

    #[test]
    fn format_flag_selects_the_engine() {
        assert_eq!(
            parse_args(&args(&["--format", "png"])).unwrap().format,
            OutputFormat::Png
        );
        assert_eq!(
            parse_args(&args(&["-f", "webp"])).unwrap().format,
            OutputFormat::Webp
        );
        assert_eq!(
            parse_args(&args(&["--format", "SVG"])).unwrap().format,
            OutputFormat::Svg
        );
        let err = parse_args(&args(&["--format", "gif"])).unwrap_err();
        assert!(err.contains("unknown format"), "{err}");
    }

    #[test]
    fn format_reaches_the_options() {
        let cli = parse_args(&args(&["--format", "png"])).unwrap();
        assert_eq!(cli.options().format, OutputFormat::Png);
        let cli = parse_args(&args(&["--format", "webp", "--font", "x.ttf"])).unwrap();
        assert_eq!(cli.options().font_file.as_deref(), Some("x.ttf"));
    }

    #[test]
    fn run_writes_png_and_webp() {
        let input = scratch("fmt.md");
        std::fs::write(&input, "# Hi\n\nBody text").unwrap();
        for (format, magic) in [
            ("png", &[0x89u8, b'P', b'N', b'G'][..]),
            ("webp", &b"RIFF"[..]),
        ] {
            let output = scratch(&format!("fmt.{format}"));
            let cli = parse_args(&args(&[
                input.to_str().unwrap(),
                "--format",
                format,
                "-o",
                output.to_str().unwrap(),
            ]))
            .unwrap();
            match run(&cli) {
                Ok(()) => {
                    let bytes = std::fs::read(&output).unwrap();
                    assert_eq!(&bytes[..magic.len()], magic, "{format} magic");
                    assert!(bytes.len() > 100);
                }
                Err(e) if e.contains("font") => {
                    eprintln!("skipping {format}: no system fonts ({e})");
                }
                Err(e) => panic!("{format} failed: {e}"),
            }
            let _ = std::fs::remove_file(&output);
        }
        let _ = std::fs::remove_file(&input);
    }

    #[test]
    fn multi_page_raster_output_uses_the_format_extension() {
        let input = scratch("paged-png.md");
        let output = scratch("pages.svg");
        let body = (0..40)
            .map(|i| format!("paragraph {i} with some words\n\n"))
            .collect::<String>();
        std::fs::write(&input, body).unwrap();
        let cli = parse_args(&args(&[
            input.to_str().unwrap(),
            "--format",
            "png",
            "--page-height",
            "400",
            "-o",
            output.to_str().unwrap(),
        ]))
        .unwrap();
        match run(&cli) {
            Ok(()) => {
                let first = numbered_path(output.to_str().unwrap(), 1, "png");
                assert!(first.ends_with("-1.png"), "{first}");
                let bytes = std::fs::read(&first).unwrap();
                assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
                let _ = std::fs::remove_file(first);
            }
            Err(e) if e.contains("font") => eprintln!("skipping: no system fonts"),
            Err(e) => panic!("{e}"),
        }
        let _ = std::fs::remove_file(&input);
    }

    #[test]
    fn font_unavailable_is_reported_with_a_hint() {
        let msg = render_message(RenderError::FontUnavailable);
        assert!(msg.contains("font") && msg.contains("--font"), "{msg}");
    }
}
