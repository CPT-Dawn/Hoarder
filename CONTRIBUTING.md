# Contributing to Hoarder

Thanks for helping. Bug reports, app-compatibility reports and PRs are all welcome.

## Reporting a bug

Use the [bug report form](https://github.com/CPT-Dawn/Hoarder/issues/new?template=bug.yml) and paste the output of:

```sh
hoarder doctor
hoarder --version
```

`doctor` names the compositor, the protocols Hoarder found, the paths it uses, the service state and any conflicting tools; most problems are diagnosed from it alone. For clipboard bugs, name the app you copied from and the app you pasted into. Daemon logs are in `journalctl --user -u hoarder`.

Never paste clipboard contents or History that might hold secrets.

## Before you write code

- **Vocabulary.** Hoarder has a precise domain language (Selection, Offer, Clip, Takeover, Restore, Concealed, Frozen Frame, Region, …) defined in [GLOSSARY.md](GLOSSARY.md). Use those words in code, comments, commits and docs, and avoid the synonyms the glossary lists.
- **Decisions.** Read the [ADRs](docs/adr/) touching the area you are changing. Some things are settled on purpose: no portal backend, no resampling, Takeover only when the Selection vanishes, no `unsafe` for Concealed memory, and the [non-goals](docs/adr/0008-non-goals.md) (screen recording, scrolling capture, sync, …). To change one, open an issue first; a new decision gets a new ADR.
- **Design.** [docs/PLAN.md](docs/PLAN.md) is the full design. Larger features should start as an issue so the approach can be agreed before the code.

## Building and testing

```sh
cargo build
cargo test --workspace      # needs sway, grim and wl-clipboard on PATH
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

The end-to-end tests drive the real `hoarder` binary inside a headless Sway session, with a helper client that paints known patterns and serves Selections. Prefer a test at that seam over a unit test of internals. See [docs/testing.md](docs/testing.md).

CI also runs `cargo deny`, the performance budgets (a regression of more than 20% fails), and checks that every config key is documented in [docs/config.md](docs/config.md): a new key needs a row there and a line in `crates/hoarder-config/src/default.toml`.

## Commits and pull requests

- Branch from `main`; one logical change per PR.
- [Conventional Commits](https://www.conventionalcommits.org/): `feat(picker): …`, `fix(daemon): …`, `docs: …`, `test(e2e): …`. The subject is at most 72 characters. The changelog and release notes are generated from these with git-cliff, so write subjects for users.
- A PR that changes behaviour updates the docs and adds or updates a test.
- The IPC API ([docs/ipc.md](docs/ipc.md)) is public and versioned: additive changes only within a version.

## License

By contributing you agree that your contributions are licensed under the [MIT License](LICENSE).
