# `ipc/` — reference-only C sources

The IPC subsystem has been ported to Rust under `rust/ipc/`. The original
C implementation files in this directory are **reference-only** and
intentionally **not** in `libkernel_a_SOURCES` (see `Makefrag.am`). They
are kept on disk for two purposes:

1. Side-by-side audit when reading or reviewing the Rust port.
2. A quick rollback escape hatch if a Rust port is found to misbehave.

## Reference-only files

```
copy_user.c       ipc_object.c    ipc_table.c
ipc_entry.c       ipc_port.c      ipc_target.c
ipc_init.c        ipc_pset.c      ipc_thread.c
ipc_kmsg.c        ipc_right.c     mach_debug.c
ipc_marequest.c   ipc_space.c     mach_msg.c
ipc_mqueue.c                      mach_port.c
ipc_notify.c
```

**Do not edit these files.** Bug fixes, new features, and any other changes
to IPC logic must go to the live Rust implementation under `rust/ipc/`.
Edits here will not affect the kernel build and will silently rot away
from the Rust source of truth.

## Files that ARE still part of the build

- `ipc_layout_asserts.c` — `_Static_assert` declarations pinning C struct
  sizes and offsets that the hand-mirrored `#[repr(C)]` types in
  `rust/ipc/mach_types.rs` rely on. **This is the contract** between the
  live C struct definitions and the Rust mirrors. Add an assertion here
  whenever you add or modify a mirror in `mach_types.rs`.

- `*.h` headers — still consumed by other kernel C code and by
  `ipc_layout_asserts.c`.

- MIG-generated `mach_port.server.c`, `mach_port.server.defs.c`,
  `notify.none.defs.c` — generated stubs; not in scope for the port.

## Rollback recipe (per file)

If a Rust port misbehaves and a fast revert is needed:

1. Add the relevant C file back to `libkernel_a_SOURCES` in `Makefrag.am`,
   e.g.

   ```
   ipc/ipc_kmsg.c \
   ```

2. Remove the matching `mod` line from `rust/ipc/lib.rs`, e.g.

   ```
   mod ipc_kmsg;
   ```

3. Remove the corresponding `use crate::ipc_kmsg::…` imports from any
   sibling Rust files that reference the module's symbols.

4. Rebuild and run the test suite. The C symbols will resolve to the C
   file's definitions; Rust callers see the same exported names.

After rollback, leave a note in the commit message explaining which Rust
port reverted and why so the next attempt can address the underlying issue.
