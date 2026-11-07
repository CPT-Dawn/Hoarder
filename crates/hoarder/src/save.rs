//! Saving a Capture to `screenshot.dir` under the `screenshot.name` template
//! (PLAN.md §4.1 step 7, §4.5).

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use hoarder_config::{NameFields, Screenshot};

use crate::Exit;
use crate::image::Image;

/// A [`save`] running on its own thread.
#[derive(Debug)]
pub struct Saving(std::thread::JoinHandle<Result<PathBuf, (Exit, String)>>);

impl Saving {
    /// Wait for the file; returns its path.
    pub fn wait(self) -> Result<PathBuf, (Exit, String)> {
        self.0
            .join()
            .unwrap_or_else(|_| Err((Exit::Io, "the saving thread panicked".into())))
    }
}

/// Start saving `image` on a worker thread and return at once, so slow formats
/// (AVIF takes seconds at 4K) never hold up the clipboard copy.
pub fn spawn(
    image: Arc<Image>,
    target: &'static str,
    app: Option<String>,
    output: Option<String>,
    settings: Screenshot,
) -> Saving {
    Saving(std::thread::spawn(move || {
        save(&image, target, app.as_deref(), output.as_deref(), &settings)
    }))
}

/// Encode `image` and write it to a new file in `settings.dir`; returns its path.
/// A name that is already taken gets `_1`, `_2`, … rather than overwriting.
/// With `output` (one Part of a split Capture), the name ends in `_OUTPUT`.
pub fn save(
    image: &Image,
    target: &str,
    app: Option<&str>,
    output: Option<&str>,
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
    let stem = match output {
        Some(output) => format!("{stem}_{output}"),
        None => stem,
    };
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

#[cfg(test)]
mod tests {
    use hoarder_config::{Format, Screenshot};

    use super::*;
    use crate::image::Depth;

    /// The saved-file encode runs off the calling thread, so the clipboard copy
    /// that follows it in `shot` is never kept waiting (PLAN.md §4.1 step 7).
    #[test]
    fn spawn_returns_before_the_file_is_encoded() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Screenshot {
            dir: dir.path().to_str().unwrap().into(),
            format: Format::Avif,
            ..Screenshot::default()
        };
        let image = Image::new(320, 240, Depth::Eight);

        let start = std::time::Instant::now();
        let saving = spawn(std::sync::Arc::new(image), "all", None, None, settings);
        let returned = start.elapsed();
        let path = saving.wait().unwrap();
        let saved = start.elapsed();

        assert!(
            returned * 10 < saved,
            "spawn took {returned:?} of the {saved:?} the save took"
        );
        assert_eq!(path.extension().unwrap(), "avif");
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }
}
