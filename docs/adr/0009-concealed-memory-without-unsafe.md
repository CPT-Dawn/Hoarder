# Concealed memory without `unsafe`: `region` and `zeroize`, not `memsec` and `secrecy`

PLAN §5.1 first called for `memsec` to allocate and mlock Concealed Clips and `secrecy` to wrap them. The workspace forbids `unsafe` code (`unsafe_code = "forbid"`), and `memsec`'s allocator is an `unsafe` API: every use needs an `unsafe` block in our code. So Concealed bytes are copied into a page-aligned `Vec` of their own, locked with `region::lock` (a safe wrapper over `mlock`), and zeroed with `zeroize` when dropped, before the lock guard unlocks the pages. Buffers that grow while an Offer is read are zeroed before they are freed (`extend_scrubbed`), and every representation a Slot lets go of is scrubbed (`Slot::hold`).

`secrecy` adds little on top: its wrapper only hides `Debug` output and makes access explicit. Our `Locked` type already prints only its length, and the bytes never leave the Daemon except through a paste or `get`.

If the kernel refuses the lock (`RLIMIT_MEMLOCK`), the Clip is still held and wiped, just not pinned, and the Daemon says so once. Losing that is a weaker guarantee, not a leak to disk; full-disk encryption covers swap.
