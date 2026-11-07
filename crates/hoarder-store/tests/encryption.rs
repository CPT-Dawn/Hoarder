//! Encryption at rest (`history.encrypt`, PLAN.md §5.3), observed through
//! the Store's interface and the raw bytes of every file it leaves on disk.

use std::path::{Path, PathBuf};

use hoarder_store::{Error, Key, Store};

/// Every byte the Store left under `dir`: database, WAL, blobs, anything.
fn disk(dir: &Path) -> Vec<u8> {
    let mut all = Vec::new();
    for file in walk(dir) {
        all.extend(std::fs::read(file).unwrap());
    }
    all
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

const SECRET_TEXT: &[u8] = b"correct horse battery staple, the vault code";
const SECRET_FILE: &[u8] = b"file:///home/me/tax-return-2026.pdf";

/// A stand-in thumbnail (the Store keeps whatever bytes it is given).
const THUMB: &[u8] = b"THUMBNAIL-OF-A-PRIVATE-SCREENSHOT";

/// A representation too big to live inline, so it becomes a blob file.
fn big() -> Vec<u8> {
    let mut big = b"BIGSECRET".repeat(40_000);
    big.extend_from_slice(b"tail-marker-of-the-big-one");
    big
}

fn fill(store: &mut Store) {
    store
        .record(&[("text/plain", SECRET_TEXT)], Some("kitty"), 1_000)
        .unwrap();
    store
        .record(&[("text/uri-list", SECRET_FILE)], None, 2_000)
        .unwrap();
    let id = store
        .record(&[("application/octet-stream", &big())], None, 3_000)
        .unwrap()
        .id;
    store.set_thumb(id, THUMB).unwrap();
}

fn assert_no_plaintext(dir: &Path) {
    let disk = disk(dir);
    let plain_hash = blake3::hash(SECRET_TEXT);
    let needles: [&[u8]; 7] = [
        b"THUMBNAIL-OF",
        &SECRET_TEXT[..20],
        b"vault code",
        b"tax-return",
        b"BIGSECRETBIGSECRET",
        b"tail-marker",
        // A plain blake3 dedup hash would let anyone confirm a guessed copy.
        &plain_hash.as_bytes()[..],
    ];
    for needle in needles {
        assert!(
            !contains(&disk, needle),
            "{:?} is readable on disk",
            String::from_utf8_lossy(needle)
        );
    }
    // Likewise a blob named by the plain blake3 of its content.
    let plain_name = blake3::hash(&big()).to_hex().to_string();
    assert!(!dir.join("blobs").join(plain_name).exists());
    // The full-text indexes on disk are trigram soup the byte scan can't
    // see into; list their terms directly, as anyone holding the file could.
    for index in ["clips_fts", "clips_text"] {
        assert_eq!(
            index_terms(dir, index),
            0,
            "the on-disk {index} index still has terms"
        );
    }
}

/// How many distinct terms the on-disk full-text `index` holds, read
/// straight off the file through fts5vocab.
fn index_terms(dir: &Path, index: &str) -> i64 {
    let db = rusqlite::Connection::open(dir.join("history.db")).unwrap();
    db.execute_batch(&format!(
        "CREATE VIRTUAL TABLE temp.vocab USING fts5vocab(main, {index}, 'row');"
    ))
    .unwrap();
    db.query_row("SELECT count(*) FROM temp.vocab", [], |r| r.get(0))
        .unwrap()
}

fn assert_reads_back(store: &Store) {
    let clips = store.list(10).unwrap();
    let previews: Vec<&str> = clips.iter().map(|c| c.preview.as_str()).collect();
    assert_eq!(
        previews,
        [
            "application/octet-stream",
            "tax-return-2026.pdf",
            "correct horse battery staple, the vault code"
        ]
    );
    assert_eq!(clips[2].source_app.as_deref(), Some("kitty"));
    let reprs = store.representations(clips[0].id).unwrap().unwrap();
    assert_eq!(reprs[0].1, big());
    let reprs = store.representations(clips[2].id).unwrap().unwrap();
    assert_eq!(reprs, [("text/plain".to_owned(), SECRET_TEXT.to_vec())]);
    assert_eq!(store.thumb(clips[0].id).unwrap().as_deref(), Some(THUMB));
    let found = |query: &str| -> Vec<i64> {
        store
            .search(query, 10)
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect()
    };
    assert_eq!(found("battery staple"), [clips[2].id]);
    assert_eq!(found("TAX-RETURN"), [clips[1].id]);
}

/// Close the Store (folding its WAL in) and open it again.
fn reopen(dir: &Path, store: Store) -> Store {
    drop(store);
    Store::open(dir).unwrap()
}

#[test]
fn an_encrypted_history_keeps_no_content_readable_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let key = Key::generate();
    let mut store = Store::open(dir.path()).unwrap();
    store.encrypt(key.clone()).unwrap();
    fill(&mut store);

    assert_no_plaintext(dir.path());
    assert_reads_back(&store);
    let mut store = reopen(dir.path(), store);
    assert_no_plaintext(dir.path());
    store.unlock(key).unwrap();
    assert_reads_back(&store);
}

#[test]
fn an_encrypted_history_cannot_be_read_without_its_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.encrypt(Key::generate()).unwrap();
    fill(&mut store);
    let mut store = reopen(dir.path(), store);

    assert!(store.is_encrypted());
    assert!(matches!(store.list(10), Err(Error::Locked)));
    assert!(matches!(
        store.record(&[("text/plain", b"new")], None, 9_000),
        Err(Error::Locked)
    ));
    assert!(matches!(
        store.unlock(Key::generate()),
        Err(Error::WrongKey)
    ));
    assert!(matches!(store.list(10), Err(Error::Locked)));
}

#[test]
fn turning_encryption_on_migrates_existing_history_and_scrubs_the_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    fill(&mut store);
    let mut store = reopen(dir.path(), store);
    assert!(contains(&disk(dir.path()), b"vault code"), "test premise");
    assert!(index_terms(dir.path(), "clips_fts") > 0, "test premise");
    assert!(index_terms(dir.path(), "clips_text") > 0, "test premise");

    let key = Key::generate();
    store.encrypt(key.clone()).unwrap();
    assert_reads_back(&store);
    let mut store = reopen(dir.path(), store);
    assert_no_plaintext(dir.path());
    store.unlock(key).unwrap();
    assert_reads_back(&store);
}

#[test]
fn turning_encryption_off_migrates_history_back_to_plain() {
    let dir = tempfile::tempdir().unwrap();
    let key = Key::generate();
    let mut store = Store::open(dir.path()).unwrap();
    store.encrypt(key.clone()).unwrap();
    fill(&mut store);
    let mut store = reopen(dir.path(), store);
    store.unlock(key).unwrap();

    store.decrypt().unwrap();
    assert!(!store.is_encrypted());
    let mut store = reopen(dir.path(), store);
    assert!(!store.is_encrypted());
    assert_reads_back(&store);
    // Plain again: a repeat copy still deduplicates against the old Clip.
    let again = store
        .record(&[("text/plain", SECRET_TEXT)], None, 10_000)
        .unwrap();
    assert!(!again.new);
}

#[test]
fn copies_still_deduplicate_under_encryption() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.encrypt(Key::generate()).unwrap();
    let first = store.record(&[("text/plain", b"twice")], None, 1).unwrap();
    let second = store.record(&[("UTF8_STRING", b"twice")], None, 2).unwrap();
    assert_eq!(first.id, second.id);
    assert!(!second.new);
    assert_eq!(store.list(10).unwrap().len(), 1);
}

#[test]
fn a_key_survives_a_round_trip_through_its_secret_string() {
    let dir = tempfile::tempdir().unwrap();
    let key = Key::generate();
    let secret = key.secret();
    assert!(secret.starts_with("AGE-SECRET-KEY-1"), "an age identity");
    let mut store = Store::open(dir.path()).unwrap();
    store.encrypt(key).unwrap();
    fill(&mut store);
    let mut store = reopen(dir.path(), store);
    store.unlock(Key::parse(&secret).unwrap()).unwrap();
    assert_reads_back(&store);
    assert!(Key::parse("not a key").is_err());
}

#[test]
fn search_reaches_past_the_preview_of_an_encrypted_text_clip() {
    let dir = tempfile::tempdir().unwrap();
    let key = Key::generate();
    let mut store = Store::open(dir.path()).unwrap();
    store.encrypt(key.clone()).unwrap();
    let long = format!("{} buried-needle at the end", "filler ".repeat(100));
    let id = store
        .record(&[("text/plain", long.as_bytes())], None, 1)
        .unwrap()
        .id;
    let mut store = reopen(dir.path(), store);
    assert!(matches!(store.search("buried", 10), Err(Error::Locked)));
    store.unlock(key).unwrap();
    let found: Vec<i64> = store
        .search("buried-needle", 10)
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(found, [id]);
    assert_no_plaintext(dir.path());
}
