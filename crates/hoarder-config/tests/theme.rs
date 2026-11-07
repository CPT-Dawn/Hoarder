//! `[theme]`: a theme name plus per-token overrides (PLAN.md §8).

use hoarder_config::{Colour, ThemeName, get, parse, set};

#[test]
fn the_theme_follows_zenith_and_overrides_nothing_by_default() {
    let theme = parse("").unwrap().theme;

    assert_eq!(theme.name, ThemeName::Zenith);
    assert_eq!(theme.accent, None);
    assert_eq!(theme.radius, None);
    assert_eq!(theme.font, "");
}

#[test]
fn overrides_take_css_style_colours() {
    let theme = parse(
        "[theme]\nname = \"gruvbox-dark\"\naccent = \"#f0a\"\nsurface = \"rgb(10 20 30 / 50%)\"\ntext = \"rgba(1, 2, 3, 0.2)\"\nmuted = \"#11223380\"\nradius = 4\nfont = \"Inter, sans-serif\"\n",
    )
    .unwrap()
    .theme;

    assert_eq!(theme.name, ThemeName::GruvboxDark);
    assert_eq!(theme.accent, Some(Colour::rgba(0xff, 0x00, 0xaa, 0xff)));
    assert_eq!(theme.surface, Some(Colour::rgba(10, 20, 30, 128)));
    assert_eq!(theme.text, Some(Colour::rgba(1, 2, 3, 51)));
    assert_eq!(theme.muted, Some(Colour::rgba(0x11, 0x22, 0x33, 0x80)));
    assert_eq!(theme.dim, None);
    assert_eq!(theme.radius, Some(4));
    assert_eq!(theme.font, "Inter, sans-serif");
}

#[test]
fn a_bad_colour_is_reported_where_it_is() {
    let d = parse("[theme]\naccent = \"#12345\"\n").unwrap_err();

    assert_eq!((d.line, d.column), (2, 10), "{d}");
    assert!(d.message.contains("#12345"), "{d}");
}

#[test]
fn an_override_can_be_set_and_unset_from_the_cli() {
    let text = set("", "theme.accent", "#ff79c6").unwrap();
    assert_eq!(
        parse(&text).unwrap().theme.accent,
        Some(Colour::rgba(0xff, 0x79, 0xc6, 0xff))
    );
    let text = set(&text, "theme.radius", "0").unwrap();
    assert_eq!(parse(&text).unwrap().theme.radius, Some(0));

    let text = hoarder_config::reset(&text, "theme.accent").unwrap();
    assert_eq!(parse(&text).unwrap().theme.accent, None);
    assert_eq!(
        get(&parse(&text).unwrap(), Some("theme.radius")).as_deref(),
        Some("0\n")
    );
}
