# Changelog

All notable changes to `hoarder-ipc`. The crate is versioned with Hoarder and fetched by git from a release tag (it is not on crates.io); the compatibility policy in its README holds: additive protocol changes keep `v: 1`, breaking ones bump the protocol `v`.

## 1.0.2

Protocol v1, wire format unchanged.

- `parse_request`: an unknown value inside a known op (`"selection":"secondary"`) is now `bad_request`, not `unknown_op`.

## 1.0.0

First release, protocol v1, from tag `v1.0.0`.

- Ops: `hello`, `status`, `reload`, `subscribe`, `list`, `search`, `get`, `thumb`, `restore`, `delete`, `pin`, `unpin`, `clear_history`, `pause`, `resume`, `clear`, `offer`, `paste`.
- Events: `selection`, `clip_added`, `clip_removed`, `state`.
- Concealed Clips: `list` with `concealed`, `ClipSummary::sensitivity` and `expires`; `restore` with `plain` for plain text only.
- `Connection` helpers for most ops, including `list_with`, `restore_as`, `delete`, `clear` and `reload`.
- `Connection`, a blocking client that passes fds (`SCM_RIGHTS`) for `get`, `thumb`, `offer` and `paste`.
