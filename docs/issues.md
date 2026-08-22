# MofuOS — Known Issues and Missing Features

These are gaps between the current implementation and the intended design.
Distinct from bugs.md (which covers concrete code defects).
Organized by subsystem.

---

## Memory

### ISSUE-M1: No physical frame deallocation [RESOLVED 2026-08-22]

`MemoryMapFrameAllocator` now has free-list reclamation (`free_list: Vec<PhysFrame>`,
`deallocate_frame`, `allocate_frame` pops free first) and `free_frame_count` /
`allocated_bump_frames` for tests. `UserMemoryManager` provides
`unmap_all_user_pages_with_owned_set` + `reclaim_empty_user_tables` + `free_pml4_frame`,
and `ProcessMemoryLayout::free_address_space` walks `mapped_regions`/`stack`/
`allocated_ranges` to collect owned `PhysAddr` and free leaves + tables on
`PROCESS_MANAGER::terminate_process`. File: `kernel/src/memory/memory.rs`,
`kernel/src/memory/usermem.rs`, `kernel/src/process/process_mem.rs`.

Impact before fix: monotonic OOM. After fix: `test_usermem_reclaim` (4×528384) asserts
`free` returns to baseline (801 vs 0) after exit; `free_list: Vec::new()` before
`init_heap` avoids heap-before-alloc fault.

### ISSUE-M2: User-space heap not implemented [RESOLVED]

`sys_allocate` (syscall 5) now uses `ProcessMemoryLayout::allocate_heap(size)` single-lock
bump (`heap_start == heap_end == 0x6000_0000` at `new()`; previously `heap_end` was
`VIRT_END`). `allocate_heap` aligns to 4 KiB, calls `grow_heap(new)` which maps
`PRESENT|WRITABLE|USER_ACCESSIBLE` and pushes `MappedMemoryRegion`. Syscall wrapper no
longer duplicates `old+size` arithmetic (fixed double-lock). File:
`kernel/src/process/syscall.rs`, `kernel/src/process/process_mem.rs`.

### ISSUE-M3: ACPI region mapping commented out

`map_acpi_regions` is commented out in `kmain`. ACPI tables are accessed via
the HHDM offset which works for most cases, but is not correct for all ACPI
memory types (NVS, reclaimable).
File: `kernel/src/boot.rs`

### ISSUE-M4: No guard pages for kernel stacks

`RSP0_STACKS` and `IST0_STACKS` in `gdt.rs` are plain `[u8; N]` arrays with no
guard pages below them. A kernel stack overflow would silently corrupt adjacent
memory before tripping any fault.

---

## Process / Scheduler

### ISSUE-P1: Scheduler timer tick does nothing [RESOLVED — see preemption]

`Scheduler::on_timer_tick` still contains no scheduling logic (the method
body is empty), but preemption is now handled directly in
`timer_interrupt_handler` in `interrupts.rs`. The method can be repurposed
for time-slice accounting when needed.

### ISSUE-P2: No preemption [RESOLVED]

Timer-based preemption is implemented in `timer_interrupt_handler`
(`kernel/src/interrupts.rs`). On each tick, if `stack_frame.code_segment.rpl()
== Ring3` (interrupted userspace), the handler:
1. Saves `instruction_pointer`, `stack_pointer`, `cpu_flags` from the
   interrupt frame into `process.execution_context`.
2. Sends LAPIC EOI (`interrupt_over()`) before returning to scheduler.
3. Calls `scheduler::return_to_scheduler()` to unwind to the scheduler loop.

If the interrupt fires in Ring0 (kernel code), it only re-arms the timer and
sends EOI — no preemption.

`jump_to_userspace` now takes a third `rflags: u64` argument so that both
first-entry (uses `RFLAGS_DEFAULT = 0x202`) and resumed-entry (uses saved
RFLAGS) work correctly.

### ISSUE-P9: Preemption ineffective for tight syscall loops

A process that calls `sys_write` in a tight loop spends nearly all its time
inside the kernel syscall handler. Even though interrupts are technically
enabled during the handler (SFMASK = 0, so IF is not cleared on SYSCALL
entry on AMD), the timer rarely fires within the brief userspace window
between `sysretq` and the next `syscall`. In practice such a process is
never preempted and runs to completion uninterrupted.

Options to fix:
- Re-enable interrupts explicitly at the start of `handle_syscall_inner`
  (safe to do since we are on the kernel syscall stack). This allows the
  timer to fire mid-syscall and preempt there.
- Add a voluntary preemption point (`sys_yield`) between iterations in the
  userspace program, which is already implemented as syscall 998.
- Lower the timer interval from 10ms to something shorter.

### ISSUE-P3: No wait() / process synchronization

`sys_exit` terminates the calling process but does not notify the parent.
There is no `wait()` syscall, no exit status propagation, and no way for a
parent to block until a child finishes.
File: `kernel/src/process/syscall.rs`

### ISSUE-P4: Most syscalls unimplemented [PARTIALLY RESOLVED]

`handle_syscall_inner` handles the following. Everything else returns `u64::MAX`.

Implemented:

| Number | Name                  | Notes                                              |
|--------|-----------------------|----------------------------------------------------|
| 2      | sys_write             | fd=1/2 -> COM2 (now Option-safe; null backend in test) |
| 5      | sys_allocate          | bump heap growth via `allocate_heap`, returns old heap_end |
| 10     | sys_create_window     | creates compositor window, returns id              |
| 11     | sys_destroy_window    | marks window invisible, recycles id                |
| 12     | sys_map_window_buffer | maps back buffer pages into user space             |
| 13     | sys_present_window    | triggers back->front swap                          |
| 14     | sys_get_window_size   | returns (width<<32)|height                         |
| 15     | sys_focus_window      | brings window to front                             |
| 20     | sys_open_file         | path -> fd index; flags: FD_FLAG_READ/WRITE        |
| 21     | sys_close_file        | swap-removes fd from process table                 |
| 22     | sys_read_file         | reads from fd at current offset, advances it       |
| 23     | sys_write_file        | writes to fd at current offset, advances it        |
| 24     | sys_stat_file         | fills StatFlat at user pointer                     |
| 25     | sys_list_dir          | fills DirEntryFlat array at user pointer           |
| 26     | sys_create_file       | creates file via Sirius                            |
| 27     | sys_create_dir        | creates directory via Sirius                       |
| 28     | sys_delete            | deletes file or empty directory via Sirius         |
| 997    | sys_get_pid           | returns calling process PID                        |
| 998    | sys_yield             | suspend + save context + re-enqueue                |
| 999    | sys_exit              | terminate + free_address_space + return to scheduler |

Notes: `sys_write` COM2 now `Option<Uart>` to avoid `DeviceNotPresent` panic in
QEMU test mode (`-serial null`). `sys_exit` now reclaims user frames/tables and
releases core; `sys_allocate` no longer double-locks.

Unimplemented: 0 (create_process), 1 (terminate_process), 3 (read), 4 (get_line),
8 (load_file), 9 (unload_file), 996 (get_process_info).

File: `kernel/src/process/syscall.rs`

### ISSUE-P5: Core 0 is never used for userspace

Core 0 is excluded from the core pool (`available_cores &= !1`). It runs the
kernel main loop (graphics, test setup). All userspace processes must go on
core 1 or higher. With only 2 cores configured in QEMU, only one process can
run at a time.

### ISSUE-P6: No IPC implemented

Architecture notes describe shared memory and port-based IPC. Nothing is
implemented. Processes have no way to communicate.

### ISSUE-P7: process_manager.rs has dead `create_process` path

`ProcessManager::create_process` exists (for non-ELF processes via raw entry
point + stack) but passes placeholder zeros for entry_point/stack_top/page_table
in the syscall handler. It is not usable as-is.
File: `kernel/src/process/syscall.rs` (CreateProcess syscall branch)

### ISSUE-P8: No multi-process support from ELF on disk

`TEST_ELF` is `include_bytes!` — a single binary baked into the kernel image.
There is no way to load programs from the filesystem into new processes.
Loading ELF from Sirius VFS is not connected to the process creation path.

---

## Graphics

### ISSUE-G1: text.rs is empty

`kernel/src/graphics/text.rs` is a 0-byte file. No text rendering primitive
exists. `Theophe` has its own character rendering but it is not reusable.

### ISSUE-G2: No alpha blending in compositor

`compositor.compose()` does a raw pixel copy. Transparency (A channel in
`Rgba8888UNORM`) is ignored entirely.
File: `kernel/src/graphics/compositor.rs`

### ISSUE-G3: No dirty region tracking

Every `compose()` call blits all visible windows in full, even if nothing
changed. No damage/dirty rect tracking.

### ISSUE-G4: `RENDER_SHADERS = false` disables the 3D render loop

The main loop does nothing when `RENDER_SHADERS` is false (the default).
The 3D cube / pipeline is set up but never animated.
File: `kernel/src/main.rs`

### ISSUE-G5: Double compositor creation in main.rs

`main()` creates a `Compositor` twice. The second creation shadows the first.
The first window and window3 created with the first compositor are orphaned.
Only the second compositor's windows are composed to the framebuffer.
File: `kernel/src/main.rs`

### ISSUE-G6: Compositor does not own the framebuffer

The design intent (noted in TODO comments in compositor.rs) is for the
compositor to own the `FrameBufferTarget`. Currently `compose()` takes a
mutable reference on every call. This prevents the compositor from doing
autonomous background rendering.

### ISSUE-G7: No window input routing

There is no keyboard or mouse input system. Keystrokes are handled by the IDT
keyboard handler but are not routed to any process or window.

---

## Filesystem

### ISSUE-F1: Filesystem not mounted in default boot [RESOLVED]

`init_filesystem_ata()` is now called from `main()` via `test_ata_filesystem()`
at startup. The FAT32 filesystem on `ata_disk.img` is mounted and available for
the duration of the boot. Sirius VFS is accessible via `get_sirius()` after that
call returns.

### ISSUE-F2: No real disk driver [RESOLVED]

`AtaPioDriver` implements `DiskDevice` for the primary ATA bus (ports 0x1F0-0x1F7),
polling mode, LBA28. Persistent writes survive QEMU restarts via `ata_disk.img`.
File: `kernel/src/io/ata.rs`

### ISSUE-F3: File descriptor table unused [RESOLVED]

`Process::file_descriptors: Vec<FileDescriptor>` is now fully used.
`sys_open_file` allocates entries; `sys_close_file` swap-removes them.
Each `FileDescriptor` holds `node_id: usize`, `offset: usize`, `flags: u8`.

### ISSUE-F4: No file buffer cache

All reads/writes go directly to the ATA driver. No in-memory caching,
no write-back buffering, no copy-on-write.

### ISSUE-F5: FAT32: 8.3 filenames only, no LFN

The driver uses short 8.3 names (stem <= 8 chars, extension <= 3 chars).
`set_filename` returns `Err(InvalidFilename)` if the name exceeds these limits.
Long File Name (LFN) support is not implemented.

### ISSUE-F6: FAT32: mirror FAT not updated

`write_fat_entry` only updates FAT 1. FAT 2 (the mirror) is never written.
`num_fats` from the BPB is 2, but only FAT 1 is used. This is fine for
QEMU/development but violates the FAT32 spec for robust operation.

---

## Hardware / Boot

### ISSUE-H1: X2APIC explicitly disabled

`MP_FLAG_NO_X2APIC` is forced in the Limine MP request. X2APIC is faster
(no MMIO, uses MSRs) and scales better with many cores. Currently not used.
File: `kernel/src/boot.rs`

### ISSUE-H2: AP trampoline identity mapping may not be used

`setup_ap_trampoline_mapping` in `memory.rs` sets up identity maps for 0x8000
and 0x9000, but the current boot path uses Limine's native MP bootstrap
(`cpus[N].bootstrap(...)`) which does not need a custom trampoline.
The trampoline in `asm/ap_trampoline.S` may be unused dead code.
File: `kernel/src/memory/memory.rs`, `kernel/src/boot.rs`

### ISSUE-H3: No SMP scaling beyond 2 cores

QEMU is launched with `-smp cores=2`. The code supports MAX_CORES=16 but
this has never been tested with more than 2. Scheduler and core pool
initialization may have untested edge cases at higher core counts.

---

## Code Quality

### ISSUE-C1: `#![allow(warnings, unused)]` suppresses all compiler feedback

The blanket allow in `lib.rs` hides dead code, unused variables, unused imports,
type errors that degrade to warnings, and the duplicate `on_timer_tick` definition.
Removing it would reveal the real state of the codebase.

### ISSUE-C2: `ACPI` Handler IO stubs will panic if called

See BUG-07. Noted here as an architectural issue: the ACPI handler is
incomplete by design, which limits future ACPI capability.

### ISSUE-C3: No no-std test harness

`tests_exp/` contains in-kernel tests but they are run by calling them
manually from `main.rs` (commented out). There is no automated test
runner, no pass/fail reporting at the kernel level, and no CI.
