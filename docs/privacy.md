# Privacy

How Hoarder decides what to keep. Each copy is classified before anything is stored ([ADR-0006](adr/0006-three-sensitivity-tiers.md)); the rules live under `[privacy]` in [Configuration](config.md).

| Class | What it is | What Hoarder does |
| --- | --- | --- |
| **Secret** | Marked by a password manager (`x-kde-passwordManagerHint`), or copied with `hoarder copy --sensitive` | Never read, stored or kept alive |
| **Concealed** | Copied in an app on `privacy.exclude_apps` (KeePassXC, Bitwarden, 1Password, Proton Pass and Secrets by default), or matching a secret pattern: private keys, cloud and API tokens, JWTs, card numbers | Locked memory only, masked in the Picker, gone after two minutes, never written to disk |
| **Normal** | Everything else | Stored in `$XDG_DATA_HOME/hoarder/`, readable only by you, optionally encrypted |

## Encryption at rest

```sh
hoarder config set history.encrypt true
```

History is then encrypted with [age](https://age-encryption.org), with the key in your Secret Service keyring. Turning it on or off migrates History both ways and scrubs the old form.

## Pausing

`hoarder pause 5m` stops recording into History for five minutes; `hoarder resume` ends it early. `hoarder clear` empties the clipboard and keeps the Daemon from restoring it.

[Back to the README](../README.md)
