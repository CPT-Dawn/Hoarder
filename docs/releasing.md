# Releasing

Releases are cut by pushing a tag. `.github/workflows/release.yml` runs on version tags only. Nothing else in the repository publishes anything. Hoarder ships through the AUR and the GitHub release only: no crate goes to crates.io (every crate has `publish = false`), and third parties depend on `hoarder-ipc` by git, pinned to a release tag (`tag = "v1.0.0"`).

| Tag | What happens |
|---|---|
| `v1.0.0` | Release notes (git-cliff), source and binary tarballs, AUR packages built and namcap'd, GitHub release. |
| `v1.0.0-rc.1` (also `-alpha.N`, `-beta.N`) | The same, marked as a pre-release on GitHub. |
| `v1.0.0-dryrun.1` | A dry run of `1.0.0`. Everything is built and checked. Nothing is published. The outputs stay as workflow artifacts. |

The tag's version must equal `version` in `Cargo.toml`, or the `plan` job stops the run.

## What the pipeline builds

| Job | Output |
|---|---|
| `notes` | `RELEASE_NOTES.md`, from `cliff.toml`. It covers the commits since the last release tag; dry-run tags never count as one. |
| `source` | `hoarder-VERSION.tar.gz`, a `git archive` of the tag. The `hoarder` AUR package builds from it. Its checksum is stable, unlike GitHub's on-the-fly archives. |
| `build` | `hoarder-VERSION-{x86_64,aarch64}-unknown-linux-gnu.tar.gz`, one per architecture, each built natively. They hold the stripped `dist`-profile binary, man pages, completions, the systemd user unit, the default config, LICENSE and README (`packaging/dist.sh`), plus a `.sha256` each. |
| `aur` | Runs in a fresh `archlinux` container. It renders the four AUR packages for this version (`packaging/aur/render.sh`), then builds `hoarder`, `hoarder-bin`, `hoarder-git` (from the tag) and `hoarder-wl-compat` as an unprivileged user, and runs namcap on each (`packaging/aur/check.sh`). It also installs `hoarder-bin` and the shims and runs them. Its output is `hoarder-VERSION-aur.tar.gz`, one directory per AUR repository with its PKGBUILD and `.SRCINFO`. |
| `github` | Creates the GitHub release with the notes, every tarball, the `.sha256` files and `SHA256SUMS`. |

The AUR is never pushed from CI. You push it yourself, as described below.

## Doing it by hand

```sh
cargo build --profile dist --locked -p hoarder
packaging/dist.sh target/dist/hoarder 1.0.0 x86_64-unknown-linux-gnu out/
git archive --format=tar --prefix=hoarder-1.0.0/ HEAD | gzip -9n > out/hoarder-1.0.0.tar.gz
cp out/hoarder-1.0.0-x86_64-unknown-linux-gnu.tar.gz out/hoarder-1.0.0-aarch64-unknown-linux-gnu.tar.gz  # stand-in, render only
packaging/aur/render.sh 1.0.0 out/ aur/
packaging/aur/check.sh aur/ out/ pkgs/ hoarder hoarder-bin hoarder-wl-compat   # builds, never installs
git cliff --unreleased --tag v1.0.0 --strip header                              # the notes
```

`check.sh` builds with `makepkg --cleanbuild` in a temporary directory, from the local tarballs, so nothing is downloaded and nothing is installed. Without a hoarder package installed it skips namcap's symlink rule for the shims. CI installs `hoarder-bin` first, so there the rule runs.

## One-time setup (before the first release)

1. **AUR account.** It needs an SSH key (<https://aur.archlinux.org/account>). Each of the four names is a free AUR repository until its first push creates it. As of 2026-11-08, `hoarder`, `hoarder-bin`, `hoarder-git` and `hoarder-wl-compat` were all unclaimed.

## Cutting v1.0.0

1. On `integration/v1.0`, bump the version. Change `version = "0.1.0"` to `"1.0.0"` in `[workspace.package]`. Then refresh the lockfile and write the changelog:
   ```sh
   cargo check --workspace          # updates Cargo.lock
   git cliff --tag v1.0.0 -o CHANGELOG.md
   git commit -am "chore(release): v1.0.0"
   ```
2. Merge `integration/v1.0` into `main` once CI (including `budgets`) is green.
3. Dry run from `main`:
   ```sh
   git tag -s v1.0.0-dryrun.1 -m "Hoarder 1.0.0 dry run" && git push origin v1.0.0-dryrun.1
   gh run watch   # the Release workflow
   ```
   Download the `aur` and `binary-*` artifacts and check them. Then delete the tag: `git push --delete origin v1.0.0-dryrun.1 && git tag -d v1.0.0-dryrun.1`.
4. Run the manual pass in [`release-checklist.md`](release-checklist.md) on real hardware. Every row passes or has an issue judged not to block.
5. Release:
   ```sh
   git tag -s v1.0.0 -m "Hoarder 1.0.0" && git push origin v1.0.0
   ```
6. Attach the filled-in checklist to the release: `gh release upload v1.0.0 checklist-v1.0.0.md`.
7. Push the AUR packages from the release's `hoarder-1.0.0-aur.tar.gz`:
   ```sh
   gh release download v1.0.0 --pattern 'hoarder-1.0.0-aur.tar.gz'
   mkdir aur && tar -xzf hoarder-1.0.0-aur.tar.gz -C aur
   for pkg in hoarder hoarder-bin hoarder-git hoarder-wl-compat; do
     git clone "ssh://aur@aur.archlinux.org/$pkg.git" "aur-$pkg"
     cp -r "aur/$pkg/." "aur-$pkg/"
     git -C "aur-$pkg" add -A && git -C "aur-$pkg" commit -m "Update to 1.0.0"
     git -C "aur-$pkg" push
   done
   ```

**If a run fails halfway:** nothing is published before the `github` job, so rerunning the failed jobs retries. If `github` itself failed after creating the release, delete the release (`gh release delete v1.0.0`, keeping the tag) and rerun it.
