//! Config editing, the secondary seam (PLAN.md §13): file text plus an edit
//! in, file text out with every comment and bit of formatting preserved.

use hoarder_config::{Config, Diagnostic, EditError, Format, parse, reset, set};

const COMMENTED: &str = r#"# hoarder config
# (hand-written, with odd spacing on purpose)

theme   =   "tokyo-night"   # dark, obviously

[screenshot]
# where captures go
dir = "~/shots"
format = "png"   # png | webp

save = true

# clipboard section
[clipboard]
persist = false
"#;

#[test]
fn set_changes_only_the_value_and_keeps_every_comment() {
    let out = set(COMMENTED, "screenshot.format", "webp").unwrap();

    assert_eq!(
        out,
        COMMENTED.replace(
            r#"format = "png"   # png | webp"#,
            r#"format = "webp"   # png | webp"#
        )
    );
}

#[test]
fn set_keeps_the_comment_above_the_first_key() {
    let out = set(COMMENTED, "theme", "gruvbox-dark").unwrap();

    assert_eq!(
        out,
        COMMENTED.replace(r#""tokyo-night""#, r#""gruvbox-dark""#)
    );
}

#[test]
fn set_round_trips_the_default_file_byte_for_byte() {
    let file = hoarder_config::DEFAULT_FILE;
    let changed = set(file, "history.max_clips", "50").unwrap();
    let back = set(&changed, "history.max_clips", "1000").unwrap();

    assert_ne!(changed, file);
    assert_eq!(back, file);
}

#[test]
fn set_adds_a_missing_key_to_its_existing_table() {
    let out = set(COMMENTED, "screenshot.cursor", "true").unwrap();

    assert_eq!(
        out,
        COMMENTED.replace("save = true\n", "save = true\ncursor = true\n")
    );
}

#[test]
fn set_creates_missing_nested_tables_as_standard_tables() {
    let out = set(COMMENTED, "screenshot.webp.quality", "75").unwrap();

    // Right after its parent table, not at the end of the file.
    assert_eq!(
        out,
        COMMENTED.replace(
            "save = true\n",
            "save = true\n\n[screenshot.webp]\nquality = 75\n"
        )
    );
    assert_eq!(parse(&out).unwrap().screenshot.webp.quality.get(), 75);
}

#[test]
fn set_on_an_empty_file_writes_just_that_setting() {
    assert_eq!(
        set("", "clipboard.primary.history", "true").unwrap(),
        "[clipboard.primary]\nhistory = true\n"
    );
}

#[test]
fn set_takes_bare_words_as_strings_and_toml_as_toml() {
    let out = set("", "screenshot.format", "jpeg").unwrap();
    assert_eq!(parse(&out).unwrap().screenshot.format, Format::Jpeg);

    let out = set("", "privacy.extra_patterns", r#"["^tok_", "x"]"#).unwrap();
    assert_eq!(parse(&out).unwrap().privacy.extra_patterns, ["^tok_", "x"]);

    // A number-looking string for a string setting stays a string.
    let out = set("", "screenshot.name", "2024").unwrap();
    assert_eq!(out, "[screenshot]\nname = \"2024\"\n");

    // `depth` takes "auto" or the integer 8.
    let out = set("", "screenshot.depth", "8").unwrap();
    assert_eq!(out, "[screenshot]\ndepth = 8\n");
}

#[test]
fn set_refuses_unknown_keys_and_sections() {
    assert_eq!(
        set(COMMENTED, "screenshot.nope", "1"),
        Err(EditError::UnknownKey("screenshot.nope".into()))
    );
    assert_eq!(
        set(COMMENTED, "screenshot", "1"),
        Err(EditError::UnknownKey("screenshot".into()))
    );
}

#[test]
fn set_refuses_invalid_values_with_the_reason() {
    let err = set(COMMENTED, "screenshot.format", "gif").unwrap_err();

    let EditError::Invalid { key, problem } = err else {
        panic!("expected Invalid, got {err:?}");
    };
    assert_eq!(key, "screenshot.format");
    assert!(problem.message.contains("gif"), "{problem}");
    assert!(
        problem.message.contains("webp"),
        "lists the choices: {problem}"
    );

    let err = set("", "screenshot.jpeg.quality", "101").unwrap_err();
    assert!(err.to_string().contains("1 to 100"), "{err}");
}

#[test]
fn reset_puts_the_default_back_and_keeps_comments() {
    let out = reset(COMMENTED, "screenshot.format").unwrap();
    assert_eq!(out, COMMENTED);

    let out = reset(COMMENTED, "clipboard.persist").unwrap();
    assert_eq!(out, COMMENTED.replace("persist = false", "persist = true"));
}

#[test]
fn the_default_file_is_the_default_config() {
    assert_eq!(
        parse(hoarder_config::DEFAULT_FILE).unwrap(),
        Config::default()
    );
    assert_eq!(parse("").unwrap(), Config::default());
}

fn diag(text: &str) -> Diagnostic {
    parse(text).unwrap_err()
}

#[test]
fn check_reports_the_line_and_column_of_an_unknown_key() {
    let d = diag("theme = \"zenith\"\n\n[screenshot]\nsave = true\n  formt = \"png\"\n");

    assert_eq!((d.line, d.column), (5, 3), "{d}");
    assert!(d.message.contains("formt"), "{d}");
}

#[test]
fn check_reports_the_line_and_column_of_a_bad_value() {
    let d = diag("[screenshot]\nsave = \"yes\"\n");
    assert_eq!((d.line, d.column), (2, 8), "{d}");

    let d = diag("[screenshot.avif]\nspeed = 11\n");
    assert_eq!((d.line, d.column), (2, 9), "{d}");
    assert!(d.message.contains("1 to 10"), "{d}");

    let d = diag("[screenshot]\nname = \"shot_{size}\"\n");
    assert_eq!((d.line, d.column), (2, 8), "{d}");
    assert!(d.message.contains("{size}"), "{d}");
}

#[test]
fn check_reports_toml_syntax_errors_with_a_position() {
    let d = diag("[screenshot]\nsave = = true\n");
    assert_eq!(d.line, 2, "{d}");
}

#[test]
fn diagnostics_render_as_line_colon_column() {
    assert_eq!(
        diag("nope = 1\n").to_string(),
        format!("1:1: {}", diag("nope = 1\n").message)
    );
}
