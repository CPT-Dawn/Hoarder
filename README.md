# Hoarder

[![CI](https://github.com/CPT-Dawn/Hoarder/actions/workflows/ci.yml/badge.svg)](https://github.com/CPT-Dawn/Hoarder/actions/workflows/ci.yml)

Pixel-exact screenshots and a clipboard that outlives the app that set it, for Wayland. Work in progress: see [`docs/PLAN.md`](docs/PLAN.md).

## Install

Arch Linux (AUR): `hoarder` (from source), `hoarder-bin` (prebuilt), `hoarder-git` (latest commit), and `hoarder-wl-compat`, which installs `wl-copy` and `wl-paste` as Hoarder's shims in place of wl-clipboard. Every package ships the systemd user unit, man pages, bash/zsh/fish completions and the default config (`/usr/share/doc/hoarder/config.toml`).

```sh
systemctl --user enable --now hoarder   # or: hoarder service enable
```

Elsewhere: a tarball from [Releases](https://github.com/CPT-Dawn/Hoarder/releases), or build a tag with `cargo install --locked --git https://github.com/CPT-Dawn/Hoarder --tag v1.0.0 hoarder`. Nothing is published to crates.io. `hoarder completions bash|zsh|fish` prints a completion script.

## Development

```sh
cargo test --workspace   # needs sway, grim and wl-clipboard on PATH
```

The workspace layout, the headless-Sway test harness and CI are described in [`docs/testing.md`](docs/testing.md).
