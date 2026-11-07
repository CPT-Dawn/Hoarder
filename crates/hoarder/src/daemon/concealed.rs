//! Concealed Clips (ADR-0006): held in memory only, in pages locked out of
//! swap, wiped when dropped, and forgotten after `privacy.concealed_ttl_secs`.
//! They never go near the Store.

use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use hoarder_ipc::{ClipSummary, ErrorCode, Failure, Got, Sensitivity, Value};
use hoarder_wayland::clipboard::Selection;
use zeroize::Zeroize as _;

use super::Representation;
use super::history::to_value;

/// Bytes in their own locked pages, zeroed before they are freed.
pub struct Locked {
    /// Unlocks on drop, after [`Drop`] has zeroed `buf`.
    _guard: Option<region::LockGuard>,
    /// Page-aligned room for the bytes, so no page is shared with anything
    /// else and unlocking ours never unlocks someone else's.
    buf: Vec<u8>,
    start: usize,
    len: usize,
}

impl Locked {
    /// Copy `data` into locked memory. If the kernel refuses to lock it
    /// (`RLIMIT_MEMLOCK`), the bytes are still held and wiped, just not
    /// pinned; the second value says so.
    fn new(data: &[u8]) -> (Locked, bool) {
        let page = region::page::size();
        let mut buf = vec![0u8; data.len() + 2 * page];
        let start = buf.as_ptr().align_offset(page).min(page);
        buf[start..start + data.len()].copy_from_slice(data);
        let guard = (!data.is_empty())
            .then(|| region::lock(buf[start..].as_ptr(), data.len()).ok())
            .flatten();
        let locked = guard.is_some() || data.is_empty();
        let bytes = Locked {
            _guard: guard,
            buf,
            start,
            len: data.len(),
        };
        (bytes, locked)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf[self.start..self.start + self.len]
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        self.buf.zeroize();
    }
}

impl std::fmt::Debug for Locked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Locked({} concealed bytes)", self.len)
    }
}

/// One Concealed Clip.
#[derive(Debug)]
struct Clip {
    id: u64,
    reprs: Vec<(String, Arc<Locked>)>,
    /// The app focused when it was copied, if known.
    app: Option<String>,
    /// When it was copied, in ms since the Unix epoch.
    at_ms: i64,
    /// When it is forgotten, in ms since the Unix epoch.
    expires_ms: i64,
}

/// A Concealed Clip as Front ends may see it: everything but its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: u64,
    pub types: Vec<String>,
    pub app: Option<String>,
    pub at_ms: i64,
    pub expires_ms: i64,
    pub size: u64,
}

/// What every Front end shows in place of a Concealed Clip's text.
pub const MASK: &str = "••••••••";

/// The IPC id of Concealed Clip `id`: negative, so it never collides with
/// a Clip in History.
pub fn ipc_id(id: u64) -> i64 {
    -(id as i64) - 1
}

/// The vault id behind IPC id `clip`, if it names a Concealed Clip.
pub fn vault_id(clip: i64) -> Option<u64> {
    (clip < 0).then(|| (-(clip + 1)) as u64)
}

/// Every live Concealed Clip.
#[derive(Debug, Default)]
pub struct Vault {
    clips: Vec<Clip>,
    next_id: u64,
    warned: bool,
}

impl Vault {
    /// Move `reprs` into locked memory, wiping the originals, and return the
    /// new Clip's id. The Daemon forgets it when `ttl_ms` is up; `app` and
    /// `now_ms` (when it was copied) are for listing it.
    pub fn keep(
        &mut self,
        reprs: &mut Vec<Representation>,
        app: Option<String>,
        now_ms: i64,
        ttl_ms: i64,
    ) -> u64 {
        let mut all_locked = true;
        let locked = reprs
            .iter()
            .map(|(mime, data)| {
                let (bytes, ok) = Locked::new(data);
                all_locked &= ok;
                (mime.clone(), Arc::new(bytes))
            })
            .collect();
        scrub(reprs);
        if !all_locked && !std::mem::replace(&mut self.warned, true) {
            eprintln!(
                "hoarder: cannot lock Concealed Clips in memory (RLIMIT_MEMLOCK); \
                 they are still kept off disk and wiped"
            );
        }
        let id = self.next_id;
        self.next_id += 1;
        self.clips.push(Clip {
            id,
            reprs: locked,
            app,
            at_ms: now_ms,
            expires_ms: now_ms.saturating_add(ttl_ms),
        });
        id
    }

    /// Every Concealed Clip not yet expired at `now_ms`, newest first,
    /// without its bytes.
    pub fn list(&self, now_ms: i64) -> Vec<Listed> {
        self.clips
            .iter()
            .rev()
            .filter(|c| c.expires_ms > now_ms)
            .map(|c| Listed {
                id: c.id,
                types: c.reprs.iter().map(|(m, _)| m.clone()).collect(),
                app: c.app.clone(),
                at_ms: c.at_ms,
                expires_ms: c.expires_ms,
                size: c.reprs.iter().map(|(_, d)| d.bytes().len() as u64).sum(),
            })
            .collect()
    }

    /// Whether Clip `id` is still held.
    pub fn holds(&self, id: u64) -> bool {
        self.clip(id).is_some()
    }

    /// The types Clip `id` holds, in the source's order.
    pub fn types(&self, id: u64) -> Vec<String> {
        self.clip(id)
            .map(|c| c.reprs.iter().map(|(m, _)| m.clone()).collect())
            .unwrap_or_default()
    }

    /// Clip `id`'s bytes for `mime_type`.
    pub fn get(&self, id: u64, mime_type: &str) -> Option<Arc<Locked>> {
        self.clip(id)?
            .reprs
            .iter()
            .find(|(m, _)| m == mime_type)
            .map(|(_, d)| Arc::clone(d))
    }

    /// Drop Clip `id`; its bytes are wiped once no paste is still sending
    /// them.
    pub fn forget(&mut self, id: u64) {
        self.clips.retain(|c| c.id != id);
    }

    fn clip(&self, id: u64) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }
}

/// Zero every representation nobody else holds, and empty the list.
pub fn scrub(reprs: &mut Vec<Representation>) {
    for (_, data) in reprs.iter_mut() {
        if let Some(bytes) = Arc::get_mut(data) {
            bytes.zeroize();
        }
    }
    reprs.clear();
}

/// Append `more` to `buf` without leaving copies behind: when `buf` has to
/// grow, the old allocation is zeroed before it is freed.
pub fn extend_scrubbed(buf: &mut Vec<u8>, more: &[u8]) {
    if buf.capacity() - buf.len() < more.len() {
        let wanted = (buf.len() + more.len()).max(buf.capacity() * 2);
        let mut grown = Vec::with_capacity(wanted);
        grown.extend_from_slice(buf);
        let mut old = std::mem::replace(buf, grown);
        old.zeroize();
    }
    buf.extend_from_slice(more);
}

/// What Front ends may do with Concealed Clips, by their (negative) IPC ids.
impl super::Daemon {
    /// Every live Concealed Clip, masked, with when it was copied: what
    /// `list` with `concealed` merges into History.
    pub(super) fn concealed_summaries(&self) -> Vec<(i64, ClipSummary)> {
        let now = jiff::Timestamp::now().as_millisecond();
        self.vault
            .list(now)
            .into_iter()
            .map(|c| (c.at_ms, summary(c)))
            .collect()
    }

    /// `get`: stream a Concealed Clip's `mime` (or its first type) into
    /// `pipe`, off-thread.
    pub(super) fn get_concealed(
        &self,
        id: u64,
        mime: Option<String>,
        pipe: OwnedFd,
    ) -> Result<Value, Failure> {
        let now = jiff::Timestamp::now().as_millisecond();
        let listed = (self.vault.list(now).into_iter())
            .find(|c| c.id == id)
            .ok_or_else(|| not_found(id))?;
        let mime = match mime {
            Some(m) => m,
            None => listed.types.first().cloned().ok_or_else(|| not_found(id))?,
        };
        let data = self.vault.get(id, &mime).ok_or_else(|| Failure {
            code: ErrorCode::NotFound,
            message: format!("Clip {} has no {mime} representation", ipc_id(id)),
        })?;
        let got = Got {
            clip: summary(listed),
            mime,
            size: data.bytes().len() as u64,
        };
        std::thread::spawn(move || {
            // A reader that hangs up early is its own business.
            let _ = std::fs::File::from(pipe).write_all(data.bytes());
        });
        Ok(to_value(&got))
    }

    /// `restore` of a Concealed Clip: serve it as the clipboard (only its
    /// plain text if `plain`). It stays Concealed, so it still expires.
    pub(super) fn restore_concealed(&mut self, client: u64, id: u64, clip: i64, plain: bool) {
        let now = jiff::Timestamp::now().as_millisecond();
        let listed =
            vault_id(clip).and_then(|v| self.vault.list(now).into_iter().find(|c| c.id == v));
        let Some(listed) = listed else {
            return self.reply(client, id, Err(not_found_ipc(clip)));
        };
        let mut types = listed.types.clone();
        if plain {
            types.retain(|m| super::is_plain_text(m));
            if types.is_empty() {
                let failure = super::history::no_plain_text(clip);
                return self.reply(client, id, Err(failure));
            }
        }
        let which = Selection::Clipboard;
        self.stop_reading(which);
        let slot = &mut self.slots[which as usize];
        scrub(&mut slot.kept);
        slot.may_take_over = true;
        slot.record = false;
        slot.once = false;
        slot.sensitivity = Sensitivity::Concealed;
        slot.concealed = Some(listed.id);
        slot.take_over_after_read = false;
        self.watcher.serve(which, &types);
        self.reply_when_synced(client, id, to_value(&summary(listed)));
    }

    /// `delete` of a Concealed Clip: forget it now, as if it had expired.
    pub(super) fn delete_concealed(&mut self, clip: i64) -> Result<Value, Failure> {
        let id = vault_id(clip)
            .filter(|&v| self.vault.holds(v))
            .ok_or_else(|| not_found_ipc(clip))?;
        self.concealed_expired(id);
        Ok(Value::Null)
    }
}

/// A Concealed Clip as `list` and `get` show it: masked.
fn summary(c: Listed) -> ClipSummary {
    let time = |ms: i64| {
        jiff::Timestamp::from_millisecond(ms)
            .map(|t| t.to_string())
            .unwrap_or_default()
    };
    let kind = if c.types.iter().any(|m| m.starts_with("image/")) {
        "image"
    } else if c
        .types
        .iter()
        .any(|m| m.starts_with("text/") || super::is_plain_text(m))
    {
        "text"
    } else {
        "other"
    };
    ClipSummary {
        id: ipc_id(c.id),
        kind: kind.into(),
        preview: MASK.into(),
        app: c.app,
        at: time(c.at_ms),
        created: time(c.at_ms),
        pinned: false,
        size: c.size,
        types: c.types,
        sensitivity: Sensitivity::Concealed,
        expires: Some(time(c.expires_ms)),
    }
}

fn not_found(id: u64) -> Failure {
    not_found_ipc(ipc_id(id))
}

fn not_found_ipc(clip: i64) -> Failure {
    Failure {
        code: ErrorCode::NotFound,
        message: format!("no Concealed Clip {clip}; it may have expired"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vault_serves_what_it_keeps_and_wipes_the_originals() {
        let mut vault = Vault::default();
        let original: Arc<[u8]> = Arc::from(b"hunter2".as_slice());
        let mut reprs = vec![("text/plain".to_string(), original)];
        let id = vault.keep(&mut reprs, Some("kitty".into()), 1_000, 120_000);

        assert!(reprs.is_empty());
        let listed = vault.list(2_000);
        assert_eq!(
            listed,
            [Listed {
                id,
                types: vec!["text/plain".into()],
                app: Some("kitty".into()),
                at_ms: 1_000,
                expires_ms: 121_000,
                size: 7,
            }]
        );
        assert!(vault.list(121_000).is_empty(), "listed past its expiry");
        assert_eq!(vault.types(id), ["text/plain"]);
        assert_eq!(vault.get(id, "text/plain").unwrap().bytes(), b"hunter2");
        assert!(vault.get(id, "text/html").is_none());

        vault.forget(id);
        assert!(vault.types(id).is_empty());
    }

    #[test]
    fn concealed_ids_are_negative_and_round_trip() {
        assert_eq!(ipc_id(0), -1);
        assert_eq!(vault_id(ipc_id(41)), Some(41));
        assert_eq!(vault_id(7), None);
        assert_eq!(vault_id(0), None);
    }

    #[test]
    fn growing_a_scrubbed_buffer_keeps_every_byte() {
        let mut buf = Vec::new();
        for chunk in [&b"abc"[..], b"defgh", b"", b"ijklmnopqrstuvwxyz"] {
            extend_scrubbed(&mut buf, chunk);
        }
        assert_eq!(buf, b"abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn locked_bytes_never_print() {
        let (bytes, _) = Locked::new(b"hunter2");
        assert!(!format!("{bytes:?}").contains("hunter2"));
    }
}
