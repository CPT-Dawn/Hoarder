//! SQLite-backed History store for Hoarder: schema, blobs, dedup, retention and search.
//!
//! One [`Store`] is one History directory (`$XDG_DATA_HOME/hoarder/`, PLAN.md
//! §5.3): `history.db` plus `blobs/<blake3>` for representations over
//! [`INLINE_MAX`]. The directories are mode 0700 and every file 0600.
//!
//! A [`Connection`](rusqlite::Connection) is `Send` but not `Sync`: own the
//! Store on one thread and send it work.
//!
//! # Encryption at rest
//!
//! An encrypted History ([`Store::encrypt`]) seals every piece of content
//! with age to the [`Key`]'s X25519 recipient: each inline representation,
//! each blob file, each thumbnail, and each Clip's preview and search text.
//! Search keeps working through full-text indexes that live only in memory
//! (`temp.clips_fts` and `temp.clips_text`, which shadow the on-disk ones by
//! name and are rebuilt from the sealed text by [`Store::unlock`]); the
//! on-disk indexes then hold nothing. Dedup hashes and blob file names are blake3 keyed with
//! a key derived from the History key, so they confirm nothing to someone
//! hashing a guess. What stays readable is metadata: times, pinned, the
//! source app, kind, MIME types and sizes.

mod clip;
mod crypt;
pub mod retention;

use std::fs::{DirBuilder, OpenOptions, Permissions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension as _, Transaction, params};

pub use clip::Kind;
pub use crypt::Key;

/// Representations up to this size live in the database; larger ones are
/// blob files.
pub const INLINE_MAX: usize = 256 * 1024;

/// The schema migrations, in order; `user_version` counts those applied.
const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE clips(
    id INTEGER PRIMARY KEY,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    source_app TEXT,
    kind TEXT NOT NULL,
    preview TEXT NOT NULL,
    size INTEGER NOT NULL,
    hash BLOB NOT NULL UNIQUE
);
CREATE INDEX clips_last_used ON clips(last_used_at);
CREATE TABLE reprs(
    clip_id INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
    ord INTEGER NOT NULL,
    mime TEXT NOT NULL,
    inline BLOB,
    blob_hash BLOB,
    PRIMARY KEY(clip_id, ord)
);
CREATE INDEX reprs_blob ON reprs(blob_hash) WHERE blob_hash IS NOT NULL;
CREATE TABLE thumbs(
    clip_id INTEGER PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,
    png BLOB NOT NULL
);
CREATE VIRTUAL TABLE clips_fts USING fts5(
    preview, content='clips', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER clips_fts_insert AFTER INSERT ON clips BEGIN
    INSERT INTO clips_fts(rowid, preview) VALUES (new.id, new.preview);
END;
CREATE TRIGGER clips_fts_delete AFTER DELETE ON clips BEGIN
    INSERT INTO clips_fts(clips_fts, rowid, preview) VALUES ('delete', old.id, old.preview);
END;
CREATE TRIGGER clips_fts_update AFTER UPDATE OF preview ON clips BEGIN
    INSERT INTO clips_fts(clips_fts, rowid, preview) VALUES ('delete', old.id, old.preview);
    INSERT INTO clips_fts(rowid, preview) VALUES (new.id, new.preview);
END;
"#,
    r#"
CREATE VIRTUAL TABLE clips_text USING fts5(
    body, content='', contentless_delete=1, tokenize='trigram'
);
INSERT INTO clips_text(rowid, body) SELECT id, preview FROM clips;
CREATE TRIGGER clips_text_delete AFTER DELETE ON clips BEGIN
    DELETE FROM clips_text WHERE rowid = old.id;
END;
"#,
    // Encryption at rest: `sealed` is the age-sealed preview and search
    // text of a Clip in an encrypted History; `encryption` names the
    // recipient it is sealed to (one row) or is empty.
    r#"
ALTER TABLE clips ADD COLUMN sealed BLOB;
CREATE TABLE encryption(
    id INTEGER PRIMARY KEY CHECK (id = 1),
    recipient TEXT NOT NULL
);
"#,
];

/// Why the Store failed.
#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// The History is encrypted and has not been given its key.
    Locked,
    /// The key given is not the one the History is encrypted with.
    WrongKey,
    /// Sealed content that would not open, or a malformed key.
    Crypto(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "History database: {e}"),
            Error::Io(e) => write!(f, "History files: {e}"),
            Error::Locked => f.write_str("History is encrypted and its key is not available"),
            Error::WrongKey => {
                f.write_str("the key given is not the one History is encrypted with")
            }
            Error::Crypto(e) => write!(f, "History encryption: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One stored representation: a MIME type and its bytes.
pub type Representation = (String, Vec<u8>);

/// One Clip's metadata, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub id: i64,
    /// Unix time in milliseconds of the first copy.
    pub created_at: i64,
    /// Unix time in milliseconds of the latest copy or Restore.
    pub last_used_at: i64,
    pub pinned: bool,
    /// The app focused when it was copied, if known.
    pub source_app: Option<String>,
    pub kind: Kind,
    pub preview: String,
    /// Total bytes of all representations.
    pub size: u64,
    /// The MIME types, in the source's order.
    pub types: Vec<String>,
}

/// The outcome of [`Store::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    pub id: i64,
    /// `false` when the content was already a Clip, which moved to the top.
    pub new: bool,
}

/// The History on disk.
#[derive(Debug)]
pub struct Store {
    db: Connection,
    blobs: PathBuf,
    /// The recipient an encrypted History is sealed to; `None` when plain.
    recipient: Option<String>,
    /// The key, once an encrypted History is unlocked.
    key: Option<Key>,
}

impl Store {
    /// Open (or create) the History in `dir`, upgrading its schema. An
    /// encrypted History opens locked: give it its key with [`Store::unlock`].
    pub fn open(dir: &Path) -> Result<Store> {
        private_dir(dir)?;
        let blobs = dir.join("blobs");
        private_dir(&blobs)?;
        let path = dir.join("history.db");
        // SQLite gives its -wal and -shm files the database file's mode.
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)?;
        std::fs::set_permissions(&path, Permissions::from_mode(0o600))?;
        let mut db = Connection::open(&path)?;
        // secure_delete: freed pages (deleted Clips, superseded plaintext)
        // are zeroed rather than left lying in the file. temp_store: the
        // in-memory search index of an encrypted History never spills.
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
             PRAGMA secure_delete=ON; PRAGMA temp_store=MEMORY;",
        )?;
        migrate(&mut db)?;
        let recipient = db
            .query_row("SELECT recipient FROM encryption", [], |r| r.get(0))
            .optional()?;
        let store = Store {
            db,
            blobs,
            recipient,
            key: None,
        };
        store.sweep_blobs()?;
        Ok(store)
    }

    /// Whether the History on disk is encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.recipient.is_some()
    }

    /// Give an encrypted History its key. A plain History ignores it.
    pub fn unlock(&mut self, key: Key) -> Result<()> {
        let Some(recipient) = &self.recipient else {
            return Ok(());
        };
        if *recipient != key.recipient() {
            return Err(Error::WrongKey);
        }
        if self.key.is_none() {
            self.key = Some(key);
            self.index_in_memory()?;
        }
        Ok(())
    }

    /// Encrypt the History with `key`, sealing everything already in it.
    /// Already encrypted with `key`, this just unlocks it.
    pub fn encrypt(&mut self, key: Key) -> Result<()> {
        if self.is_encrypted() {
            return self.unlock(key);
        }
        self.rewrite(Some(key))
    }

    /// Decrypt the History back to plain, which needs it unlocked.
    pub fn decrypt(&mut self) -> Result<()> {
        if !self.is_encrypted() {
            return Ok(());
        }
        self.sealer()?;
        self.rewrite(None)
    }

    /// The key to seal with: `None` for a plain History.
    fn sealer(&self) -> Result<Option<&Key>> {
        match (&self.recipient, &self.key) {
            (None, _) => Ok(None),
            (Some(_), Some(key)) => Ok(Some(key)),
            (Some(_), None) => Err(Error::Locked),
        }
    }

    /// Record a Selection change: `reprs` are its `(mime, bytes)` in the
    /// source's order. Content already in History is not added again; that
    /// Clip's last use moves to `now` (Unix ms) instead.
    pub fn record(
        &mut self,
        reprs: &[(&str, &[u8])],
        source_app: Option<&str>,
        now: i64,
    ) -> Result<Recorded> {
        let key = self.sealer()?.cloned();
        let key = key.as_ref();
        let (kind, preview, hash, text) = clip::describe_with_text(reprs, key);
        let tx = self.db.transaction()?;
        let now = next_time(&tx, now)?;
        let existing: Option<i64> = tx
            .query_row("SELECT id FROM clips WHERE hash = ?1", [&hash[..]], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(id) = existing {
            tx.execute(
                "UPDATE clips SET last_used_at = ?1 WHERE id = ?2",
                params![now, id],
            )?;
            tx.commit()?;
            return Ok(Recorded { id, new: false });
        }
        let size: usize = reprs.iter().map(|(_, d)| d.len()).sum();
        let (plain_preview, sealed) = match key {
            Some(key) => ("", Some(key.seal(&seal_text(&preview, &text))?)),
            None => (preview.as_str(), None),
        };
        tx.execute(
            "INSERT INTO clips(created_at, last_used_at, source_app, kind, preview,
                               sealed, size, hash)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                now,
                source_app,
                kind.as_str(),
                plain_preview,
                sealed,
                size as i64,
                &hash[..]
            ],
        )?;
        let id = tx.last_insert_rowid();
        if key.is_some() {
            tx.execute(
                "INSERT INTO temp.clips_fts(rowid, preview) VALUES (?1, ?2)",
                params![id, preview],
            )?;
            tx.execute(
                "INSERT INTO temp.clips_text(rowid, body) VALUES (?1, ?2)",
                params![id, text],
            )?;
        } else {
            tx.execute(
                "INSERT INTO main.clips_text(rowid, body) VALUES (?1, ?2)",
                params![id, text],
            )?;
        }
        for (ord, (mime, data)) in reprs.iter().enumerate() {
            let (inline, blob) = store_repr(&self.blobs, key, data)?;
            tx.execute(
                "INSERT INTO reprs(clip_id, ord, mime, inline, blob_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, ord as i64, mime, inline, blob],
            )?;
        }
        tx.commit()?;
        Ok(Recorded { id, new: true })
    }

    /// Up to `limit` Clips, most recently used first.
    pub fn list(&self, limit: usize) -> Result<Vec<Clip>> {
        self.sealer()?;
        let mut stmt = self
            .db
            .prepare_cached("SELECT id FROM clips ORDER BY last_used_at DESC, id DESC LIMIT ?1")?;
        let ids = stmt
            .query_map([limit.min(i64::MAX as usize) as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        ids.into_iter()
            .filter_map(|id| self.clip(id).transpose())
            .collect()
    }

    /// Up to `limit` Clips whose text contains `query` (case-insensitive),
    /// most recently used first. Text Clips are searched through their
    /// first 64 KB, not just their preview; other Clips through their
    /// preview. Queries under three characters match nothing (the trigram
    /// index cannot answer them; the Picker's fuzzy match covers those).
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Clip>> {
        // Under encryption `clips_text` is the in-memory index.
        self.sealer()?;
        if query.chars().count() < 3 {
            return Ok(Vec::new());
        }
        let phrase = format!("\"{}\"", query.replace('"', "\"\""));
        let mut stmt = self.db.prepare_cached(
            "SELECT c.id FROM clips_text JOIN clips c ON c.id = clips_text.rowid
             WHERE clips_text MATCH ?1
             ORDER BY c.last_used_at DESC, c.id DESC LIMIT ?2",
        )?;
        let ids = stmt
            .query_map(params![phrase, limit.min(i64::MAX as usize) as i64], |r| {
                r.get(0)
            })?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        ids.into_iter()
            .filter_map(|id| self.clip(id).transpose())
            .collect()
    }

    /// One Clip's metadata.
    pub fn clip(&self, id: i64) -> Result<Option<Clip>> {
        let key = self.sealer()?;
        // An encrypted History's previews come from the in-memory index.
        let sql = if key.is_some() {
            "SELECT created_at, last_used_at, pinned, source_app, kind,
                    (SELECT preview FROM temp.clips_fts WHERE rowid = clips.id), size
             FROM clips WHERE id = ?1"
        } else {
            "SELECT created_at, last_used_at, pinned, source_app, kind, preview, size
             FROM clips WHERE id = ?1"
        };
        let clip = self
            .db
            .prepare_cached(sql)?
            .query_row([id], |r| {
                Ok(Clip {
                    id,
                    created_at: r.get(0)?,
                    last_used_at: r.get(1)?,
                    pinned: r.get(2)?,
                    source_app: r.get(3)?,
                    kind: Kind::parse(&r.get::<_, String>(4)?),
                    preview: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    size: r.get::<_, i64>(6)? as u64,
                    types: Vec::new(),
                })
            })
            .optional()?;
        let Some(mut clip) = clip else {
            return Ok(None);
        };
        clip.types = self
            .db
            .prepare_cached("SELECT mime FROM reprs WHERE clip_id = ?1 ORDER BY ord")?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(clip))
    }

    /// A Clip's representations, `(mime, bytes)` in the source's order.
    pub fn representations(&self, id: i64) -> Result<Option<Vec<Representation>>> {
        let key = self.sealer()?;
        let rows = self.raw_reprs(id)?;
        if rows.is_empty() && self.clip(id)?.is_none() {
            return Ok(None);
        }
        let mut reprs = Vec::with_capacity(rows.len());
        for (_, mime, inline, blob) in rows {
            reprs.push((mime, load_repr(&self.blobs, key, inline, blob)?));
        }
        Ok(Some(reprs))
    }

    /// `(ord, mime, inline, blob_hash)` of a Clip's representations, as stored.
    fn raw_reprs(&self, id: i64) -> Result<Vec<RawRepr>> {
        Ok(self
            .db
            .prepare_cached(
                "SELECT ord, mime, inline, blob_hash FROM reprs WHERE clip_id = ?1 ORDER BY ord",
            )?
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Keep `png` as a Clip's thumbnail, replacing any it had.
    pub fn set_thumb(&mut self, id: i64, png: &[u8]) -> Result<()> {
        let sealed = self.sealer()?.map(|key| key.seal(png)).transpose()?;
        self.db.execute(
            "INSERT OR REPLACE INTO thumbs(clip_id, png) VALUES (?1, ?2)",
            params![id, sealed.as_deref().unwrap_or(png)],
        )?;
        Ok(())
    }

    /// A Clip's thumbnail, if it has one.
    pub fn thumb(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let key = self.sealer()?;
        let png: Option<Vec<u8>> = self
            .db
            .prepare_cached("SELECT png FROM thumbs WHERE clip_id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?;
        match (png, key) {
            (Some(png), Some(key)) => key.open(&png).map(Some),
            (png, _) => Ok(png),
        }
    }

    /// Mark a Clip used at `now` (a Restore), moving it to the top.
    /// `false` if there is no such Clip.
    pub fn touch(&mut self, id: i64, now: i64) -> Result<bool> {
        let tx = self.db.transaction()?;
        let now = next_time(&tx, now)?;
        let n = tx.execute(
            "UPDATE clips SET last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// Delete a Clip, and any blob no other Clip uses. `false` if there is
    /// no such Clip.
    pub fn delete(&mut self, id: i64) -> Result<bool> {
        Ok(self.delete_all(&[id])? > 0)
    }

    /// Pin or unpin a Clip. `false` if there is no such Clip.
    pub fn pin(&mut self, id: i64, pinned: bool) -> Result<bool> {
        let n = self.db.execute(
            "UPDATE clips SET pinned = ?1 WHERE id = ?2",
            params![pinned, id],
        )?;
        Ok(n > 0)
    }

    /// Delete every unpinned Clip, and the pinned ones too if `pinned`.
    /// Returns how many went.
    pub fn clear(&mut self, pinned: bool) -> Result<usize> {
        let ids: Vec<i64> = self
            .db
            .prepare("SELECT id FROM clips WHERE ?1 OR NOT pinned")?
            .query_map([pinned], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        self.delete_all(&ids)
    }

    /// Apply the retention policy at `now` (Unix ms): evict what
    /// [`retention::evict`] picks. Returns the evicted ids.
    pub fn retain(&mut self, limits: &retention::Limits, now: i64) -> Result<Vec<i64>> {
        let entries: Vec<retention::Entry> = self
            .db
            .prepare_cached("SELECT id, last_used_at, pinned, size FROM clips")?
            .query_map([], |r| {
                Ok(retention::Entry {
                    id: r.get(0)?,
                    last_used_at: r.get(1)?,
                    pinned: r.get(2)?,
                    size: r.get::<_, i64>(3)? as u64,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let evicted = retention::evict(&entries, limits, now);
        self.delete_all(&evicted)?;
        Ok(evicted)
    }

    /// Delete Clips in one transaction, then the blobs no Clip uses any
    /// more. Returns how many Clips there were to delete.
    fn delete_all(&mut self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let tx = self.db.transaction()?;
        let mut blobs: Vec<Vec<u8>> = Vec::new();
        let mut deleted = 0;
        for id in ids {
            blobs.extend(
                tx.prepare_cached(
                    "SELECT DISTINCT blob_hash FROM reprs WHERE clip_id = ?1 AND blob_hash IS NOT NULL",
                )?
                .query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<Vec<u8>>>>()?,
            );
            deleted += tx.execute("DELETE FROM clips WHERE id = ?1", [id])?;
        }
        blobs.sort();
        blobs.dedup();
        let mut orphans = Vec::new();
        for hash in blobs {
            let users: i64 = tx.query_row(
                "SELECT count(*) FROM reprs WHERE blob_hash = ?1",
                [&hash],
                |r| r.get(0),
            )?;
            if users == 0 {
                orphans.push(hash);
            }
        }
        tx.commit()?;
        for hash in orphans {
            remove(&self.blob_path(&hash))?;
        }
        Ok(deleted)
    }

    fn blob_path(&self, hash: &[u8]) -> PathBuf {
        blob_path(&self.blobs, hash)
    }

    /// Rebuild the in-memory search indexes of an unlocked encrypted History
    /// from its sealed text. Deleting a Clip by any path drops its entries.
    fn index_in_memory(&self) -> Result<()> {
        let Some(key) = self.sealer()? else {
            return Ok(());
        };
        self.db.execute_batch(
            "DROP TABLE IF EXISTS temp.clips_fts;
             DROP TABLE IF EXISTS temp.clips_text;
             CREATE VIRTUAL TABLE temp.clips_fts USING fts5(preview, tokenize='trigram');
             CREATE VIRTUAL TABLE temp.clips_text USING fts5(body, tokenize='trigram');
             CREATE TEMP TRIGGER IF NOT EXISTS clips_mem_forget AFTER DELETE ON main.clips BEGIN
                 DELETE FROM clips_fts WHERE rowid = old.id;
                 DELETE FROM clips_text WHERE rowid = old.id;
             END;",
        )?;
        let mut previews = self
            .db
            .prepare("INSERT INTO temp.clips_fts(rowid, preview) VALUES (?1, ?2)")?;
        let mut texts = self
            .db
            .prepare("INSERT INTO temp.clips_text(rowid, body) VALUES (?1, ?2)")?;
        let mut sealed = self
            .db
            .prepare("SELECT id, sealed FROM clips WHERE sealed IS NOT NULL")?;
        let mut rows = sealed.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let (preview, text) = open_text(&key.open(&row.get::<_, Vec<u8>>(1)?)?)?;
            previews.execute(params![id, preview])?;
            texts.execute(params![id, text])?;
        }
        Ok(())
    }

    /// Re-store every Clip sealed with `to` (or plain for `None`), then
    /// scrub what the old form left behind: superseded blob files, freed
    /// database pages, the WAL and the on-disk search index.
    fn rewrite(&mut self, to: Option<Key>) -> Result<()> {
        let from = self.sealer()?.cloned();
        let (from, to) = (from.as_ref(), to.as_ref());
        let mut old_blobs = Vec::new();
        let tx = self.db.transaction()?;
        let ids: Vec<i64> = tx
            .prepare("SELECT id FROM clips")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in ids {
            let rows: Vec<RawRepr> = tx
                .prepare_cached(
                    "SELECT ord, mime, inline, blob_hash FROM reprs WHERE clip_id = ?1 ORDER BY ord",
                )?
                .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut plain = Vec::with_capacity(rows.len());
            for (ord, mime, inline, blob) in rows {
                old_blobs.extend(blob.clone());
                plain.push((ord, mime, load_repr(&self.blobs, from, inline, blob)?));
            }
            for (ord, _, data) in &plain {
                let (inline, blob) = store_repr(&self.blobs, to, data)?;
                tx.prepare_cached(
                    "UPDATE reprs SET inline = ?1, blob_hash = ?2 WHERE clip_id = ?3 AND ord = ?4",
                )?
                .execute(params![inline, blob, id, ord])?;
            }
            let reprs: Vec<(&str, &[u8])> =
                plain.iter().map(|(_, m, d)| (m.as_str(), &d[..])).collect();
            let (_, preview, hash, text) = clip::describe_with_text(&reprs, to);
            // The on-disk search text goes when sealing, comes back when not.
            tx.prepare_cached("DELETE FROM main.clips_text WHERE rowid = ?1")?
                .execute([id])?;
            let (plain_preview, sealed) = match to {
                Some(key) => (String::new(), Some(key.seal(&seal_text(&preview, &text))?)),
                None => {
                    tx.prepare_cached("INSERT INTO main.clips_text(rowid, body) VALUES (?1, ?2)")?
                        .execute(params![id, text])?;
                    (preview, None)
                }
            };
            tx.prepare_cached(
                "UPDATE clips SET preview = ?1, sealed = ?2, hash = ?3 WHERE id = ?4",
            )?
            .execute(params![plain_preview, sealed, &hash[..], id])?;
        }
        let thumbs: Vec<(i64, Vec<u8>)> = tx
            .prepare("SELECT clip_id, png FROM thumbs")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, png) in thumbs {
            let png = from.map_or(Ok(png.clone()), |k| k.open(&png))?;
            let png = to.map_or(Ok(png.clone()), |k| k.seal(&png))?;
            tx.execute(
                "UPDATE thumbs SET png = ?1 WHERE clip_id = ?2",
                params![png, id],
            )?;
        }
        tx.execute("DELETE FROM encryption", [])?;
        let recipient = to.map(Key::recipient);
        if let Some(recipient) = &recipient {
            tx.execute(
                "INSERT INTO encryption(id, recipient) VALUES (1, ?1)",
                [recipient],
            )?;
        }
        // The on-disk preview index is rebuilt from `preview`, empty when
        // sealed; merging the text index's segments drops deleted entries.
        tx.execute_batch(
            "INSERT INTO main.clips_fts(clips_fts) VALUES ('rebuild');
             INSERT INTO main.clips_text(clips_text) VALUES ('optimize');",
        )?;
        tx.commit()?;
        self.recipient = recipient;
        self.key = to.cloned();
        for hash in old_blobs {
            let users: i64 = self.db.query_row(
                "SELECT count(*) FROM reprs WHERE blob_hash = ?1",
                [&hash],
                |r| r.get(0),
            )?;
            if users == 0 {
                remove(&self.blob_path(&hash))?;
            }
        }
        if self.key.is_some() {
            self.index_in_memory()?;
        } else {
            self.db.execute_batch(
                "DROP TRIGGER IF EXISTS temp.clips_mem_forget;
                 DROP TABLE IF EXISTS temp.clips_fts;
                 DROP TABLE IF EXISTS temp.clips_text;",
            )?;
        }
        // Fold the WAL (which holds the old pages) in, rewrite the file
        // without its free pages, and empty the WAL again.
        self.db.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
        )?;
        Ok(())
    }

    /// Delete blob files no representation refers to: leftovers of a crash
    /// between writing a blob and committing, or between an encryption
    /// change and its clean-up (which could otherwise leave plaintext).
    fn sweep_blobs(&self) -> Result<()> {
        let used: std::collections::HashSet<PathBuf> = self
            .db
            .prepare("SELECT DISTINCT blob_hash FROM reprs WHERE blob_hash IS NOT NULL")?
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .map(|h| h.map(|h| self.blob_path(&h)))
            .collect::<rusqlite::Result<_>>()?;
        for entry in std::fs::read_dir(&self.blobs)? {
            let path = entry?.path();
            if !used.contains(&path) {
                remove(&path)?;
            }
        }
        Ok(())
    }
}

/// A Clip's preview and search text, as one buffer to seal.
fn seal_text(preview: &str, text: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + preview.len() + text.len());
    buf.extend_from_slice(&(preview.len() as u32).to_le_bytes());
    buf.extend_from_slice(preview.as_bytes());
    buf.extend_from_slice(text.as_bytes());
    buf
}

/// The preview and search text [`seal_text`] put together.
fn open_text(buf: &[u8]) -> Result<(String, String)> {
    let bad = || Error::Crypto("malformed sealed text".into());
    let (len, rest) = buf.split_first_chunk::<4>().ok_or_else(bad)?;
    let len = u32::from_le_bytes(*len) as usize;
    if len > rest.len() {
        return Err(bad());
    }
    let (preview, text) = rest.split_at(len);
    Ok((
        String::from_utf8_lossy(preview).into_owned(),
        String::from_utf8_lossy(text).into_owned(),
    ))
}

/// The `inline` and `blob_hash` columns of one representation.
type StoredRepr = (Option<Vec<u8>>, Option<Vec<u8>>);

/// `(ord, mime, inline, blob_hash)`, as stored.
type RawRepr = (i64, String, Option<Vec<u8>>, Option<Vec<u8>>);

/// Store one representation, sealed with `key` if given: inline if small,
/// else as a blob file. Returns the `inline` and `blob_hash` columns.
fn store_repr(blobs: &Path, key: Option<&Key>, data: &[u8]) -> Result<StoredRepr> {
    // Decided on the plain size, so sealing never moves the line.
    if data.len() <= INLINE_MAX {
        let inline = match key {
            Some(key) => key.seal(data)?,
            None => data.to_vec(),
        };
        return Ok((Some(inline), None));
    }
    let hash = crypt::hash(key, data);
    if !blob_path(blobs, &hash).exists() {
        match key {
            Some(key) => write_blob(blobs, &hash, &key.seal(data)?)?,
            None => write_blob(blobs, &hash, data)?,
        }
    }
    Ok((None, Some(hash.to_vec())))
}

/// The plain bytes of one stored representation.
fn load_repr(
    blobs: &Path,
    key: Option<&Key>,
    inline: Option<Vec<u8>>,
    blob: Option<Vec<u8>>,
) -> Result<Vec<u8>> {
    let stored = match (inline, blob) {
        (Some(data), _) => data,
        (None, Some(hash)) => std::fs::read(blob_path(blobs, &hash))?,
        (None, None) => return Ok(Vec::new()),
    };
    match key {
        Some(key) => key.open(&stored),
        None => Ok(stored),
    }
}

/// Remove a file that may already be gone.
fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn blob_path(blobs: &Path, hash: &[u8]) -> PathBuf {
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    blobs.join(hex)
}

/// Write a blob file unless it is already there (content-addressed).
fn write_blob(blobs: &Path, hash: &[u8], data: &[u8]) -> Result<()> {
    let path = blob_path(blobs, hash);
    if path.exists() {
        return Ok(());
    }
    let tmp = path.with_extension("part");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(data)?;
    file.sync_data()?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Create `dir` (and its parents) and make it 0700.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, Permissions::from_mode(0o700))
}

/// `now`, or just after the latest use if the clock has not moved past it,
/// so the most recent Clip is always strictly on top.
fn next_time(tx: &Transaction<'_>, now: i64) -> Result<i64> {
    let latest: Option<i64> =
        tx.query_row("SELECT max(last_used_at) FROM clips", [], |r| r.get(0))?;
    Ok(latest.map_or(now, |l| now.max(l + 1)))
}

/// Apply the migrations not yet applied, each in its own transaction.
fn migrate(db: &mut Connection) -> Result<()> {
    let applied: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (n, sql) in MIGRATIONS.iter().enumerate().skip(applied.max(0) as usize) {
        let tx = db.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", n as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A History written before the `clips_text` index (schema version 1)
    /// upgrades in place: old Clips become searchable, deletes reach the new
    /// index, and new Clips land in it too.
    #[test]
    fn a_version_1_history_migrates_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Connection::open(dir.path().join("history.db")).unwrap();
            db.execute_batch(MIGRATIONS[0]).unwrap();
            db.pragma_update(None, "user_version", 1).unwrap();
            for (id, preview) in [(1, "an old needle"), (2, "another old needle")] {
                db.execute(
                    "INSERT INTO clips(id, created_at, last_used_at, kind, preview, size, hash)
                     VALUES (?1, ?1, ?1, 'text', ?2, ?3, ?4)",
                    (id, preview, preview.len() as i64, [id as u8; 32]),
                )
                .unwrap();
                db.execute(
                    "INSERT INTO reprs(clip_id, ord, mime, inline) VALUES (?1, 0, 'text/plain', ?2)",
                    (id, preview.as_bytes()),
                )
                .unwrap();
            }
        }

        let mut store = Store::open(dir.path()).unwrap();
        let version: i64 = store
            .db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);

        let ids = |store: &Store| -> Vec<i64> {
            store
                .search("needle", 10)
                .unwrap()
                .into_iter()
                .map(|c| c.id)
                .collect()
        };
        assert_eq!(ids(&store), [2, 1]);

        assert!(store.delete(1).unwrap());
        assert_eq!(ids(&store), [2]);

        let new = store
            .record(&[("text/plain", b"a new needle")], None, 3)
            .unwrap()
            .id;
        assert_eq!(ids(&store), [new, 2]);
    }

    /// Searches go through `clips_fts`, which under encryption must be the
    /// in-memory index of the plain previews (the one on disk is empty),
    /// and must forget Clips however they are deleted.
    #[test]
    fn an_unlocked_encrypted_history_stays_searchable_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let old = store
            .record(&[("text/plain", b"the vault code")], None, 1)
            .unwrap()
            .id;
        store.encrypt(Key::generate()).unwrap();
        let new = store
            .record(&[("text/plain", b"another vault")], None, 2)
            .unwrap()
            .id;
        let hits = |store: &Store| -> Vec<i64> {
            store
                .db
                .prepare("SELECT rowid FROM clips_fts WHERE clips_fts MATCH 'vault' ORDER BY rowid")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(hits(&store), [old, new]);
        // Retention deletes in plain SQL; the index must follow.
        store
            .db
            .execute("DELETE FROM clips WHERE id = ?1", [old])
            .unwrap();
        assert_eq!(hits(&store), [new]);
    }

    /// A History at schema version 2 (with `clips_text`, before encryption)
    /// upgrades in place: plain and searchable as before, and it can then
    /// be encrypted, after which search still finds the old Clip.
    #[test]
    fn a_version_2_history_migrates_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Connection::open(dir.path().join("history.db")).unwrap();
            db.execute_batch(MIGRATIONS[0]).unwrap();
            db.execute_batch(MIGRATIONS[1]).unwrap();
            db.pragma_update(None, "user_version", 2).unwrap();
            let text = "a version two needle";
            let (_, _, hash) = clip::describe(&[("text/plain", text.as_bytes())], None);
            db.execute(
                "INSERT INTO clips(id, created_at, last_used_at, kind, preview, size, hash)
                 VALUES (1, 1, 1, 'text', ?1, ?2, ?3)",
                (text, text.len() as i64, &hash[..]),
            )
            .unwrap();
            db.execute("INSERT INTO clips_text(rowid, body) VALUES (1, ?1)", [text])
                .unwrap();
            db.execute(
                "INSERT INTO reprs(clip_id, ord, mime, inline) VALUES (1, 0, 'text/plain', ?1)",
                [text.as_bytes()],
            )
            .unwrap();
        }

        let mut store = Store::open(dir.path()).unwrap();
        let version: i64 = store
            .db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        assert!(!store.is_encrypted());
        let ids = |store: &Store| -> Vec<i64> {
            store
                .search("two needle", 10)
                .unwrap()
                .into_iter()
                .map(|c| c.id)
                .collect()
        };
        assert_eq!(ids(&store), [1]);

        store.encrypt(Key::generate()).unwrap();
        assert_eq!(ids(&store), [1]);
        assert_eq!(store.list(10).unwrap()[0].preview, "a version two needle");
        // The same content is still one Clip, under its new keyed hash.
        let again = store
            .record(&[("text/plain", b"a version two needle")], None, 5)
            .unwrap();
        assert_eq!((again.id, again.new), (1, false));
    }
}
