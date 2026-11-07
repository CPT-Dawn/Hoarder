# Changelog

All notable changes to `hoarder-ipc`. The crate follows the versioning policy in its README: additive protocol changes are minor releases, breaking ones bump the protocol `v` and the major version.

## Unreleased

First published release, protocol v1.

- Ops: `hello`, `status`, `reload`, `subscribe`, `list`, `search`, `get`, `thumb`, `restore`, `delete`, `pin`, `unpin`, `clear_history`, `pause`, `resume`, `clear`, `offer`, `paste`.
- Events: `selection`, `clip_added`, `clip_removed`, `state`.
- `Connection`, a blocking client that passes fds (`SCM_RIGHTS`) for `get`, `thumb`, `offer` and `paste`.
