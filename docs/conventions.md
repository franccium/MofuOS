# MofuOS — Coding Conventions and Project Rules

## General Style

From `notes/AGENTS.md`:
- No emotes / special non-ASCII characters in debug logs or comments. Plain text only.
- No unnecessary leading whitespace in log messages.
- Write simple, direct plaintext — avoid decorative formatting in code strings.

From the codebase:
- Rust nightly (1.96.0-nightly). Features used: `abi_x86_64_interrupt`, `allocator_api`,
  `portable_simd`, `naked_functions` (for syscall handler).
- `#![allow(warnings, unused)]` is in lib.rs as a blanket suppressor — this is
  acknowledged as a TODO to remove.
- All kernel code is `#![no_std]` + `extern crate alloc`.

## Unsafe Rules

This is bare-metal OS code. Unsafe is pervasive and necessary. Rules to follow:

1. Every `unsafe fn` or `unsafe {}` block must have a clear reason. If it is not
   obvious from context, add a brief comment explaining the invariant being upheld.

2. Raw pointer arithmetic must account for the HHDM offset when accessing physical
   memory from the kernel. Physical address → virtual: `phys + HHDM_OFFSET`.
   Never dereference a physical address directly.

3. Never hold a lock across `core_run_scheduling(core_id)`, `block_current_task_until_event` or anything else that can switch
   tasks. A task that sleeps with a lock held blocks every core that wants the lock.
   Pattern: extract needed data from a lock, drop the lock, then block.

4. Never hold `PROCESS_MANAGER` while blocking — the syscall handler of every
   other task acquires it, so one blocked holder stalls the whole system.

5. Statics mutated after init (`static mut`) should be indexed by core_id where
   per-core state is needed (e.g., `PER_CORE_GDT[core_id]`).

6. `UnsafeCell` is used for double-buffer pointer swapping in `WindowBuffer`.
   The `unsafe` there relies on the single-threaded compositor assumption.

## Naming Conventions

- Kernel globals: `UPPER_SNAKE_CASE` statics (e.g., `CORE_RUN_QUEUES`, `CORE_POOL`,
  `FRAME_ALLOCATOR`, `PROCESS_MANAGER`).
- Constants: `UPPER_SNAKE_CASE` (e.g., `HHDM_OFFSET`, `MAX_CORES`, `HEAP_POINTER`).
- Types: `PascalCase`. Traits: `PascalCase`.
- Functions and methods: `snake_case`.
- Per-core indexed statics: prefix with `PER_CORE_` (e.g., `PER_CORE_GDT`, `PER_CORE_TSS`).
- Global singletons: `Once<Mutex<T>>` or `lazy_static! Mutex<T>`.
- Sentinel value for "no process" / invalid: `INVALID_PID = usize::MAX`.
- Sentinel for "no window": `INVALID_WINDOW_ID = u32::MAX`.

## Logging

Use the correct macro for each context:

| Macro                  | Output    | Prefix                    | Use when                       |
|------------------------|-----------|---------------------------|--------------------------------|
| `serial_print!`        | COM1      | none                      | Simple kernel log, no newline  |
| `serial_println!`      | COM1      | none                      | Kernel log with newline        |
| `serial_println_core!` | COM1      | `[Core N \| Xus]`          | Per-core / SMP aware logging   |
| `serial2_print!`       | COM2      | none                      | Routing userspace writes only  |

Logs are captured by `log_splitter.py`. COM1 goes to `core_N.txt` and `all.txt`.
COM2 goes to `userspace_pid_N.txt`.

Do not use `println!` or `print!` — there is no std and no VGA text mode handler.

## Data Structures

Custom no-std implementations in `data_structures/`:
- `Vec<T>` — heap-backed growable array with manual capacity management
- `Dequeue<T>` — double-ended queue (the run queues no longer use it, they are intrusive lists through `TaskSlot::next`)

Prefer these over `alloc::vec::Vec` inside the kernel for consistency. In some places
`alloc::vec::Vec` is used directly (ELF loader, compositor, filesystem) — both are fine,
just be consistent within a module.

## Error Handling

No `std::error::Error`. Errors are enums with `#[derive(Debug, Clone, Copy)]` where possible.

Pattern: `Result<T, FooError>` with a module-local error enum. Examples:
- `FileSystemResult<T> = Result<T, FileSystemError>`
- `DiskOpResult<T> = Result<T, DiskOpError>`
- `ProcessError`, `ElfLoadError`, `MapToError<Size4KiB>`

No panics in hot paths. Panics in boot / initialization are acceptable and will
trigger the panic handler which calls `exit_qemu(QemuExitCode::Failed)`.

`assert!` and `debug_assert!` are used to enforce invariants in scheduler and
process management (e.g., `assert!(pid != INVALID_PID)`).

## Concurrency / Lock Ordering

Lock hierarchy (acquire in this order to avoid deadlock):

1. Compositor (when needed)
2. `CORE_POOL`
3. `PROCESS_MANAGER`
4. Run queue of one core (`IrqMutex`, never two at once) and wait queues (`IrqMutex`)
5. `FRAME_ALLOCATOR` (`USER_MEMORY_MANAGER` has no lock, it is a `&'static` from a `Once`)
6. `SERIAL1` / `SERIAL2` (grabbed briefly, interrupts disabled)

Window cleanup on process exit (`destroy_windows_of_process`) takes the compositor and then
`PROCESS_MANAGER`, so it runs before `PROCESS_MANAGER` is locked by the exit and kill paths.

Never hold a run queue lock while acquiring PROCESS_MANAGER. Data that an interrupt handler
touches (run queues, wait queues) is behind `IrqMutex`, which keeps interrupts off while it is held.
A plain `spin::Mutex` that an interrupt handler also takes deadlocks the core (the LAPIC address
lock did).

Compositor uses `RwLock<Vec<Window>>` — reads are concurrent, writes (create/destroy)
are exclusive.

## Build Constraints

- All builds via `cargo xtask` (`cargo x`): `cargo xtask build` builds kernel + user, `cargo xtask iso` builds ISO. Legacy `make` and `kernel/GNUmakefile` are deprecated shims.
- Direct kernel build without xtask: `cargo build -p kernel --target x86_64-unknown-none -Z build-std=core,alloc` (from workspace root).
- The `kernel/build.rs` uses `cc`, `ld`, `objcopy` — these must be available in PATH.
  On Debian: `sudo apt install -y build-essential binutils`.
- `llvm-tools-preview` component is required for `llvm-objcopy` (toolchain installs it).
- No `std` in kernel; `build-std` is passed explicitly by xtask (`-Z build-std=core,alloc`), not via global `.cargo/config.toml`.
- Target: `x86_64-unknown-none` (or `x86_64-kernel.json` for the custom target spec). `xtask` builds user programs via `clang`/`ld.lld` + `user/rustspace` cargo.

## File Layout Rules

- New kernel subsystems go in `kernel/src/<name>/` with a `mod.rs`.
- Public re-exports at module level (`pub use ...`) in `mod.rs` for key types.
- Tests go in `kernel/src/tests_exp/test_<name>.rs` (experimental test harness).
- Userspace programs go in `user/programs/<name>/` with a C source file.

## Architecture Constraints (non-negotiable)

1. Monolithic kernel — no user-kernel IPC for kernel services.
2. `no_std` — absolutely no std library anywhere in kernel code.
3. Kernel virtual base: `0xffffffff80000000` — hardcoded in linker script and Limine.
4. HHDM offset: `0xFFFF_8000_0000_0000` — matches what Limine provides in practice.
   The actual value is read from Limine at boot and stored in `BootInfo::hhdm_offset`.
   `HHDM_OFFSET` constant is the expected value; always use the runtime value from
   `BootInfo` for any new mapping code.
5. MMX is disabled (`-mmx` in target features). Do not use MMX intrinsics.
6. Red zone is disabled (`disable-redzone = true`). Inline asm that touches RSP is safe.
7. Code model is `large` — can use absolute addresses anywhere in the binary.
8. Paging: 4-level (PML4), 4KiB pages. Huge pages are handled in
   `translate_user_virt_to_phys` (2MiB check) but not allocated.
9. Interrupt model: XAPIC only (X2APIC disabled). LAPIC MMIO at `LAPIC_VIRT_BASE`.
10. SMP: core 0 = BSP (kernel, graphics, process creation), core 1+ = AP (scheduler
    loops, userspace execution). Currently only 2 cores in QEMU.

## Known Suppressed Warnings

`#![allow(warnings, unused)]` in `lib.rs` suppresses all warnings kernel-wide.
This is a known TODO. When working on a module, it is fine to have dead code;
the blanket suppressor will not report it.

## Process / Thread Creation Invariant

When creating a new process:
1. Allocate the task slot and build its first-entry stack (`task::init_task_stack`).
2. Push `Process` to `PROCESS_MANAGER.processes` Vec.
3. **Only then** make the slot `Ready` and queue it (`scheduler::core_enqueue_ready_task`).

This ordering is required. An AP core can dequeue the task the instant it is queued and its
first syscall looks the process up with `get_process(pid)`.

## Pixel Format

All window and framebuffer pixel buffers use XRGB8888 (`u32`):
- Bits 31-24: ignored
- Bits 23-16: R
- Bits 15-8:  G
- Bits 7-0:   B

Use `rgba_to_xrgb(Rgba8888UNORM) -> u32` for all conversions into buffer slots.
`Rgba8888UNORM.to_u32_rgba()` produces RGBA (different layout) — do not use this
for writing to pixel buffers.

## Session Notes Location

Agent session notes: `notes/agents/userspace/`
- `userspace_master.md` — master index of sessions and quick reference
- `userspace_todo.md` — current TODO list
- `session_YYYY_MM_DD.md` — per-session detailed notes

At the start of any session working on this codebase, read:
1. `notes/agents/start_session.md` — orientation guide
2. `notes/agents/overview.md` — project structure
3. The relevant subsystem note (`memory.md`, `process.md`, etc.)
4. `notes/agents/userspace/userspace_master.md` for current state/TODOs
