
## Current State of the System (end of session)

- Process creation from ELF works for both single-segment (`first`) and
  multi-segment (`test`) programs.
- ELF flags (R/W/X) are correctly mapped to page table flags including NX.
- Shared boundary pages between ELF segments are handled with correct
  permission merging.
- SMP: core 1 correctly runs userspace processes without racing with core 0's
  process creation.
- Syscall entry/exit works: `sys_write` (fd 1 → serial), `sys_exit` tested.
- The `test` program was running far enough to print suite headers before this
  session ended. The remaining failing suites (if any) should be syscall
  return-value or runtime issues, not memory mapping issues.

## Known Remaining Issues / TODOs

- `SYSCALL_STACK` is a single global static, not per-core. If multiple cores
  run processes simultaneously and both make syscalls, they share one kernel
  stack -> corruption. Needs a `[SyscallStack; MAX_CORES]` array indexed by
  core ID, set in `STACK_TOP` via per-core MSR write.
- Unimplemented syscalls in `handle_syscall_inner` (read, allocate, etc.) all
  return `u64::MAX`. They need real implementations.
- Wasted physical frame on `PageAlreadyMapped`: the frame allocated before
  `map_to` is called but not used. Needs a way to return it to the allocator
  (currently there is no `deallocate_frame` on `MemoryMapFrameAllocator`).
- `stack_segment` in the page fault interrupt frame shows `Ring0` instead of
  `Ring3` in some crash dumps — investigate whether the GDT SS selector for
  userspace is set up correctly in all paths.