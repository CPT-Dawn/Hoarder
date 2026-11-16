# Installation

Every way to get Hoarder, what each package ships, and how to build it yourself. The packages are built from `packaging/aur/` and the release workflow, `.github/workflows/release.yml`.

## Arch Linux (AUR)

```sh
paru -S hoarder-bin          # or yay, or makepkg
```

| Package | What you get |
| --- | --- |
| `hoarder-bin` | The prebuilt release binary (recommended) |
| `hoarder-git` | Built from the latest commit on `main` |
| `hoarder-wl-compat` | `wl-copy` / `wl-paste` provided by Hoarder, in place of `wl-clipboard` (conflicts with it) |

Every package ships the systemd user unit, man pages, bash, zsh and fish completions, and the default config at `/usr/share/doc/hoarder/config.toml`.

## Release tarballs

Each [GitHub release](https://github.com/CPT-Dawn/Hoarder/releases/latest) has `x86_64` and `aarch64` Linux tarballs with a `.sha256` file each, plus `SHA256SUMS`. Put the `hoarder` binary on your `PATH`, then start the Daemon with `hoarder service enable`, which installs the user unit for you.

## From source

Needs Rust 1.97 or newer.

```sh
cargo install --locked --git https://github.com/CPT-Dawn/Hoarder --tag v1.0.4 hoarder
```

Hoarder is not on crates.io. To build a checkout the way packages do:

```sh
cargo build --profile dist --locked -p hoarder
```

## After installing

Continue with the [Quick start](../README.md#quick-start): start the Daemon, write the keybindings and run `hoarder doctor`.

[Back to the README](../README.md)
