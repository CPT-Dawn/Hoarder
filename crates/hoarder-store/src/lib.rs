//! SQLite-backed History store for Hoarder: schema, blobs, dedup, retention and search.
//!
//! One [`Store`] is one History directory (`$XDG_DATA_HOME/hoarder/`, PLAN.md
//! §5.3): `history.db` plus `blobs/<blake3>` for representations over
//! [`INLINE_MAX`]. The directories are mode 0700 and every file 0600.
//!
//! A [`Connection`](rusqlite::Connection) is `Send` but not `Sync`: own the
//! Store on one thread and send it work.

mod clip;

use std::fs::{DirBuilder, OpenOptions, Permissions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension as _, Transaction, params};

pub use clip::Kind;

/// Representations up to this size live in the database; larger ones are
/// blob files.
pub const INLINE_MAX: usize = 256 * 1024;

/// The schema migrations, in order; `user_version` counts those applied.
const MIGRATIONS: &[&str] = &[r#"
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
"#];

/// Why the Store failed.
#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "History database: {e}"),
            Error::Io(e) => write!(f, "History files: {e}"),
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
}

impl Store {
    /// Open (or create) the History in `dir`, upgrading its schema.
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
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )?;
        migrate(&mut db)?;
        Ok(Store { db, blobs })
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
        let (kind, preview, hash) = clip::describe(reprs);
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
        tx.execute(
            "INSERT INTO clips(created_at, last_used_at, source_app, kind, preview, size, hash)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                now,
                source_app,
                kind.as_str(),
                preview,
                size as i64,
                &hash[..]
            ],
        )?;
        let id = tx.last_insert_rowid();
        for (ord, (mime, data)) in reprs.iter().enumerate() {
            let (inline, blob) = if data.len() <= INLINE_MAX {
                (Some(*data), None)
            } else {
                let hash = blake3::hash(data);
                write_blob(&self.blobs, hash.as_bytes(), data)?;
                (None, Some(hash.as_bytes().to_vec()))
            };
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

    /// One Clip's metadata.
    pub fn clip(&self, id: i64) -> Result<Option<Clip>> {
        let clip = self
            .db
            .prepare_cached(
                "SELECT created_at, last_used_at, pinned, source_app, kind, preview, size
                 FROM clips WHERE id = ?1",
            )?
            .query_row([id], |r| {
                Ok(Clip {
                    id,
                    created_at: r.get(0)?,
                    last_used_at: r.get(1)?,
                    pinned: r.get(2)?,
                    source_app: r.get(3)?,
                    kind: Kind::parse(&r.get::<_, String>(4)?),
                    preview: r.get(5)?,
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
        let rows = self
            .db
            .prepare_cached(
                "SELECT mime, inline, blob_hash FROM reprs WHERE clip_id = ?1 ORDER BY ord",
            )?
            .query_map([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<Vec<u8>>>(1)?,
                    r.get::<_, Option<Vec<u8>>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() && self.clip(id)?.is_none() {
            return Ok(None);
        }
        let mut reprs = Vec::with_capacity(rows.len());
        for (mime, inline, blob) in rows {
            let data = match (inline, blob) {
                (Some(data), _) => data,
                (None, Some(hash)) => std::fs::read(self.blob_path(&hash))?,
                (None, None) => Vec::new(),
            };
            reprs.push((mime, data));
        }
        Ok(Some(reprs))
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
        let tx = self.db.transaction()?;
        let blobs: Vec<Vec<u8>> = tx
            .prepare(
                "SELECT DISTINCT blob_hash FROM reprs WHERE clip_id = ?1 AND blob_hash IS NOT NULL",
            )?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let n = tx.execute("DELETE FROM clips WHERE id = ?1", [id])?;
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
            match std::fs::remove_file(self.blob_path(&hash)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(n > 0)
    }

    fn blob_path(&self, hash: &[u8]) -> PathBuf {
        blob_path(&self.blobs, hash)
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
