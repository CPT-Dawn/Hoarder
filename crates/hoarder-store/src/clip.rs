//! What a Clip is, judged from its representations alone: its kind, the
//! canonical representation its identity is hashed from, and a preview.

/// What a Clip holds, by its most telling representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
    /// A file list (`text/uri-list`), as file managers copy it.
    Files,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Image => "image",
            Kind::Files => "files",
            Kind::Other => "other",
        }
    }

    pub(crate) fn parse(s: &str) -> Kind {
        match s {
            "text" => Kind::Text,
            "image" => Kind::Image,
            "files" => Kind::Files,
            _ => Kind::Other,
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The plain-text types, best first.
const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

const EMPTY: (&str, &[u8]) = ("", &[]);

/// Previews are cut to this many characters.
const PREVIEW_CHARS: usize = 200;

/// A Clip's kind, preview, and the blake3 hash of its canonical
/// representation (prefixed by the kind), which is its identity for dedup.
pub(crate) fn describe(reprs: &[(&str, &[u8])]) -> (Kind, String, [u8; 32]) {
    let find = |pred: &dyn Fn(&str) -> bool| reprs.iter().find(|(m, _)| pred(m));
    let (kind, canonical) = if let Some(r) = find(&|m| m == "text/uri-list")
        .filter(|(_, d)| uris(d).next().is_some())
    {
        (Kind::Files, r)
    } else if let Some(r) =
        find(&|m| m == "image/png").or_else(|| find(&|m| m.starts_with("image/")))
    {
        (Kind::Image, r)
    } else if let Some(r) = TEXT_TYPES
        .iter()
        .find_map(|t| find(&|m| m.eq_ignore_ascii_case(t)))
        .or_else(|| find(&|m| m.starts_with("text/")))
    {
        (Kind::Text, r)
    } else {
        (Kind::Other, reprs.first().unwrap_or(&EMPTY))
    };
    let (mime, data) = *canonical;
    let preview = match kind {
        Kind::Text => text_preview(data),
        Kind::Files => uris(data).map(file_name).collect::<Vec<_>>().join(", "),
        Kind::Image => png_size(data)
            .map(|(w, h)| format!("{w}×{h}"))
            .unwrap_or_else(|| mime.to_owned()),
        Kind::Other => mime.to_owned(),
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(data);
    (kind, preview, *hasher.finalize().as_bytes())
}

/// The first line-ish of text, whitespace collapsed, cut short.
fn text_preview(data: &[u8]) -> String {
    let text = String::from_utf8_lossy(data);
    let mut preview = String::new();
    for (i, word) in text.split_whitespace().enumerate() {
        if i > 0 {
            preview.push(' ');
        }
        preview.push_str(word);
        if preview.chars().count() > PREVIEW_CHARS {
            break;
        }
    }
    if preview.chars().count() > PREVIEW_CHARS {
        preview = preview.chars().take(PREVIEW_CHARS - 1).collect();
        preview.push('…');
    }
    preview
}

/// The URIs of a `text/uri-list` (RFC 2483): one per line, `#` comments.
fn uris(data: &[u8]) -> impl Iterator<Item = &str> {
    std::str::from_utf8(data)
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// The last path segment of a URI, percent-decoded.
fn file_name(uri: &str) -> String {
    let segment = uri.trim_end_matches('/').rsplit('/').next().unwrap_or(uri);
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Width and height from a PNG's IHDR.
fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| u32::from_be_bytes(data[at..at + 4].try_into().expect("4 bytes"));
    Some((be(16), be(20)))
}
