# MofuOS — Agent Session Start Guide

Read this file at the start of every session working on MofuOS.

## Quick Orientation

MofuOS is a monolithic x86_64 kernel in Rust. Boots via Limine revision 5 (UEFI).
Runs in QEMU KVM. Has a custom software rendering pipeline and compositor.
Currently runs a single userspace C test program (TEST_ELF embedded in kernel).

The only run command: `make run` (from repo root, runs `make run-x86_64`).
All output to serial. No display window for logs. Logs go to `logs/<timestamp>/`.

## Agent Reference Notes

| File                              | Contents                                               |
|-----------------------------------|--------------------------------------------------------|
| `notes/agents/overview.md`        | Repo structure, build system, boot flow, key constants |
| `notes/agents/memory.md`          | Frame allocator, heap, user address spaces, layout     |
| `notes/agents/process.md`         | Process model, ELF loading, scheduler, syscalls        |
| `notes/agents/graphics.md`        | Framebuffer, windows, compositor, rendering pipeline   |
| `notes/agents/filesystem.md`      | Sirius VFS, FAT32, disk abstraction                    |
| `notes/agents/hardware.md`        | GDT/TSS, IDT, APIC, SMP, serial, MSR helpers          |
| `notes/agents/conventions.md`     | Coding rules, unsafe rules, naming, lock ordering      |
| `notes/agents/userspace/`         | Session history, current TODOs, userspace state        |

## Current State (as of 2026-07-11)

Working:
- Boot to kernel, SMP (3 cores), LAPIC timer, UEFI
- Heap (16 MB fixed-size block allocator)
- ELF loading + user address space creation
- Multiple processes on AP cores (scheduler queue, not 1:1 pinned)
- Syscalls: sys_write (fd=1/2 -> COM2), sys_exit, sys_yield (998), sys_get_pid (997)
- Window syscalls: sys_create_window (10), sys_map_window_buffer (12),
  sys_present_window (13), sys_get_window_size (14), sys_destroy_window (11),
  sys_focus_window (15)
- sys_allocate (5): user heap growth, bump allocator in rustspace uses this
- Filesystem syscalls (20-28): sys_open_file, sys_close_file, sys_read_file,
  sys_write_file, sys_stat_file, sys_list_dir, sys_create_file,
  sys_create_dir, sys_delete — all wired to Sirius VFS
- FileDescriptor table in Process: node_id, offset, flags (FD_FLAG_READ/WRITE)
- ATA PIO driver + FAT32 driver: read, write, create, delete, directories
- FAT32 mounted from ATA disk at boot via test_ata_filesystem() in main()
- Per-core GDT + TSS with real RSP0 and IST0 stacks
- Per-core syscall stacks via KERNEL_GS_BASE MSR + swapgs (BUG-01 resolved)
- sys_exit returns to scheduler loop via saved kernel RSP (BUG-02 resolved)
- sys_yield: suspends process, saves context, re-enqueues, runs next process
- Timer-based preemption: preempts Ring3 if other processes waiting;
  patches SS=0x1b in interrupt frame if no preemption needed (Intel sysretq
  does not restore SS.RPL=3 — patching required to avoid GPF on iretq)
- SFMASK masks IF on SYSCALL entry; sti at start of handle_syscall_inner
- SyscallFrame: all 15 registers including rax (syscall_num) correctly saved
- jump_to_userspace takes rflags param — correctly restores RFLAGS on resume
- Graphics: framebuffer, compositor, window double-buffering, 3D pipeline
- Software renderer: vertex/pixel shader traits, SIMD vertex types
- SSE enabled per-core
- rustspace: no_std Rust userspace with alloc (bump arena via sys_allocate)
- rustspace: DirEntryFlat, StatFlat repr(C) structs matching kernel wire format
- theophe: creates window, maps back buffer, renders text via embedded-graphics,
  presents frames — compositor displays on screen
- fs_test: userspace filesystem test suite (6 suites, embedded as FS_TEST_ELF)
- user programs rebuilt automatically before kernel via GNUmakefile dependency

Not working / TODO:
- Preemption of tight syscall loops limited [ISSUE-P9]
- No frame deallocation (memory leaks on process exit) [ISSUE-M1]
- Text rendering in kernel text.rs empty [ISSUE-G1]
- Alpha blending in compositor [ISSUE-G2]
- FAT32: 8.3 filenames only, no LFN [ISSUE-F5]
- FAT32: only FAT 1 written, mirror FAT 2 not updated [ISSUE-F6]

## Key Invariants (never break these)

1. Never hold PROCESS_MANAGER lock across jump_to_userspace (-> !).
2. Push process to PROCESS_MANAGER BEFORE enqueuing in SCHEDULER.
3. Physical → virtual: always add HHDM_OFFSET (runtime value from BootInfo).
4. All pixel buffers use XRGB8888 u32; use rgba_to_xrgb() for conversion.
5. Core 0 = BSP (kernel/graphics). Core 1+ = AP (userspace scheduler loops).
6. MMX disabled. Red zone disabled. Code model = large. No std.
