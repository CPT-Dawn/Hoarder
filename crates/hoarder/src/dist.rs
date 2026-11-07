//! `hoarder completions` and the files a package ships besides the binary.

use std::io::Write;
use std::path::Path;

use clap::ValueEnum;

use crate::Exit;

/// The shells `hoarder completions` writes for.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl From<Shell> for clap_complete::Shell {
    fn from(shell: Shell) -> Self {
        match shell {
            Shell::Bash => Self::Bash,
            Shell::Zsh => Self::Zsh,
            Shell::Fish => Self::Fish,
        }
    }
}

/// Write the completion script for `shell` to stdout.
pub fn completions(shell: Shell, cli: clap::Command) -> Result<(), (Exit, String)> {
    std::io::stdout()
        .write_all(&completion_script(shell, cli))
        .map_err(|e| (Exit::Io, format!("cannot write the completions: {e}")))
}

fn completion_script(shell: Shell, cli: clap::Command) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cli = public(cli);
    clap_complete::generate(
        clap_complete::Shell::from(shell),
        &mut cli,
        "hoarder",
        &mut out,
    );
    out
}

/// `cli` without its hidden subcommands (the helpers Hoarder starts itself),
/// which bash completion would otherwise offer.
fn public(cli: clap::Command) -> clap::Command {
    let shown: Vec<clap::Command> = cli
        .get_subcommands()
        .filter(|s| !s.is_hide_set())
        .cloned()
        .collect();
    let args: Vec<clap::Arg> = cli.get_arguments().cloned().collect();
    let public = clap::Command::new("hoarder")
        .version(env!("CARGO_PKG_VERSION"))
        .args(args)
        .subcommands(shown);
    match cli.get_about() {
        Some(about) => public.about(about.clone()),
        None => public,
    }
}

/// `hoarder dist DIR`: lay out everything a package installs besides the
/// binary, for a binary that will live in `bindir`:
///
/// ```text
/// DIR/man/man1/hoarder.1, hoarder-<subcommand>.1
/// DIR/completions/hoarder.bash, _hoarder, hoarder.fish
/// DIR/systemd/hoarder.service
/// DIR/config.toml            the default config, comments and all
/// ```
pub fn run(dir: &Path, bindir: &Path, cli: clap::Command) -> Result<(), (Exit, String)> {
    let write = |rel: &str, bytes: &[u8]| {
        let path = dir.join(rel);
        let io = |e: std::io::Error| (Exit::Io, format!("cannot write {}: {e}", path.display()));
        std::fs::create_dir_all(path.parent().expect("a file under DIR")).map_err(io)?;
        std::fs::write(&path, bytes).map_err(io)
    };

    let unbuilt = cli.clone();
    let mut cli = cli;
    cli.build();
    write("man/man1/hoarder.1", &man_page(cli.clone(), "hoarder")?)?;
    for sub in cli.get_subcommands() {
        if sub.is_hide_set() || sub.get_name() == "help" {
            continue;
        }
        let title = format!("hoarder-{}", sub.get_name());
        write(
            &format!("man/man1/{title}.1"),
            &man_page(sub.clone(), &title)?,
        )?;
    }

    for (shell, file) in [
        (Shell::Bash, "hoarder.bash"),
        (Shell::Zsh, "_hoarder"),
        (Shell::Fish, "hoarder.fish"),
    ] {
        write(
            &format!("completions/{file}"),
            &completion_script(shell, unbuilt.clone()),
        )?;
    }

    let unit = crate::daemon::service::unit_for(&bindir.join("hoarder"));
    write("systemd/hoarder.service", unit.as_bytes())?;
    write("config.toml", hoarder_config::DEFAULT_FILE.as_bytes())
}

fn man_page(cmd: clap::Command, title: &str) -> Result<Vec<u8>, (Exit, String)> {
    let mut out = Vec::new();
    clap_mangen::Man::new(cmd)
        .title(title)
        .section("1")
        .source(format!("hoarder {}", env!("CARGO_PKG_VERSION")))
        .manual("Hoarder Manual")
        .render(&mut out)
        .map_err(|e| (Exit::Io, format!("cannot render the {title} man page: {e}")))?;
    Ok(out)
}
