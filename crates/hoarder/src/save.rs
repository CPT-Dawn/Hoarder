//! Saving a Capture to `screenshot.dir` under the `screenshot.name` template
//! (PLAN.md §4.1 step 7, §4.5).

use std::io::Write as _;
use std::path::PathBuf;

use hoarder_config::{NameFields, Screenshot};

use crate::Exit;
use crate::image::Image;

/// Encode `image` and write it to a new file in `settings.dir`; returns its path.
/// A name that is already taken gets `_1`, `_2`, … rather than overwriting.
pub fn save(
    image: &Image,
    target: &str,
    app: Option<&str>,
    settings: &Screenshot,
) -> Result<PathBuf, (Exit, String)> {
    let dir = hoarder_config::expand_dir(&settings.dir)
        .map_err(|e| (Exit::Io, format!("screenshot.dir: {e}")))?;
    if crate::encode::narrows(image, settings.format) {
        eprintln!(
            "hoarder: {} keeps 8 bits per channel; use png to keep 10-bit detail",
            settings.format.extension()
        );
    }
    let stem = settings.name.expand(&NameFields {
        time: &jiff::Zoned::now(),
        target,
        app,
        width: image.width,
        height: image.height,
    });
    let mut bytes = Vec::new();
    crate::encode::encode(image, settings.format, settings, &mut bytes)
        .map_err(|e| (Exit::Io, e))?;
    let io =
        |e: std::io::Error, p: &PathBuf| (Exit::Io, format!("cannot write {}: {e}", p.display()));
    std::fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
    let ext = settings.format.extension();
    for n in 0u32.. {
        let name = match n {
            0 => format!("{stem}.{ext}"),
            n => format!("{stem}_{n}.{ext}"),
        };
        let path = dir.join(name);
        match std::fs::File::create_new(&path) {
            Ok(mut file) => {
                file.write_all(&bytes).map_err(|e| io(e, &path))?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io(e, &path)),
        }
    }
    unreachable!("ran out of file names")
}
