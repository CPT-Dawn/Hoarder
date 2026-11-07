# Three Sensitivity tiers: Secret, Concealed, Normal

The data-control protocol does not reveal which app set a Selection, and browser password extensions cannot set `x-kde-passwordManagerHint`, so the hint alone is not enough. Every Selection change is therefore classified:

- **Secret**: carries `x-kde-passwordManagerHint: secret`. Never read, stored, restored or displayed.
- **Concealed**: copied while an excluded app had focus (KeePassXC, Bitwarden, 1Password, Proton Pass, Secrets by default; focus read via compositor IPC or foreign-toplevel) or matching a built-in or user secret pattern. Held in memory only, shown masked, zeroized and removed after 2 minutes, and the live Selection is cleared if it still holds it.
- **Normal**: stored in History.

The middle tier exists because the cheap alternatives both fail: dropping pattern matches silently loses legitimate copies, and storing them puts tokens on disk.
