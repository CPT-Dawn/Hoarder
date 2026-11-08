//! `docs/config.md` documents every key the default config has.

use toml_edit::{DocumentMut, Item, Table};

fn keys(table: &Table, prefix: &str, out: &mut Vec<String>) {
    for (key, item) in table {
        let path = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        match item {
            Item::Table(t) => keys(t, &path, out),
            _ => out.push(path),
        }
    }
}

#[test]
fn every_config_key_is_in_the_reference() {
    let default: DocumentMut = include_str!("../src/default.toml").parse().unwrap();
    let reference = include_str!("../../../docs/config.md");

    let mut all = Vec::new();
    keys(default.as_table(), "", &mut all);
    assert!(all.len() > 40, "walked only {} keys", all.len());

    let missing: Vec<_> = all
        .iter()
        .filter(|k| !reference.contains(&format!("| `{k}` |")))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/config.md lacks a row for {missing:?}"
    );
}
