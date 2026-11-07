//! Retention, pins and clearing, through the Store's own interface.

use hoarder_store::Store;
use hoarder_store::retention::Limits;

const DAY: i64 = 24 * 60 * 60 * 1000;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

/// Record `text` at `at` and return its id.
fn text(store: &mut Store, text: &str, at: i64) -> i64 {
    store
        .record(&[("text/plain", text.as_bytes())], None, at)
        .unwrap()
        .id
}

fn previews(store: &Store) -> Vec<String> {
    store
        .list(usize::MAX)
        .unwrap()
        .into_iter()
        .map(|c| c.preview)
        .collect()
}

fn blobs(dir: &tempfile::TempDir) -> usize {
    std::fs::read_dir(dir.path().join("blobs")).unwrap().count()
}

#[test]
fn pinning_shows_in_the_clip_and_unpinning_undoes_it() {
    let (_dir, mut store) = store();
    let id = text(&mut store, "keep", 1);

    assert!(store.pin(id, true).unwrap());
    assert!(store.clip(id).unwrap().unwrap().pinned);
    assert!(store.pin(id, false).unwrap());
    assert!(!store.clip(id).unwrap().unwrap().pinned);
    assert!(!store.pin(id + 1, true).unwrap(), "no such Clip");
}

#[test]
fn retain_evicts_the_oldest_unpinned_and_keeps_pins() {
    let (_dir, mut store) = store();
    let pinned = text(&mut store, "oldest, pinned", 1);
    store.pin(pinned, true).unwrap();
    text(&mut store, "old", 2);
    text(&mut store, "new", 3);
    let limits = Limits {
        max_clips: Some(2),
        ..Limits::default()
    };

    let evicted = store.retain(&limits, 4).unwrap();

    assert_eq!(evicted.len(), 1);
    assert_eq!(previews(&store), ["new", "oldest, pinned"]);
}

#[test]
fn retain_expires_by_last_use_and_spares_pins() {
    let (_dir, mut store) = store();
    let pinned = text(&mut store, "pinned", 0);
    store.pin(pinned, true).unwrap();
    text(&mut store, "stale", 1);
    let used = text(&mut store, "used again", 2);
    store.touch(used, 20 * DAY).unwrap();
    let limits = Limits {
        max_age_ms: Some(30 * DAY),
        ..Limits::default()
    };

    store.retain(&limits, 31 * DAY).unwrap();

    assert_eq!(previews(&store), ["used again", "pinned"]);
}

#[test]
fn retain_removes_the_blobs_of_evicted_clips() {
    let (dir, mut store) = store();
    let big = vec![b'x'; 300 * 1024];
    store.record(&[("image/png", &big)], None, 1).unwrap();
    assert_eq!(blobs(&dir), 1);
    let limits = Limits {
        max_total_bytes: Some(1024),
        ..Limits::default()
    };

    store.retain(&limits, 2).unwrap();

    assert!(previews(&store).is_empty());
    assert_eq!(blobs(&dir), 0);
}

#[test]
fn clear_keeps_pinned_clips_unless_asked() {
    let (dir, mut store) = store();
    let pinned = text(&mut store, "pinned", 1);
    store.pin(pinned, true).unwrap();
    text(&mut store, "plain", 2);
    store
        .record(&[("image/png", &vec![1u8; 300 * 1024])], None, 3)
        .unwrap();

    assert_eq!(store.clear(false).unwrap(), 2);
    assert_eq!(previews(&store), ["pinned"]);
    assert_eq!(blobs(&dir), 0);
    assert_eq!(store.clear(true).unwrap(), 1);
    assert!(previews(&store).is_empty());
}
