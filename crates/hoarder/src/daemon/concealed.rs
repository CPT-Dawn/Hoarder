//! Concealed Clips (ADR-0006): held in memory only, in pages locked out of
//! swap, wiped when dropped, and forgotten after `privacy.concealed_ttl_secs`.
//! They never go near the Store.

use std::sync::Arc;

use zeroize::Zeroize as _;

use super::Representation;

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
    /// new Clip's id. The Daemon forgets it when its TTL is up.
    pub fn keep(&mut self, reprs: &mut Vec<Representation>) -> u64 {
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
        self.clips.push(Clip { id, reprs: locked });
        id
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vault_serves_what_it_keeps_and_wipes_the_originals() {
        let mut vault = Vault::default();
        let original: Arc<[u8]> = Arc::from(b"hunter2".as_slice());
        let mut reprs = vec![("text/plain".to_string(), original)];
        let id = vault.keep(&mut reprs);

        assert!(reprs.is_empty());
        assert_eq!(vault.types(id), ["text/plain"]);
        assert_eq!(vault.get(id, "text/plain").unwrap().bytes(), b"hunter2");
        assert!(vault.get(id, "text/html").is_none());

        vault.forget(id);
        assert!(vault.types(id).is_empty());
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
