# Hoarder

[![CI](https://github.com/CPT-Dawn/Hoarder/actions/workflows/ci.yml/badge.svg)](https://github.com/CPT-Dawn/Hoarder/actions/workflows/ci.yml)

Pixel-exact screenshots and a clipboard that outlives the app that set it, for Wayland. Work in progress: see [`docs/PLAN.md`](docs/PLAN.md).

## Development

```sh
cargo test --workspace   # needs sway, grim and wl-clipboard on PATH
```

The workspace layout, the headless-Sway test harness and CI are described in [`docs/testing.md`](docs/testing.md).
