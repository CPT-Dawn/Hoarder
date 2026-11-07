//! Search: what the Picker's full-text half finds.

use hoarder_store::Store;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("hoarder")).unwrap();
    (dir, store)
}

fn ids(clips: Vec<hoarder_store::Clip>) -> Vec<i64> {
    clips.into_iter().map(|c| c.id).collect()
}

#[test]
fn search_finds_text_past_the_preview_newest_first() {
    let (_dir, mut store) = store();
    let long = format!("{} Needle here", "filler ".repeat(100));
    let a = store
        .record(&[("text/plain", long.as_bytes())], None, 1)
        .unwrap()
        .id;
    let b = store
        .record(&[("text/plain", b"a needle too")], None, 2)
        .unwrap()
        .id;
    store
        .record(&[("text/plain", b"nothing")], None, 3)
        .unwrap();

    assert_eq!(ids(store.search("needle", 10).unwrap()), [b, a]);
    assert_eq!(ids(store.search("needle", 1).unwrap()), [b]);
}

#[test]
fn search_treats_query_syntax_as_plain_text() {
    let (_dir, mut store) = store();
    let id = store
        .record(&[("text/plain", br#"say "hi": a-b OR c*"#)], None, 1)
        .unwrap()
        .id;

    assert_eq!(ids(store.search(r#""hi": a-b OR c*"#, 10).unwrap()), [id]);
}

#[test]
fn short_queries_and_deleted_clips_find_nothing() {
    let (_dir, mut store) = store();
    let id = store
        .record(&[("text/plain", b"abcdef")], None, 1)
        .unwrap()
        .id;
    assert!(store.search("ab", 10).unwrap().is_empty());

    store.delete(id).unwrap();

    assert!(store.search("abcdef", 10).unwrap().is_empty());
}

#[test]
fn files_are_searched_by_their_preview() {
    let (_dir, mut store) = store();
    let id = store
        .record(
            &[("text/uri-list", b"file:///home/me/report.pdf\n")],
            None,
            1,
        )
        .unwrap()
        .id;

    assert_eq!(ids(store.search("report", 10).unwrap()), [id]);
}
