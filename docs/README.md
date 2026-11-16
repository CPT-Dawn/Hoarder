# Hoarder documentation

Guides for using Hoarder, then the design and maintainer notes. Start at the [README](../README.md) if you have not installed it yet.

## Using Hoarder

| Guide | What it covers |
| --- | --- |
| [Installation](installation.md) | AUR packages, release tarballs, building from source |
| [Keybindings](keybindings.md) | Compositor bindings, the Overlay, Refine and the Picker |
| [Command line](cli.md) | Every subcommand, with script examples |
| [Configuration](config.md) | Every key in `config.toml`, its default and the themes |
| [Privacy](privacy.md) | Secret, Concealed and Normal copies; encryption at rest |
| [Compatibility](compatibility.md) | What works on each compositor |
| [Troubleshooting](troubleshooting.md) | `hoarder doctor`, logs and common questions |
| [Set up with an AI agent](agent-setup.md) | Step-by-step instructions a coding agent can follow |

## Building on Hoarder

| Guide | What it covers |
| --- | --- |
| [Socket API](ipc.md) | The versioned JSON-lines protocol and the `hoarder-ipc` crate |

## Design and maintenance

| Guide | What it covers |
| --- | --- |
| [Design plan](PLAN.md) | The full design, performance budgets and testing strategy |
| [Decisions](adr/) | Architecture decision records |
| [Glossary](../GLOSSARY.md) | The domain vocabulary: Clip, Daemon, Overlay, Refine, … |
| [Research](research/) | The clipboard and screenshot tool landscape |
| [Testing and CI](testing.md) | The headless-Sway harness and every CI job |
| [Releasing](releasing.md) | How a tag becomes a release and AUR packages |
| [Release checklist](release-checklist.md) | The manual pass on real hardware |
| [Agent workflow](agents/issue-tracker.md) | Issue tracker, [triage labels](agents/triage-labels.md) and [domain docs](agents/domain.md) for coding agents |

[Back to the README](../README.md)
