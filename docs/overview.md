# MofuOS — Project Overview

## What It Is

MofuOS is a monolithic x86_64 kernel written in Rust. No message-passing for kernel
functions — direct calls for performance. Boots via Limine revision 5 (protocol v5 /
BaseRevision 5). Runs in QEMU with KVM acceleration.

Target: educational/thesis OS with a custom software rendering pipeline, compositor,
and userspace.

## Repository Layout

```
MofuOS/
  GNUmakefile          — root build orchestrator (make run targets here)
  Cargo.toml           — workspace root; kernel is the only member
  .cargo/config.toml   — target = x86_64-unknown-none, link-arg linker script
  rust-toolchain.toml  — nightly channel, rust-src + llvm-tools-preview
  x86_64-kernel.json   — custom target spec (code-model=large, no-redzone, no-mmx, static reloc)
  linker-x86_64.ld     — kernel linker script (base 0xffffffff80000000)
  limine.conf          — bootloader config (timeout 0, protocol limine)
  kernel/              — kernel crate (the only binary)
    Cargo.toml         — kernel dependencies
    build.rs           — assembles + links AP trampoline binary blob
    GNUmakefile        — thin wrapper around cargo build
    src/
      main.rs          — binary entry: panic handler, QemuExitCode, main()
      boot.rs          — Limine requests, kmain() entry point
      lib.rs           — crate root: constants, module declarations
      boot_info.rs     — BootInfo struct + BOOT_INFO static (Once)
      gdt.rs           — per-core GDT/TSS
      interrupts.rs    — IDT, APIC, TSC calibration, timer
      process_start.rs — helpers: create_init_process, create_userspace_processes
      process/         — scheduler, process, elf_loader, syscall, core_pool, ...
      memory/          — frame allocator, heap, user address space manager
      graphics/        — framebuffer, color, window, compositor, pipeline, renderer, ...
      filesystem/      — Sirius VFS, FAT32 driver
      io/              — serial (uart_16550), disk abstraction
      util/            — APIC registers, MSR helpers, cpuinfo/SMP
      asm/             — AP trampoline assembly
      data_structures/ — Vec, Dequeue (no-std replacements)
      programs/        — Theophe (terminal/text renderer), arche (empty stub)
      tests_exp/       — in-kernel test runners (graphics, process, filesystem)
  user/
    Makefile           — builds all user programs with clang
    linker.ld          — user ELF linker script
    crt0.c/crt0.o      — minimal C runtime
    libc/              — syscall.h, syscall.o, types.h (bare-metal libc)
    programs/test/     — main test.c userspace test suite (linked in as TEST_ELF)
    programs/first/    — first.c minimal hello-world process
  os_disk_fat32/       — content for the FAT32 disk image (test.txt)
  limine/              — Limine bootloader binaries (v10.x-binary)
  ovmf/                — OVMF firmware (code + vars FDs for UEFI boot)
  esp/                 — EFI System Partition artifacts
  scripts/
    log_splitter.py    — reads two serial UNIX sockets, splits into per-core log files
  logs/                — timestamped log directories (all.txt, core_N.txt, userspace_pid_N.txt)
  notes/               — design notes, thesis drafts, agent notes
```

## How to Build and Run

The ONLY command used in practice:

```
make run
```

This invokes `make run-x86_64` which:
1. Builds the kernel: `cargo build --target x86_64-unknown-none` inside `kernel/`
2. Assembles the AP trampoline via `build.rs` (uses `cc` + `ld` + `objcopy`)
3. Creates `template-x86_64.iso` with xorriso (Limine + kernel binary)
4. Runs `log_splitter.py` in background (listens on two UNIX sockets for COM1/COM2)
5. Launches QEMU:
   - Machine: q35, KVM, 2 cores (cores=2 threads=1), cpu qemu64 (+tsc-deadline +apic)
   - UEFI: OVMF pflash drives
   - Boot: -cdrom template-x86_64.iso
   - Serial: COM1 → /tmp/mofuos_com1.sock (kernel logs), COM2 → /tmp/mofuos_com2.sock (userspace)
   - Exit device: isa-debug-exit at 0xf4 (write 0x10 = success, 0x11 = failure)
   - Monitor: telnet 127.0.0.1:1234
   - RAM: 2GB

There is NO serial stdio. All output is captured to log files under `logs/`.

## Cargo and Build Config

Workspace `Cargo.toml`:
- members = ["kernel"]
- kernel is the only binary

`.cargo/config.toml`:
```toml
[build]
target = "x86_64-unknown-none"

[target.x86_64-unknown-none]
rustflags = ["-C", "link-arg=-Tlinker-x86_64.ld", "-C", "relocation-model=static"]

[unstable]
build-std = ["core", "alloc"]
```

`rust-toolchain.toml`:
- nightly, components: rustfmt, clippy, rust-src, llvm-tools-preview

Custom target `x86_64-kernel.json`:
- code-model = large (kernel lives at top 2GiB)
- panic-strategy = abort
- disable-redzone = true
- features = "-mmx" (MMX disabled; SSE/AVX still available)
- linker = rust-lld, linker-flavor = lld-elf

## Linker Script (linker-x86_64.ld)

Kernel loads at `0xffffffff80000000` (topmost 2GiB, as Limine mandates).

Sections:
- `.text` → PT_LOAD (text)
- `.rodata` → PT_LOAD (rodata), aligned to MAXPAGESIZE
- `.data` + Limine requests + `.bss` → PT_LOAD (data), aligned to MAXPAGESIZE
- Limine request markers: `.requests_start_marker`, `.requests`, `.requests_end_marker`
- Discards: `.eh_frame*`, `.note*`

Entry symbol: `kmain`

## Boot Flow (kmain in boot.rs)

1. Assert `BASE_REVISION.is_supported()` (revision 5)
2. Get UEFI memory map, Limine memory map, paging mode, HHDM offset, RSDP, framebuffer
3. Init framebuffer (`graphics::framebuffer::init_framebuffer`)
4. Store `BootInfo { hhdm_offset }` in `BOOT_INFO` (Once)
5. BSP GDT init (`init_core_gdt(0)`) + load IDT
6. Init offset page table (`init_offset_page_table(hhdm_offset)`)
7. Init frame allocator (`MemoryMapFrameAllocator::init`)
8. Init heap (`allocator::init_heap`) at `HEAP_POINTER = 0xFFFF_8080_0000_0000`, 16 MB
9. Init ACPI (`interrupts::init_acpi`)
10. Parse MP response (multiprocessor) — enumerates CPUs
11. Init CPU info (`init_cpu_info`, `init_cpu_infos`)
12. Map LAPIC for core 0, init LAPIC timer
13. Init `CORE_POOL` and `SCHEDULER` with core count
14. Init `init_current_core()` for BSP
15. Init global memory globals (`FRAME_ALLOCATOR`, `USER_MEMORY_MANAGER`)
16. Calibrate TSC via PIT (`init_tsc_globals`)
17. Bootstrap AP core 1: `cpus[1].bootstrap(ap_core_from_limine_entry_point, 0x12345678)`
18. Enable interrupts
19. Call `main()` (in src/main.rs)

## main() in src/main.rs

1. `init_syscall_stack()` — sets up global syscall stack pointer
2. `create_userspace_processes()` — parses `TEST_ELF`, creates process via `PROCESS_MANAGER`,
   enqueues on core 1's scheduler queue
3. Init framebuffer, create compositor + windows
4. Run graphics demos (shapes, 3D cube shader test via pipeline)
5. Enters infinite render loop (RENDER_SHADERS const gate)

## AP Core Boot (ap_core_from_limine_entry_point in cpuinfo.rs)

Each AP core (currently only core 1) runs:
1. `init_core_gdt(core_id)` — own GDT+TSS
2. Map LAPIC for this core
3. `init_lapic_for_current_core(core_id)`
4. `init_syscall()` — write STAR/LSTAR/SFMASK MSRs for SYSCALL/SYSRET
5. Enable interrupts
6. `run_on_core_loop(core_id)` — scheduler loop

## Key Constants (lib.rs)

```rust
HHDM_OFFSET: u64 = 0xFFFF_8000_0000_0000   // Higher-Half Direct Map base
MAX_CORES: u8 = 16                            // hard limit; core pool uses u64 bitmap
AP_CORE_COUNT: u8 = MAX_CORES - 1            // APs = all except BSP
```

`static_assertions::const_assert!(MAX_CORES <= 64)` — enforced at compile time.

## QEMU Exit Device

Port 0xF4, size 4 bytes. Write:
- `0x10` → `QemuExitCode::Success` (QEMU exits with code 33)
- `0x11` → `QemuExitCode::Failed` (QEMU exits with code 35)

Panic handler calls `exit_qemu(QemuExitCode::Failed)`.

## Logging

Two serial ports:
- COM1 (0x3F8): kernel debug logs — `serial_print!`, `serial_println!`, `serial_println_core!`
- COM2 (0x2F8): userspace stdout/stderr — writes routed via syscall 2 (sys_write fd=1/2)

`serial_println_core!` prepends `[Core N | Xus]` using `get_current_core_id()` and
`tsc_timestamp_us()`.

`log_splitter.py` receives raw serial data on UNIX sockets, splits into files:
`logs/<timestamp>/all.txt`, `core_N.txt`, `userspace_pid_N.txt`
