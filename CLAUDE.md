# Hoarder

## Agent skills

### Issue tracker

Issues live in GitHub Issues on `CPT-Dawn/Hoarder`, managed via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Commands

- Build: `cargo build --release` (packages use `cargo build --profile dist --locked -p hoarder`).
- Test: `PATH=$HOME/.cache/wl-clipboard/usr/bin:$PATH cargo test --all-targets --no-fail-fast`. The e2e suite needs real wl-clipboard as its oracle; this machine has `hoarder-wl-compat` instead, so extract wl-clipboard from the pacman cache once: `mkdir -p ~/.cache/wl-clipboard && bsdtar -xf /var/cache/pacman/pkg/wl-clipboard-*.pkg.tar.zst -C ~/.cache/wl-clipboard usr/bin` (see docs/testing.md).
- One e2e file: `cargo test -p hoarder --test e2e_daemon` (same PATH prefix).
- Lint/format: `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- Run: `cargo run --release -- shot` (or `daemon`, `picker`, …). Don't stop the user's own `hoarder daemon`; e2e tests run in private headless Sway sessions.
- Release: bump `[workspace.package] version` in Cargo.toml, `cargo check --workspace`, `git cliff --tag vX.Y.Z -o CHANGELOG.md`, commit `chore(release): vX.Y.Z`, `git tag -s vX.Y.Z -m "Hoarder X.Y.Z"`, push main + tag, watch `gh run watch <id> --exit-status`, then push the AUR repos from the release's `hoarder-X.Y.Z-aur.tar.gz` (docs/releasing.md). AUR `hoarder` only once `ssh aur@aur.archlinux.org list-repos` lists it.
