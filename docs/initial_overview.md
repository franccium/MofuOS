# MofuOS — Initial Overview (Caveman Compressed)

> Self-contained intro for new agents. Covers build, arch, boot, memory, process, graphics, filesystem, hardware, conventions, syscalls, known gaps. Use at session start. Canonical docs: `docs/` full detail, `docs/rust_coding_guidelines.md` for code style.

---

## What It Is

MofuOS monolithic `x86_64` kernel Rust, `#![no_std]` `#![no_main]`. Direct calls for kernel functions, no IPC. Boots via Limine revision 5 (`BaseRevision 5`). Runs QEMU `q35` + `KVM`. Target: thesis OS, custom software rendering pipeline, compositor, userspace.

Toolchain: `nightly` (`rust-toolchain.toml`), `rust-src`, `llvm-tools-preview`, `x86_64-unknown-uefi`. Features: `abi_x86_interrupt`, `allocator_api`, `portable_simd`, `naked_functions`. Host needs `QEMU`, `llvm-tools` (`llvm-ar`, `ld.lld`, `objcopy`), `xorriso`, `mtools`, `parted`, `mkfs.fat`, `cc`/`ld`.

---

## Build — Never `cargo` at Root

Root `Cargo.toml` workspace (`members = ["kernel"]`) and `src/main.rs` (`ovmf_prebuilt` harness commented out) not build entry. All builds via `GNUmakefile`.

```bash
make all              # clone/build limine (v10.x-binary), build kernel + userspace, create template-x86_64.iso
make all-hdd          # same but HDD image (template-x86_64.hdd) + test_disk_image.fat32.img
make -C kernel        # kernel only (nightly cargo + AP trampoline via cc/ld/objcopy)
make -C user          # C userspace only (clang + ld.lld); rust userspace via cargo in user/rustspace
make clean            # kernel cargo clean + rm iso/hdd/fat32/ata_disk.img
```

`kernel/GNUmakefile`:

```bash
RUSTFLAGS="-C link-arg=-Tlinker-x86_64.ld -C relocation-model=static" cargo build --target x86_64-unknown-none --profile dev|release
```

`user/Makefile`: `clang --target=x86_64-unknown-elf -ffreestanding -mno-red-zone` + `ld.lld -T linker.ld`; Rust userspace `user/rustspace` `cargo fmt && cargo +nightly build -Z build-std=core,alloc -Z json-target-spec --target x86_64-user.json`.

Custom target `x86_64-kernel.json`: `code-model=large`, `disable-redzone=true`, `features="-mmx"`, `linker=rust-lld`, `linker-flavor=lld-elf`. Linker script `linker-x86_64.ld`: base `0xffffffff80000000`, `ENTRY(kmain)`, sections `.text` / `.rodata` / `.data` + Limine requests + `.bss`, discard `.eh_frame*`, `.note*`.

`.cargo/config.toml`:

```toml
[build]
target = "x86_64-unknown-none"

[target.x86_64-unknown-none]
rustflags = ["-C", "link-arg=-Tlinker-x86_64.ld", "-C", "relocation-model=static"]

[unstable]
build-std = ["core", "alloc"]
```

---

## Run — OVMF Required

```bash
# First time (OVMF not tracked in git):
mkdir -p ovmf && cp /usr/share/OVMF/OVMF_VARS_4M.fd ovmf/ovmf-vars-x86_64.fd \
               && cp /usr/share/OVMF/OVMF_CODE_4M.fd ovmf/ovmf-code-x86_64.fd
# or: make edk2-ovmf  (curls edk2-ovmf-nightly tarball)

make run              # QEMU q35 + kvm + 3 cores (SMP) + ATA disk + log_splitter.py; preferred dev path
make run-nologs       # same but -serial stdio, no socket/log splitting
make run-fast-x86_64  # virtio-vga-gl + gtk + gl, minimal serial
make run-fs           # adds test_disk_image.fat32.img as second drive
QEMUFLAGS="-m 4G" make run   # override -m 2G default (KARCH=x86_64, QEMUFLAGS appended)
```

QEMU details: `-M q35 -accel kvm -cpu host,+tsc-deadline,+apic -device isa-debug-exit,iobase=0xf4,iosize=0x04 -monitor telnet:127.0.0.1:1234,server,nowait`. Exit codes `kernel/src/main.rs:47-51`: `0x10` success, `0x11` failed. Remove `-no-reboot` if stuck black screen.

Generated/ignored: `limine/`, `ovmf/`, `target/`, `iso_root/`, `*.iso`/`*.hdd`, `storage/ata_disk.img`, `logs/`, `user/crt0.o`/`libc.a` — never edit/commit.

---

## Logging — Unix Sockets, Not Stdio

`make run` (`run-x86_64`) spawns `scripts/log_splitter.py` before QEMU:

- `COM1` (`/tmp/mofuos_com1.sock`) = kernel `serial_println_core!` → `logs/<YYYY-MM-DD_HH-MM-SS>/all.txt` + `core_N.txt` (routed by `^\[Core\s+(\d+)`, `MAX_CORES=4` — keep `kernel/src/lib.rs:13` and `scripts/log_splitter.py:22` in sync)
- `COM2` (`/tmp/mofuos_com2.sock`) = userspace → `all.txt` + `userspace_pid_<pid>.txt` (routed by `[pid=N]` tag)
- Also prints stdout. `logs/` gitignored. Single-socket/stdin fallback supported.

Macros:

| Macro | Output | Prefix | Use |
|-------|--------|--------|-----|
| `serial_print!` | COM1 | none | simple kernel log, no newline |
| `serial_println!` | COM1 | none | kernel log + newline |
| `serial_println_core!` | COM1 | `[Core N | Xus]` | SMP aware logging |
| `serial2_print!` | COM2 | none | routing userspace writes only |

No `println!`/`print!` — no `std`, no VGA text mode handler.

---

## Repository Layout

```
MofuOS/
  GNUmakefile          — root build orchestrator
  Cargo.toml           — workspace root; kernel only member
  .cargo/config.toml   — target + linker flag
  rust-toolchain.toml  — nightly channel
  x86_64-kernel.json   — custom target spec
  linker-x86_64.ld     — kernel ELF layout
  limine.conf          — bootloader config (timeout 0, protocol limine)
  kernel/
    Cargo.toml
    build.rs           — assembles+links AP trampoline blob (cc/ld/objcopy → $OUT_DIR/ap_trampoline.bin via env AP_TRAMPOLINE_BIN)
    GNUmakefile
    src/
      main.rs          — binary entry: panic handler, QemuExitCode, main()
      boot.rs          — Limine requests, kmain() entry point
      lib.rs           — crate root: constants, modules
      boot_info.rs     — BootInfo struct + BOOT_INFO static (Once)
      gdt.rs           — per-core GDT/TSS
      interrupts.rs    — IDT, APIC, TSC calibration, timer
      process_start.rs — create_init_process, create_userspace_processes
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
    crt0.c/crt0.o      — minimal C runtime (_start → main → sys_exit)
    libc/              — syscall.h, syscall.o, types.h (bare-metal libc)
    programs/test/     — test.c userspace test suite (linked as TEST_ELF)
    programs/first/    — first.c minimal hello-world
    rustspace/x86_64-user.json + src/bin/{rust_first,window_app,theophe,fs_test}.rs
  disk_templates/fat32_os_disk_template_default/ — staged into storage/ata_disk.img via scripts/create_ata_disk.sh
  limine.conf
  storage/ata_disk.img — generated FAT32 ATA image (qemu ide-hd)
  scripts/log_splitter.py
```

---

## Boot Flow

### kmain in `kernel/src/boot.rs`

1. Assert `BASE_REVISION.is_supported()` (revision 5)
2. Get UEFI memory map, Limine memory map, paging mode, `HHDM offset`, `RSDP`, framebuffer
3. Init framebuffer `graphics::framebuffer::init_framebuffer`
4. Store `BootInfo { hhdm_offset }` in `BOOT_INFO` (`Once`)
5. BSP GDT `init_core_gdt(0)` + load IDT
6. Init offset page table `init_offset_page_table(hhdm_offset)` (reads `CR3`, adds `HHDM offset` → `OffsetPageTable`)
7. Init frame allocator `MemoryMapFrameAllocator::init`
8. Init heap `allocator::init_heap` at `HEAP_POINTER = 0xFFFF_8080_0000_0000`, `16 MB`
9. Init ACPI `interrupts::init_acpi`, parse MP response, init CPU info `init_cpu_info`, `init_cpu_infos`
10. Map LAPIC for core 0, init LAPIC timer
11. Init `CORE_POOL` + `SCHEDULER` with core count, `init_current_core()` for BSP
12. Init global memory globals `FRAME_ALLOCATOR`, `USER_MEMORY_MANAGER`, calibrate TSC `init_tsc_globals` via PIT
13. Bootstrap AP core 1: `cpus[1].bootstrap(ap_core_from_limine_entry_point, 0x12345678)`
14. Enable interrupts, call `main()` in `kernel/src/main.rs`

### main in `kernel/src/main.rs`

1. `init_syscall_stack()` (now inside `init_syscall()`)
2. `create_userspace_processes()` — parses `TEST_ELF`, creates process via `PROCESS_MANAGER`, enqueues on core 1 queue
3. Init framebuffer, create `Compositor` + windows, `test_graphics::draw_shapes`, create `Theophe` for CPU info text, `compositor.compose`
4. Render 3D test (textured cube) into window via `RenderContext`
5. Enter infinite render loop (gated by `RENDER_SHADERS: bool = false` — currently disabled)

### AP Core Boot `ap_core_from_limine_entry_point` in `kernel/src/util/cpuinfo.rs`

Per AP (core 1+):
1. `init_core_gdt(core_id)` — own GDT+TSS
2. Map LAPIC for this core
3. `init_lapic_for_current_core(core_id)` — enable LAPIC (`SVR` bit 8), timer
4. `init_syscall()` — write `STAR`/`LSTAR`/`SFMASK` MSRs, set `KERNEL_GS_BASE` to `&PER_CORE_SYSCALL[core_id]`
5. Enable interrupts
6. `run_on_core_loop(core_id)` — scheduler loop, never return

---

## Key Constants `kernel/src/lib.rs`

```rust
HHDM_OFFSET: u64 = 0xFFFF_8000_0000_0000   // Higher-Half Direct Map base (expected; runtime value in BootInfo::hhdm_offset)
MAX_CORES: u8 = 16                            // hard limit; core pool uses u64 bitmap; const_assert!(MAX_CORES <= 64)
AP_CORE_COUNT: u8 = MAX_CORES - 1
HEAP_POINTER = 0xFFFF_8080_0000_0000          // 16 MB
LAPIC_VIRT_BASE = 0xFFFF_FFFF_0000_0000
IOAPIC_VIRT_BASE = 0xFFFF_FFFF_FF00_0000
USER_STACK_TOP = 0x7FFF_FFFF_F000
USER_WINDOW_BUFFER_BASE = 0x0000_0001_0000_0000 // + id*8MB (back), +8MB offset front; MAX_WINDOW_BUFFER_SIZE=8MB
USER_MEM_ALLOC_START = 0x1000_0000 // circular buffer alloc region up to 0x5000_0000
INVALID_PID = usize::MAX
INVALID_WINDOW_ID = u32::MAX
RFLAGS_DEFAULT = 0x202 // IF=1, reserved bit 1=1
DEFAULT_NEW_PROCESS_STACK_SIZE = 1 * 1024 * 1024 // 1 MB
SYSCALL_STACK_SIZE = 4096 * 16 // 64 KiB per core
```

QEMU exit device port `0xF4` size `4`: write `0x10` success (QEMU 33), `0x11` failed (QEMU 35). Panic handler calls `exit_qemu(QemuExitCode::Failed)`.

---

## Memory Subsystem `kernel/src/memory/`

### Address Space Layout

| Region | Virtual Address | Notes |
|--------|-----------------|-------|
| Kernel image | `0xffffffff80000000+` | Limine higher-half |
| HHDM | `0xFFFF_8000_0000_0000+` | `HHDM_OFFSET` |
| Kernel heap | `0xFFFF_8080_0000_0000` | `HEAP_POINTER`, `16 MB` |
| LAPIC MMIO | `0xFFFF_FFFF_0000_0000` | `LAPIC_VIRT_BASE` |
| IOAPIC MMIO | `0xFFFF_FFFF_FF00_0000` | `IOAPIC_VIRT_BASE` |
| User stack top | `0x7FFF_FFFF_F000` | `USER_STACK_TOP` |
| User heap | `0x0000_0000_6000_0000` | `heap_start` in `ProcessMemoryLayout` |
| User window buffers | `0x0000_0001_0000_0000+` | per `window_id*8MB` |
| User circular buffers | `0x1000_0000..0x5000_0000` | `allocate_virtual_range` |

PML4 split: `0..255` user, `256..511` kernel. New user PML4 copies kernel entries `256..511` from BSP PML4 — ensures kernel accessible in user space. LAPIC mapping must exist in kernel PML4 before address space creation (verified via `pml4[lapic_pml4_idx]`).

### Frame Allocator `memory/memory.rs`

`MemoryMapFrameAllocator` — bump allocator over Limine `MEMMAP_USABLE`. No `deallocate_frame` yet (leak). Fields `memory_map`, `curr_region_index`, `frame_offset_in_region`. Global `FRAME_ALLOCATOR: Once<Mutex<MemoryMapFrameAllocator>>`. No frame reclamation — leaked on `PageAlreadyMapped` (`BUG-05`) and on process exit (`ISSUE-M1`).

`init_offset_page_table(hhdm_offset)` helpers `align_up`/`align_down`.

### Heap `memory/allocator.rs`

Two-tier: fixed-size slab lists `[8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096]` + fallback `linked_list_allocator::Heap`. `#[global_allocator]` static `ALLOCATOR: MutexWrapper<FixedSizeBlockAllocator>`. `init_heap` maps heap pages then `init_fallback_allocator`. Debug flag `ALLOC_DEBUG`.

### User Memory Manager `memory/usermem.rs`

```rust
pub struct UserMemoryManager {
    pub kernel_page_table_phys: PhysAddr,
    pub phys_offset: u64, // HHDM_OFFSET
}
```

Global `USER_MEMORY_MANAGER: Once<Mutex<UserMemoryManager>>`. Access `memory::get_user_mem_mgr()`.

- `allocate_new_address_space` — alloc 4KiB frame for new PML4, zero, copy kernel entries `256..511`, return phys.
- `map_virt_mem_region` — per page alloc frame, `map_to` with `protection_flags | USER_ACCESSIBLE`, on `PageAlreadyMapped` merges flags (`OR` caps, `AND` `NO_EXECUTE`), leaked frame on miss.
- `create_main_stack` — maps `stack_size` ending at `USER_STACK_TOP`, flags `PRESENT|WRITABLE`
- `translate_user_virt_to_phys` — walks `PML4→PDPT→PD→PT` (handles 2MiB huge), used ELF load + window buffer map
- `get_page_flags` — walk to read `PageTableFlags`
- `map_specific_frame` — maps explicit `phys_addr` into user PML4 with `PRESENT|WRITABLE|USER_ACCESSIBLE|NO_EXECUTE` (no alloc), used window buffers + circular buffers
- `translate_kernel_heap_virt_to_phys` removed — use page-table walk via `kernel_page_table_phys`, never `vaddr - HHDM_OFFSET` for heap (latter bug `BUG-11`)

`ProcessMemoryLayout` in `process/process_mem.rs`:

```rust
pub struct ProcessMemoryLayout {
    pub top_page_table_phys: PhysAddr,
    pub stack_top: VirtAddr,
    pub stack_size: u64,
    pub heap_start: VirtAddr, // 0x6000_0000
    pub heap_end: VirtAddr,
    pub mapped_regions: Vec<MappedMemoryRegion>,
}
```

`grow_heap` bump at `0x6000_0000` via `sys_allocate(5)`, maps `PRESENT|WRITABLE|USER_ACCESSIBLE|NO_EXECUTE`.

Globals accessor:

```rust
memory::init_memory_globals(frame_allocator, user_mem_manager);
memory::get_frame_allocator() -> MutexGuard<MemoryMapFrameAllocator>
memory::get_user_mem_mgr()    -> MutexGuard<UserMemoryManager>
```

---

## Process Subsystem `kernel/src/process/`

Files: `mod.rs`, `process.rs`, `process_manager.rs`, `process_mem.rs`, `elf_loader.rs`, `scheduler.rs`, `core_pool.rs`, `kernel_thread.rs`, `execution.rs`, `syscall.rs`, `process_start.rs`

### Process Model

Monolithic, multiple processes share AP core via scheduler queue — not hard-pinned, assigned to least-loaded AP at creation (`scheduler.ready_count_on_core()`).

Root arche PID 0, children re-parented to arche if parent exits without cascade.

Embedded ELFs `elf_loader.rs`: `TEST_ELF` (`include_bytes!("../../../user/programs/test/test")`), `PING_ELF` (`include_bytes!(".../ping/ping")`), `ODYS_ELF` for `CreateProcess`. `create_userspace_processes` currently launches two `ping` instances PID 1/2 on core 1.

### Core Types

```rust
pub struct Process {
    pub pid: PID, pub parent_pid: PID,
    pub priority: u8, // 0-7, MAX_PRIORITY=7
    pub state: ProcessState, // Ready/Running/Waiting/Terminated
    pub name: String, pub children: Vec<PID>,
    pub file_descriptors: Vec<FileDescriptor>,
    pub resources: ProcessResources,
    pub exit_code: Option<i32>, pub is_out: bool,
    pub execution_context: ExecutionContext,
    pub memory_layout: ProcessMemoryLayout,
}
pub struct ExecutionContext {
    rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp, r8-r15, rip, rflags,
    page_table_base_phys: u64, // CR3
}
pub struct KernelThread { pub pid: PID, pub priority: u8, pub name: String, pub state: ThreadState, pub context: ExecutionContext, pub core_id: Option<u8> }
```

`ThreadGroup` holds PID + `KernelThread` (1:1 with Process).

`FileDescriptor { pub node_id: usize, pub flags: u8 }` (`FD_FLAG_READ=0x01`, `FD_FLAG_WRITE=0x02`), no per-fd offset — offset explicit per `ReadFile`/`WriteFile` arg. Swap-remove on close — fd indices unstable.

### Process Manager `PROCESS_MANAGER: Mutex<ProcessManager>` `lazy_static!`

`new_pid` starts `1` (arche 0). `create_process_from_elf`: verify parent, assign pid, pick least-loaded core via `CORE_POOL`/`SCHEDULER`, `Process::create_with_elf` (maps ELF segments with `PageTableFlags` from `PF_` bits, copy data + zero BSS via `translate_user_virt_to_phys`, create stack, build `ExecutionContext::new(entry, stack_top, pml4_phys)`), create `KernelThread`, `assign_to_core`, create `ThreadGroup`. **Critical order**: push `process` + `thread_group` vectors **first**, then `SCHEDULER.lock().enqueue_on_core(core_id, new_pid, priority)` — else AP dequeue finds `ProcessNotFound` and jumps with invalid context.

`terminate_process` sets `Terminated`, terminates thread, `CORE_POOL.release_core`, cascade or orphan children to arche.

`get_process`/`get_process_mut` linear `Vec` scan O(n).

### ELF Loader `process/elf_loader.rs`

`ElfLoadInfo::from_elf_data` uses `elf::ElfBytes`, validates `EM_X86_64`, collects `PT_LOAD` `LoadSegment {vaddr, filesz, memsz, flags, data}`, `merge_segments` sorts by vaddr, merges overlapping (OR flags), returns `{entry_point, min_vaddr, max_vaddr, segments}`.

### Scheduler `SCHEDULER: Mutex<Scheduler>`

```rust
pub struct Scheduler { per_core: Vec<CoreScheduler>, core_count: u8 }
pub struct CoreScheduler {
    core_id: u8,
    ready_queues: [Dequeue<PID>; 8], // 8 priority levels, 7 highest, FIFO pop_front
    blocked_queue: Dequeue<PID>,
    current_thread: PID,
}
```

`run_on_core_loop(core_id)`:

```
loop:
  disable interrupts
  get_next_on_core(core_id) -> (pid, prio) // highest non-empty queue
  if pid==INVALID_PID: enable interrupts, hlt, continue
  set_current_on_core(core_id, pid)
  extract exec context from PROCESS_MANAGER (lock then drop)
  set_current_process_for_core(core_id, pid)
  Cr3::write(user_pml4)
  enable interrupts
  asm: lea rax,[rip+2f]; push rax; mov [KERNEL_RSP_ON_CORE[core_id]], rsp; jmp jump_to_userspace
  2: mark_core_idle(core_id); if !terminated {re-enqueue(pid,prio)} else {log terminated}
```

`KERNEL_RSP_ON_CORE: [AtomicU64; MAX_CORES]` set before `jmp`, read by `return_to_scheduler()`: `mov rsp, KERNEL_RSP_ON_CORE[core_id]; ret` (pops label, resumes loop). Used by `sys_yield`/`sys_exit` + timer preemption.

`CURRENT_PROCESS_ON_CORE: [AtomicU64; MAX_CORES]` stores pid per core, `INVALID_PID` = idle. Used syscall to identify caller.

`SCHEDULER_STACKS: [KernelStack; MAX_CORES]` `32 KiB` each, `get_scheduler_stack_top(core_id)` — switched at `run_on_core_loop` entry because Limine bootstrap stack too small.

### Core Pool `CORE_POOL: Mutex<CorePool>`

```rust
pub struct CorePool {
    total_cores: u8,
    available_cores: u64, // bitmask, bit N=1 free
    core_assignments: [usize; MAX_CORES],
    lapic_ids: [u8; MAX_CORES],
}
```

Core 0 reserved `available_cores &= !1`. `allocate_core(pid)` linear scan first free bit. `release_core(core_id)` sets bit. Limit `MAX_CORES=16` via `u64` bitmap.

### Jump to Userspace `process/execution.rs`

```rust
pub unsafe fn jump_to_userspace(entry_point: u64, stack_pointer: u64, rflags: u64) -> !
```

Disable interrupts, load user DS/ES/FS/GS, build `iretq` frame `SS,RSP,RFLAGS,CS,RIP`, `iretq` → ring3. `rflags` `0x202` first entry, saved `frame.rflags` on resume.

### Syscall Interface `process/syscall.rs` — Canonical Ref `docs/syscalls.md`

Mechanism `SYSCALL`/`SYSRET` (64-bit, ~30 cycles). MSRs per AP core `init_syscall()`:

```rust
EFER.SCE = 1                 // 0xC0000080 bit 0
STAR  = (0x10 << 48) | (0x08 << 32) // 0xC0000081: 63:48 user CS base 0x10, 47:32 kernel CS 0x08
LSTAR = syscall_handler as u64      // 0xC0000082
SFMASK = 1 << 9                     // 0xC0000084 — mask IF on entry (BUG-09)
KERNEL_GS_BASE = &PER_CORE_SYSCALL[core_id] as u64 // 0xC0000102
```

GDT: `0x08` kernel code DPL0 L=1, `0x10` kernel data DPL0, `0x18` user data DPL3, `0x20` user code DPL3 L=1, `0x28` TSS (2 slots). `SYSRET` then `CS=0x23` (`0x20|3`), `SS=0x1B` (`0x18|3`).

CPU `SYSCALL` actions: `RCX=RIP`, `R11=RFLAGS`, `RFLAGS &= ~SFMASK` (clears `IF`), `CPL=0`, `CS=0x08`, `SS=0x10`, `RIP=LSTAR`, no stack switch — kernel must `swapgs`+`gs:0`.

Userspace convention `user/rustspace/src/lib.rs:224-244` `syscall6`: `rax=num`, `rdi=arg1`, `rsi=arg2`, `rdx=arg3`, `r10=arg4`, `r8=arg5`, `r9=arg6` (`r10` not `rcx` because `SYSCALL` clobbers `rcx`).

#### syscall_handler Naked Asm `syscall.rs:154-206`

Per-core `64 KiB` stack `PER_CORE_SYSCALL[core_id]: PerCoreSyscallData` (`#[repr(C, align(64))]`, `stack_top: u64` first field, `_stack: [u8; 65536]`), pointed via `KERNEL_GS_BASE`:

```asm
swapgs; mov r15,rsp; mov rsp,gs:0; swapgs
push r15; push rcx; push r11; push rax
push rdi; push rsi; push rdx; push r10; push r8; push r9
push rbx; push rbp; push r12; push r13; push r14
mov rdi,rsp; call handle_syscall_inner // sti at top, cli before return_to_scheduler
pop r14; pop r13; pop r12; pop rbp; pop rbx
pop r9; pop r8; pop r10; pop rdx; pop rsi; pop rdi
add rsp,8 // skip syscall_num slot — rax holds return value
pop r11; pop rcx; pop r15; mov rsp,r15; sysretq
```

`SyscallFrame` `#[repr(C)]` lowest addr first: `r14,r13,r12,rbp,rbx, arg6(r9),arg5(r8),arg4(r10),arg3(rdx),arg2(rsi),arg1(rdi), syscall_num(rax), rflags(r11), user_rip(rcx), user_rsp(r15)`.

`handle_syscall_inner` does `sti` early (preemptible long syscalls), `cli` before `return_to_scheduler` in `Yield`/`Exit`.

Intel `SYSRETQ` bug (`BUG-09`): Intel sets `SS = STAR[63:48]+8` without ORing `RPL=3` → `SS=0x18` RPL0 in userspace (`qemu64` Intel-mode). Timer firing in Ring3 captures `SS=0x18`, `iretq` faults `#GP` because `SS.RPL != CS.RPL`. Fix: `SFMASK=1<<9` + `sti` inside handler so timer never fires with kernel `SS`; if Ring3 and work waiting → `return_to_scheduler()` bypass `iretq`; if Ring3 nothing waiting → patch `*(stack_frame_ptr+4)=0x1B` (`SS` slot at `+32`).

#### Pointer Validation

`validate_user_ptr(ptr,len)`: `ptr!=0 && end=ptr+len <= 0x0000_7FFF_FFFF_FFFF && !wraparound`. All ptr syscalls check first, return `u64::MAX` on fail. TODO walk page tables for `USER_ACCESSIBLE`.

#### Syscall Table (complete, `syscall.rs:215-1156`)

| Num | Name | Args | Ret | Notes |
|-----|------|------|-----|-------|
| 0 | `CreateProcess` | `rdi=path_ptr, rsi=path_len, rdx=name_ptr, r10=name_len` | `0` ok, `MAX` err | partial — parses `ODYS_ELF` + `create_process_from_elf`, path not wired `ISSUE-P8` |
| 1 | `TerminateProcess` | — | `MAX` | stub |
| 2 | `Write` | `rdi=fd(1/2), rsi=buf, rdx=count` | `count` | `fd==1/2` → `COM2` `serial2_print!("[pid=N] s")` to `userspace_pid_N.txt` |
| 5 | `Allocate` | `rdi=size` | `old_heap_end:u64` or `MAX` | bump at `0x6000_0000`, maps `PRESENT|WRITABLE|USER_ACCESSIBLE|NO_EXECUTE`; rustspace `Arena` `64KiB` slabs |
| 10 | `CreateWindow` | `rdi=w, rsi=h, rdx=x, r10=y` | `window_id:u32` | `compositor.create_window(w,h,x,y,pid)` → `Window {Arc<WindowBuffer 2*heap alloc> + EventBuffer}`, `z=7` |
| 11 | `DestroyWindow` | `rdi=window_id` | `0` | `is_visible=false`, recycle id |
| 12 | `MapWindowBuffer` | `rdi=window_id` | `user_base:u64` or `MAX` | maps **both** back+front: `0x0000_0001_0000_0000+id*8MB` (back), `+8MB` front, `MAX_WINDOW_BUFFER_SIZE=8MB`, walks heap via page-table walk (BUG-11 fix), `map_specific_frame` |
| 13 | `PresentWindow` | `rdi=window_id` | `0` | `buffer.present()` sets `needs_swap=true` |
| 14 | `GetWindowSize` | `rdi=window_id` | `(width<<32)|height` or `MAX` | reads `WindowBuffer` |
| 15 | `FocusWindow` | `rdi=window_id` | `0` | `max_z+1`, normalize if `>250` |
| 16 | `GetWindowInfo` | `rdi=window_id, rsi=info_ptr` | `0` ok, `MAX` err | fills `WindowInfo {width,height,event_buffer_vaddr}` (`window.rs:33-38`) |
| 20 | `OpenFile` | `rdi=path_ptr, rsi=path_len, rdx=flags` | `fd:usize` or `MAX` | `sirius.resolve_path` → `node_id`, `flags=0x01 READ 0x02 WRITE` |
| 21 | `CloseFile` | `rdi=fd` | `0` ok, `MAX` err | swap-remove |
| 22 | `ReadFile` | `rdi=fd, rsi=offset, rdx=buf_ptr, r10=count` | `bytes_read` or `MAX` | validates buf, checks `FD_FLAG_READ`, `sirius.driver.read_file(node_id,offset,&mut)` |
| 23 | `WriteFile` | `rdi=fd, rsi=offset, rdx=buf_ptr, r10=count` | `bytes_written` or `MAX` | checks `WRITE`, writeback if cached (dirty, flush on evict) |
| 24 | `StatFile` | `rdi=path_ptr, rsi=path_len, rdx=stat_ptr` | `0` ok, `MAX` err | → `StatFlat` |
| 25 | `ListDir` | `rdi=path_ptr, rsi=path_len, rdx=out_ptr, r10=out_len` | `entry_count` or `MAX` | → `DirEntryFlat` array |
| 26 | `CreateFile` | `rdi=path_ptr, rsi=path_len` | `0` ok, `MAX` err | via Sirius |
| 27 | `CreateDir` | `rdi=path_ptr, rsi=path_len` | `0` ok, `MAX` err | via Sirius |
| 28 | `Delete` | `rdi=path_ptr, rsi=path_len` | `0` ok, `MAX` err | file or empty dir |
| 30 | `PinFile` | `rdi=path_ptr, rsi=path_len` | `0` ok, `MAX` err | cache `Resident` (cfg `use_cached_fs`) |
| 31 | `UnpinFile` | `rdi=path_ptr, rsi=path_len` | `0`/`MAX` | `Normal` |
| 32 | `ReserveCache` | `rdi=path_ptr, rsi=path_len, rdx=importance(0..6)` | `0`/`MAX` | `CacheImportance` hint, currently dead `A0-3` |
| 33 | `EvictDirectory` | `rdi=path_ptr, rsi=path_len` | `freed_bytes` or `MAX` | drop cached children, dead `A0-3` |
| 34 | `GetCacheStats` | `rdi=stats_ptr` | `0` or `MAX` | → `CacheStatsFlat {total_files,total_bytes,max_bytes,dirty_files}` |
| 35 | `FlushFileCache` | — | `0` or `MAX` | flush open fds of caller `sirius.flush_nodes(&node_ids)`; global `flush_all_file_caches()` flushes all dirty |
| 600 | `CreateCircularBuffer` | `rdi=size, rsi=page_flags, rdx=info_ptr` | `0` ok, `MAX` err | `data_size=align_up(size,PAGE_SIZE)`, `total=2*data_size`, alloc `data_size/PAGE` frames, map twice at `virt_base + i*PAGE` and `+data_size + i*PAGE`, returns `CircularBufferInfo {virtual_base, view_size, total_virtual_size}` |
| 970 | `GetCpuInfo` | `rdi=buf_ptr, rsi=buf_len` | `0` ok, `2` err | fills `CpuInfoFlat` (vendor, family/model/stepping, cache_line, apic_id, features, tsc_frequency_hz, boot_tsc, max_cpuid, core_count) |
| 997 | `GetPID` | — | `pid:u64` | `CURRENT_PROCESS_ON_CORE[core_id]` |
| 998 | `Yield` | — | `0` | save `user_rip/rsp/rflags` → `return_to_scheduler()`, re-enqueue |
| 999 | `Exit` | `rdi=exit_code:i32` | `!` | `terminate_process(pid, code, false)` orphans children to arche 0 → `return_to_scheduler()` |

Unlisted → `u64::MAX`. Stubs: `1,3,4,8,9,996`.

Wire types (`kernel` ↔ `user/rustspace/src/lib.rs` must match, `FS_NAME_LEN=16` = `DIR_ENTRY_NAME_LEN`):

- `DirEntryFlat` `32B`: `name[16], name_len:u8, is_dir:u8, _pad[6], size:u64, created:u32, modified:u32`
- `StatFlat` `32B`: `name[16], name_len, is_dir, size, created, modified` (no `_pad`)
- `WindowInfo` `16B`: `width:u32, height:u32, event_buffer_vaddr:u64`
- `CpuInfoFlat` `32B align(16)`: vendor, family, model, stepping, display_family:u16, display_model, cache_line_size, apic_id, features:u32, tsc_frequency_hz:u64, boot_tsc:u64, max_cpuid_leaf:u32, max_extended:u32, core_count:u8
- `CircularBufferInfo` `24B`: `virtual_base:u64, view_size:u64, total_virtual_size:u64`
- `CacheStatsFlat` `32B`: `total_files,total_bytes,max_bytes,dirty_files:u64`
- `EventBuffer` `4KiB align(4096)`: `write_idx,read_idx,event_count:AtomicU32 + events[MAX_EVENT_COUNT]`

Userspace wrappers: Rust `user/rustspace/src/lib.rs:223-547` `syscall6` (`r10` dance), typed `sys_*`, `Arena` `64KiB` slabs via `sys_allocate`; C `user/libc/syscall.h` + `syscall.c` + `crt0.c`.

---

## Graphics Subsystem `kernel/src/graphics/`

Pixel format XRGB8888 `u32`: bits `31-24` ignored, `23-16` R, `15-8` G, `7-0` B. `rgba_to_xrgb(Rgba8888UNORM) -> u32` canonical. `FRAMEBUFFER_BYTES_PER_PIXEL=4`. Limine `red_mask_*` etc not checked — hardcoded ` (r<<16)|(g<<8)|b` works QEMU XRGB, fails real hardware BGRX (`BUG-06`).

### Framebuffer `graphics/framebuffer.rs`

Wraps Limine response, `NonNull<()>` linear fb memory. Methods `write_pixel(x,y,Rgb888)` bounds-checked, `fill_rect`, `copy_row`, `embedded_graphics::DrawTarget`. Global `FRAMEBUFFER_TARGET: Once<Mutex<FrameBufferTarget>>`, `init_framebuffer(fb)`, `get_framebuffer()`.

### Window System `window.rs`

```rust
pub struct Window { pub id: WindowID, pub x: i32, pub y: i32, pub z_index: u8, pub is_visible: bool, pub buffer: Arc<WindowBuffer> }
pub struct WindowBuffer {
    pub width: u32, pub height: u32, pub x: i32, pub y: i32,
    back_buffer: UnsafeCell<NonNull<u32>>, front_buffer: UnsafeCell<NonNull<u32>>,
    needs_swap: AtomicBool, swap_count: AtomicU32,
}
```

Double-buffered, each buffer `width*height*4` heap-allocated page-aligned. Processes write back via `back_buffer_mut() -> WindowBackBuffer`, `buffer.present()` sets `needs_swap`, compositor `try_swap()` pointer-swaps atomically if set, then reads front. `WindowBackBuffer` exposes `write_pixel`, `write_pixel_unchecked`, `clear`, `as_slice_mut()`, `DrawTarget`. `Rect` helpers `contains`, `intersects`, `get_intersection_rect`, `get_union_rect`.

### Compositor `graphics/compositor.rs`

```rust
pub struct Compositor {
    framebuffer_width: u32, framebuffer_height: u32,
    next_window_id: AtomicU32,
    currently_focused_window: Mutex<WindowID>,
    free_window_ids: Mutex<Vec<WindowID>>,
    windows: RwLock<Vec<Window>>,
}
```

`create_window(w,h,x,y) -> (WindowID, Arc<WindowBuffer>)` alloc `WindowBuffer`, `z=0`, push `windows`, set focused. `compose(&mut FrameBufferTarget)`: read-lock, collect visible, sort by `z_index` asc, `try_swap()` per window, clip to fb bounds, `copy_nonoverlapping` row-by-row XRGB. No alpha blending, no dirty region tracking. `focus_window` sets `z = max_z+1`, normalize if `>250`. `destroy_window` marks invisible, recycles id.

Userspace window rendering `sys_map_window_buffer`: kernel creates `WindowBuffer` heap, maps back+front phys pages via page-table walk `USER_ACCESSIBLE` at `USER_WINDOW_BUFFER_BASE + window_id*8MB`, returns base. User writes XRGB `u32`, `sys_present_window` triggers swap, compositor blits front.

**Critical**: heap at `0xFFFF_8080_0000_0000` not HHDM-mapped — walk page table, not `vaddr - HHDM_OFFSET`.

### Software Rendering Pipeline `pipeline.rs`, `renderer.rs`, `resources.rs`, `shaders.rs`, `transform.rs`

Fixed-function software rasterizer, pluggable vertex/pixel shaders.

```rust
pub struct Vertex2D { pub xyuv: f32x4 } // [x,y,u,v] SIMD
pub struct Vertex3D { pub pos: f32x4, pub uv: f32x4, pub norm: f32x4 }
pub struct PipelineState {
    pub vs: Box<dyn VertexShader>, pub ps: Box<dyn PixelShader>,
    pub vertex_layout: VertexLayout, pub rasterizer_state: RasterizerState,
    pub blend_state: BlendState, pub render_mode: RenderMode, pub depth_enabled: bool, pub depth_write: bool, pub depth_func: DepthFunc,
}
pub trait VertexShader: Send + Sync { fn run(&self, input: &VSIn, output: &mut VSOut, constants: &[ConstantBuffer]); }
pub trait PixelShader: Send + Sync { fn run(&self, input: &mut PSIn); }
```

`VSOut` clip-space `f32x4` + 8 interpolated attrs, `PSIn` gives interpolated attrs, screen x/y, `&mut [u32]` target, textures, cbuffers. `Texture {width,height,data:Vec<u32>}`, `ConstantBuffer {data:Vec<u8>}`, `RWBuffer`.

`RenderContext {textures:Vec<Texture>, cbuffers:Vec<ConstantBuffer>}`: `bind_texture`, `bind_cbuffer`, `begin_frame(back_buffer) -> RenderTarget`, `clear`. Built-ins `PassThroughVS`, `TextureSamplePS` (bilinear via `micromath`). `transform.rs` `Mat4x4` row-major, `perspective_matrix`, `view_matrix`, `model_matrix`.

`Theophe` `programs/theophe.rs` software text terminal renders into `WindowBackBuffer`, `write_line`, `write_str`, `render`. Kernel `text.rs` empty.

Graphics in `main.rs`: `FrameBufferTarget` → `test_graphics::draw_shapes` direct, create `Compositor` (twice currently — first dead code), create windows, `Theophe::new(window_buffer.back_buffer_mut())` CPU info, `focus_window(0)`, `compose`, 3D cube via `RenderContext`, loop gated `RENDER_SHADERS=false`.

Userspace renderer plan `docs/userspace/user_renderer.md` Option A: move `color`, `pipeline`, `renderer`, `resources`, `shaders`, `transform`, `theophe` to `user/rustspace/src/gfx/` as library, `UserSurface {pixels:*mut u32,width,height}` replaces `WindowBackBuffer`, kernel keeps `compositor`+`framebuffer`+`window`.

---

## Filesystem Subsystem — Sirius VFS `kernel/src/filesystem/`

```
kernel/src/filesystem/mod.rs — fat32, file_cache, sirius; re-exports
  sirius.rs — Sirius<D>, FilesystemDriver, CachedDriver<D>, FsDriverAdapter<D>, init_filesystem, init_filesystem_ata
  file_cache.rs — FileCache<D>, CacheFilesystemDriver, CacheImportance, CacheStats, writeback
  fat32/mod.rs — Fat32Driver read/write/create/delete
    direntry.rs — DirectoryEntry, FatFileAttributes, fat_time_to_unix_timestamp
    boot_sector.rs — BootSector parsing
    test_data.rs — create_fat32_image() helper
kernel/src/io/ata.rs — AtaPioDriver ATA PIO primary bus master DiskDevice
  disk.rs — DiskDevice, MockDiskDevice, DiskManager, DISK global
```

### How to Run with Real Disk

```bash
make run-x86_64-ata # QEMU: -device piix3-ide,id=ide -device ide-hd,drive=ata0,bus=ide.0,unit=0 -drive file=ata_disk.img,format=raw,id=ata0,if=none
bash scripts/create_ata_disk.sh ata_disk.img 16 # recreate 16MB FAT32 image
```

`ata_disk.img` raw FAT32 persistent across QEMU exit; `MockDiskDevice` not.

### Sirius VFS Type Hierarchy

With cache `Sirius<CachedDriver<Fat32Driver>>` → `CachedDriver` → `FileCache<FsDriverAdapter<Fat32Driver>>` → `Fat32Driver`. Without cache `Sirius<Fat32Driver>` direct. No dynamic dispatch, monomorphized.

Static concretely typed via `cfg`:

```rust
#[cfg(feature = "use_cached_fs")]
lazy_static! { pub static ref SIRIUS: Once<Mutex<Sirius<CachedDriver<Fat32Driver>>>> = Once::new(); }
#[cfg(not(feature = "use_cached_fs"))]
lazy_static! { pub static ref SIRIUS: Once<Mutex<Sirius<Fat32Driver>>> = Once::new(); }
```

`get_sirius()` returns concrete guard.

### FilesystemDriver Trait

```rust
pub trait FilesystemDriver: Send + Sync {
    fn read_file(&mut self, node_id, offset, out_buffer) -> FileSystemResult<usize>;
    fn write_file(&mut self, node_id, offset, data) -> FileSystemResult<usize>;
    fn find_node(&self, path: &str) -> FileSystemResult<FileNodeHandle>;
    fn get_node(&self, node_id) -> FileSystemResult<FileNode>;
    fn list_directory(&self, node_id) -> FileSystemResult<Vec<FileNode>>;
    fn create_file(&mut self, parent_id, name) -> FileSystemResult<FileNodeHandle>;
    fn create_directory(&mut self, parent_id, name) -> FileSystemResult<FileNodeHandle>;
    fn delete(&mut self, node_id) -> FileSystemResult<()>;
    fn root_node(&self) -> FileNodeHandle;
}
```

`Sirius<D>` path API: `resolve_path`, `open_file`, `list_directory`, `read_file(path,offset,&mut)`, `write_file`, `create_file`, `create_directory`, `delete`. Leading `/` optional, root `/`. Cache-only on `Sirius<CachedDriver<D>>`: `pin_file` (Resident never evict), `unpin_file` (Normal), `reserve_cache(path,importance)`, `evict_directory(path)`, `flush_node`, `flush_nodes`, `flush_all`, `cache_stats()`.

Init:

```rust
pub fn init_filesystem(fat32_image: &[u8]) -> Result<(), &'static str> // MockDisk
pub fn init_filesystem_ata() -> Result<(), &'static str> // ATA, default FS_CACHE_SIZE 64MB with use_cached_fs
pub fn init_filesystem_ata_with_cache(cache_size: usize) -> Result<(), &'static str>
```

Not called default boot — `main()` calls `test_ata_filesystem()` when testing.

`FileSystemError`: `NotFound, PermissionDenied, FileExists, IsDirectory, NotDirectory, DiskOpError, InvalidPath, FileSizeExceeded, InvalidFilename, DirectoryNotEmpty, NoSpace, DirectoryFull, IoError, NotSupported`.

### File Cache `file_cache.rs`

`FileCache<D: CacheFilesystemDriver>` owns driver, no dispatch. `CacheFilesystemDriver` whole-file ops: `read_file(node_id)->Vec<u8>`, `read_file_range`, `write_file`, `file_size`, `read_dir`. `FsDriverAdapter<D>` newtype wraps `FilesystemDriver` implements `CacheFilesystemDriver`, lives at `cache.driver.0`. `CachedDriver<D> { pub cache: FileCache<FsDriverAdapter<D>> }` implements `FilesystemDriver` transparently — delegates `find_node`, `get_node`, `list_directory`, `create*`, `root_node` to `cache.driver.0`; `delete` invalidates cache first.

Writeback: `write_file` in `CachedDriver`: if not cached load from disk first, patch in-mem via `apply_write_inmem`, mark `is_dirty=true`, return count, no disk write. Flush on `flush_file`, `flush_nodes(&[node_id])` (batch), `flush_all()`, or `evict_one` (flush before evict, skip on fail). Syscalls `30-35` handle pin/unpin/reserve/evict/stats/flush (`FlushFileCache` flushes open fds of caller via `syscall.rs:1216`).

Eviction LRU-weighted importance: score `(u64::MAX - priority)*age`, higher = evict candidate. `CacheImportance` `Minimal<Low<Normal<High<VeryHigh<Critical<Resident` (Resident never evict).

Known issue `A0-3`: `directory_children` never populated — `register_file_in_directory` never called. Decode `parent_cluster` from `FileNodeHandle` bits `55-32` on load, fix `get_effective_importance` to lookup parent, not file id. Then `reserve_cache`/`evict_directory` dead.

### FAT32 Driver

`FileNodeHandle` packing:

```
bit 63: reserved_flag (0)
bits 62-56: FAT attrs byte (7 bits)
bits 55-32: parent_cluster (24 bits, max 0xFFFFFF)
bits 31-0: file cluster (24 bits)
```

`encode_node_id(entry, parent_cluster)` / `decode_node_id`. `is_directory` via attrs, no disk read.

Boot sector geometry `Fat32Driver::new`:

```
fat_start_sector   = reserved_sectors
root_start_sector  = fat_start_sector + num_fats * fat_size_32
data_start_sector  = root_start_sector // root_dir_sectors 0 FAT32
cluster_size       = sectors_per_cluster * bytes_per_sector
total_sectors      = total_sectors_32 if !=0 else total_sectors_16 // mkfs.fat <=32MB uses 16-bit
available_sectors  = total_sectors - data_start_sector
max_cluster        = available_sectors / sectors_per_cluster + ROOT_CLUSTER(2)
```

FAT entry masking critical: `28 bits`, top 4 reserved. `read_fat_entry` → `raw & 0x0FFF_FFFF`, `write_fat_entry` RMW `new=(existing & 0xF000_0000)|(value & 0x0FFF_FFFF)`. Constants `END_OF_CHAIN 0x0FFF_FFFF`, `BAD_CLUSTER 0x0FFF_FFF7`, `FAT_ENTRY_RESERVED_BEGIN 0x0FFF_FFF8`, `END 0x0FFF_FFFE`. `write_fat_entry` only writes FAT1, FAT2 stale — violates spec, `fsck.fat` reports diff, power loss → cross-linked (`A1-1`).

Operations: `read_cluster_chain`, `get_cluster_chain_length`, `find_free_cluster` linear FAT scan (FSInfo hints unused), `allocate_clusters`, `free_cluster_chain`, `clear_clusters`, `expand_directory`, `read_directory_entries` (returns all incl deleted, callers `retain(|e|e.is_valid())`), `write_direntry` (re-reads entire chain per 32B entry — expensive), `find_free_slot_in_directory` (expand if none), `find_entry_by_cluster`. Filename `8.3` only (`set_filename` validates `stem<=8 ext<=3`, `InvalidFilename`), LFN not supported `ISSUE-F5`. No timestamp update.

### ATA PIO Driver `io/ata.rs`

Primary bus master unit 0, `28-bit LBA`, polling no DMA/IRQ. Ports `0x1F0` Data 16-bit, `0x1F1` Error, `0x1F2` Sector count, `0x1F3` LBA low, `0x1F4` mid, `0x1F5` high, `0x1F6` Drive/Head (`LBA[27:24]+0xE0`), `0x1F7` Status/Command, `0x3F6` Alternate status. Commands `READ_SECTORS 0x20`, `WRITE_SECTORS 0x30`, `CACHE_FLUSH 0xE7`, `IDENTIFY 0xEC`. After write issue `CACHE_FLUSH` poll `BSY` clear, `POLL_TIMEOUT_ITERS=100_000`.

---

## Hardware — GDT/IDT/TSS, APIC, SMP, Serial `kernel/src/gdt.rs`, `interrupts.rs`, `util/`, `asm/`, `io/serial.rs`

### GDT/TSS per Core `MAX_CORES=16`

Statics `.bss`:

```rust
static mut RSP0_STACKS:   [KernelStack; MAX_CORES] // 16*32KiB=512KiB, PER_CORE_STACK_SIZE=8*4096=32KiB
static mut IST0_STACKS:   [KernelStack; MAX_CORES] // 16*32KiB
static mut PER_CORE_GDT:  [Gdt; MAX_CORES]
static mut PER_CORE_TSS:  [UnsafeCell<TaskStateSegment>; MAX_CORES]
static mut PER_CORE_SYSCALL: [PerCoreSyscallData; MAX_CORES] // 64KiB each
static mut SCHEDULER_STACKS: [KernelStack; MAX_CORES] // 32KiB
```

GDT layout per core: `0 null`, `1 0x08` kernel code ring0, `2 0x10` kernel data ring0, `3 0x18` user data ring3, `4 0x20` user code ring3, `5 0x28+` TSS (2 slots). STAR userspace `CS 0x23` (`0x20|3`), `SS 0x1B` (`0x18|3`).

TSS: `privilege_stack_table[0]` = top `RSP0_STACKS[core_id]` (ring3→ring0, without it `RSP=0` → triple fault), `interrupt_stack_table[DOUBLE_FAULT_IST_INDEX=0]` = top `IST0_STACKS[core_id]` (double fault dedicated, else corrupt `RSP` → triple fault).

`init_core_gdt(core_id)`: build `Gdt::new(core_id)` sets `RSP0`/`IST0`, store `PER_CORE_GDT[core_id]`, `gdt.table.load()` LGDT, reload CS via `retfq` idiom, set DS/ES/SS kernel data, `load_tss(tss_selector)` LTR. Called BSP `kmain` + each AP boot. Selectors via `get_*_selector()` lookup `PER_CORE_GDT[get_current_core_id()]`.

### IDT/Interrupts `interrupts.rs`

`lazy_static! IDT: InterruptDescriptorTable`, handlers `page_fault`, `double_fault` (IST0), `general_protection_fault`, `timer` (APIC TSC-Deadline), `keyboard`, spurious. `load_idt()` called BSP only — APs share same virtual addr via shared kernel page table.

TSC/time:

```rust
static TSC_FREQUENCY_HZ: AtomicU64
static BOOT_TSC: AtomicU64
static SYSTEM_TICKS: AtomicU64
TIMER_TICK_INTERVAL_MS = 10 // TIMER_TICK_FREQ_HZ 100, TICK_DURATION_NS 10_000_000
```

`init_tsc_globals()` measures via PIT channel2 one-shot `~55ms`, records boot TSC. `tsc_timestamp_us()` for `serial_println_core!`, `system_uptime_ns()` via TSC.

APIC timer: `TSC-Deadline` (`MSR_IA32_TSC_DEADLINE 0x6E0`), per core programs `TSC + ticks_per_interval`, vector `0x20`. On tick: inc `SYSTEM_TICKS`, `SCHEDULER.lock().on_timer_tick(core_id)` (currently empty, preemption handled `timer_interrupt_handler`), re-arm deadline.

LAPIC `LAPIC_VIRT_BASE 0xFFFF_FFFF_0000_0000`, phys from `APIC_BASE MSR 0x1B`, `map_local_apic_for_current_core`, `init_lapic_for_current_core` enables `SVR` bit8 + timer. Helpers `util/apic.rs` `lapic_read`/`lapic_write` via `APICOffset` enum.

IOAPIC phys from ACPI MADT, virt `IOAPIC_VIRT_BASE 0xFFFF_FFFF_FF00_0000`, routes IRQ1 keyboard → `0x21`.

ACPI `init_acpi(rsdp_phys, hhdm_offset, mapper, frame_alloc)` via `acpi` crate `IdentityAcpiHandler` (typo), finds MADT for LAPIC/IOAPIC, maps IOAPIC MMIO, ISOs. Most `AcpiHandler` IO methods `todo!()` — only mapping works (`BUG-07`).

### SMP

QEMU configured `cores=2` (`cores=3` dev), `MAX_CORES=16` bitmap `u64`.

AP boot: `build.rs` assembles `src/asm/ap_trampoline.S` + `ap_trampoline.ld` → `objcopy -O binary` → `ap_trampoline.bin` embedded `env!("AP_TRAMPOLINE_BIN")` `asm/mod.rs`; BSP writes blob to phys `0x8000`, identity-maps `0x8000`/`0x9000` `setup_ap_trampoline_mapping`, `cpus[1].bootstrap(ap_core_from_limine_entry_point, 0x12345678)` sends `IPI SIPI` at trampoline; trampoline transitions real→prot→long, minimal GDT, `CR0/CR4/EFER`, load `CR3` from BSP, jump `ap_core_from_limine_entry_point`.

CpuInfo `util/cpuinfo.rs`:

```rust
pub struct CpuInfo {
    pub features: CpuFeatureFlags, pub cache_line_size: u8, pub apic_id: u8,
    pub family: u8, pub model: u8, pub stepping: u8, pub vendor: CpuVendor,
}
```

`CPU_INFO_PER_CORE: Once<Vec<CpuInfo>>`, `init_cpu_infos(&cpus)`, `get_cpu_info()`, `get_cpu_info_for_core(core_id)`. Flags `APIC, X2APIC, TSC, TSC_DEADLINE, SSE…AVX,AES,RDRAND,HYPERVISOR`. `get_current_core_id()` reads APIC `IDr` then translates via table.

### Serial `io/serial.rs`

`uart_16550` `Uart16550Tty<PioBackend>`:

| Port | Addr | Use |
|------|------|-----|
| COM1 | `0x3F8` | kernel `serial_print!` |
| COM2 | `0x2F8` | userspace `sys_write fd=1/2` |

`lazy_static! SERIAL1, SERIAL2: Mutex<Uart16550Tty<PioBackend>>`, `_print`/`_print2` disable interrupts while holding lock. Log macros `serial_print!`, `serial_println!`, `serial_println_core!` (`[Core N | Xus]` via `get_current_core_id()` + `tsc_timestamp_us()`), `serial2_print!` for syscall.

MSR helpers `util/msr.rs`: `msr_read(reg)->u64` `RDMSR`, `msr_write(reg,val)` `WRMSR` for `EFER 0xC0000080`, `STAR 0xC0000081`, `LSTAR 0xC0000082`, `SFMASK 0xC0000084`, `APIC_BASE 0x1B`, `IA32_TSC_DEADLINE 0x6E0`, `KERNEL_GS_BASE 0xC0000102`.

---

## Conventions & Gotchas `docs/conventions.md`

- `nightly 1.96.0-nightly`, `#![allow(warnings, unused)]` blanket suppressor in `lib.rs` (TODO remove).
- Unsafe: every `unsafe fn`/`unsafe {}` needs reason comment if not obvious; phys→virt `phys + HHDM_OFFSET` (runtime `BootInfo::hhdm_offset`), never deref phys directly; never hold `spin::Mutex` (`PROCESS_MANAGER`, `SCHEDULER`) across `jump_to_userspace` (`->!` noreturn, deadlock — extract data, drop lock, then jump); `static mut` per core indexed `core_id`; `UnsafeCell` for double-buffer swap relies single compositor thread.
- Naming: globals `UPPER_SNAKE_CASE` (`SCHEDULER`, `CORE_POOL`, `FRAME_ALLOCATOR`), constants `UPPER_SNAKE_CASE`, types `PascalCase`, funcs `snake_case`, per-core `PER_CORE_`, `Once<Mutex<T>>` singletons, sentinel `INVALID_PID = usize::MAX`, `INVALID_WINDOW_ID = u32::MAX`.
- Data structures `data_structures/`: custom `Vec`, `Dequeue` (no-std), prefer over `alloc::vec::Vec` for consistency.
- Error handling: no `std::error::Error`, enums `#[derive(Debug,Clone,Copy)]`, `Result<T,FooError>` (`FileSystemResult`, `DiskOpResult`, `ProcessError`, `ElfLoadError`, `MapToError`), no panics hot path, `assert!`/`debug_assert!` for invariants (`assert!(pid != INVALID_PID)`).
- Lock ordering `CORE_POOL → PROCESS_MANAGER → SCHEDULER → FRAME_ALLOCATOR/USER_MEMORY_MANAGER → SERIAL1/SERIAL2` — never hold SCHEDULER while acquiring PROCESS_MANAGER. Compositor `RwLock<Vec<Window>>`.
- File layout: new subsystems `kernel/src/<name>/mod.rs` with `pub use` re-exports, tests `kernel/src/tests_exp/test_<name>.rs`, userspace `user/programs/<name>/`.
- Arch constraints: monolithic, `no_std`, base `0xffffffff80000000`, HHDM `0xFFFF_8000_0000_0000` (use runtime `BootInfo` value for new mapping), MMX disabled, red zone disabled, `large` code model, `4-level` `4KiB` paging, XAPIC only (`X2APIC` disabled), SMP `BSP core0` kernel/graphics/process creation, `AP core1+` scheduler loops, currently `2` cores QEMU.
- Suppressed warnings known TODO.
- Pixel XRGB8888 `u32` via `rgba_to_xrgb`, not `to_u32_rgba`.
- Session notes `notes/agents/userspace/` (master index: see note drift `notes/agents/` vs `docs/` canonical — keep `docs/` source truth).

Process creation invariant: push `Process` + `ThreadGroup` to `PROCESS_MANAGER` vectors **before** enqueue PID `SCHEDULER`.

---

## Rust Coding Guidelines `docs/rust_coding_guidelines.md`

Performance first, trade security/clean code for max perf, CPU-friendly, keep `icache` small, linear cache-friendly access, Data-Oriented Design, `debug_assert!` for good state, vectorized maths (`glam`), avoid `std` math unless required, no dynamic alloc for lifetime objects, likely branch first, never dynamic `dyn` dispatch — static handler dispatch table, compile-time `const fn` when possible.

Data structures: `struct`/`enum`/`newtype` per domain, slice APIs for perf, ownership per field (`&str` vs `String`, `slices` vs `Vec`, `Arc<T>`, `Cow<'a,T>`), invariants via types (`NonZeroU32`, `Duration`, enums), bool flags → state enum.

Style: `impl` directly below type, group methods `constructors, getters, mutations, domain, helpers`, clear `new`/`with_capacity`, no magic numbers (`const SOME_FIELD_VALUE: u32 = 4`), traits `Display`/`Debug`/`From`/`Into`, avoid overuse `Result`/`Option` — `debug_assert` + placeholder well-defined state, derive macros, small declarative macros, build speed `mold`/`sccache`/`cargo check`/`workspace split`, maintainable modules, no overuse generics, `pub` fields ok, long expressive names `time_s` not `time`, no padded alignment, no special chars `──✔` nor emojis, ASCII only, comments only when non-obvious (`// inverse depth: clear to 0`), macros for target/config branching to reduce binary size.

---

## Resolved Bugs `docs/bugs.md`

`BUG-01` global syscall stack SMP corruption: fixed per-core `PER_CORE_SYSCALL[core_id]` + `KERNEL_GS_BASE` + `swapgs` `gs:0`.

`BUG-02` `sys_exit` halts core: fixed `return_to_scheduler()` `mov rsp,[slot]; ret` to label `2:` in `run_on_core_loop`, correct asm order `push rax` before `mov [slot],rsp` before `mov cr3,rcx`, per-core `SCHEDULER_STACKS`, register pinning `r8` slot, `rcx` cr3, `rdi` rip, `rsi` rsp, `lateout rax`.

`BUG-08` missing `push rax` in `SyscallFrame`: all syscalls hit `_=> MAX`, fixed `push rax` + `add rsp,8` pop skip.

`BUG-09` Intel `SYSRETQ` leaves `SS=0x18 RPL0` → timer `iretq` `#GP`: fixed `SFMASK=1<<9` + `sti` inside handler + `cli` before `return_to_scheduler` + preempt via `return_to_scheduler` bypass or patch `SS=0x1B`.

`BUG-10` `MapWindowBuffer` missing `USER_ACCESSIBLE`: fixed, also `map_specific_frame` ORs it.

`BUG-11` `translate_kernel_heap_virt_to_phys` `vaddr - HHDM_OFFSET` mapped garbage `~512GB`: fixed walk kernel page table via `kernel_page_table_phys`.

Remaining `BUG-05` frame leak `PageAlreadyMapped`, `BUG-06` framebuffer ignore Limine masks, `BUG-07` ACPI `todo!()` stubs.

---

## Known Issues & TODOs `docs/issues.md`, `docs/action_items.md` Summary

Blockers P0:

- `A0-1`/`ISSUE-M1`/`BUG-05` no frame reclamation — bump allocator, leak on overlap, no dealloc on `terminate_process`, OOM after ~10k cycles or steady leak. Fix: free list `Vec<PhysFrame>` or HHDM linked list, `deallocate_frame`, lazy alloc fix, free on exit walk `mapped_regions`+stack+PML4 + intermediate tables.
- `A0-2`/`ISSUE-M4` no guard pages kernel stacks (`RSP0`, `IST0`, `SCHEDULER_STACKS`, `PER_CORE_SYSCALL`) — overflow silent corrupt next core GDT/TSS → triple fault. Fix: unmap guard page `virt - 4096` after heap init, `align(4096)`, `debug_assert!` rsp bounds.
- `A0-3` cache `directory_children` never populated `reserve_cache`/`evict_directory` dead. Fix: decode `parent_cluster` bits `55-32` on load, `register_file_in_directory`, lookup parent in `get_effective_importance`.

High P1:

- `A1-1`/`ISSUE-F6` FAT mirror FAT2 not updated — `fsck.fat` diff, cross-linked on power loss. Fix: write both `FAT1` and `FAT2` (`fat_start + fat_size_32*bytes_per_sector`).
- `A1-2`/`BUG-06` framebuffer hardcoded XRGB — wrong colors real hardware. Fix: store `red_mask_shift` etc at `init_framebuffer`, compose via masks.
- `A1-3`/`BUG-07` ACPI IO stubs `todo!()` panic on AML/PCI. Fix: HHDM `read_volatile`/`port` impl or graceful no-op with log.
- `A1-4`/`ISSUE-P9` tight syscall loop starves preemption `100Hz` + `~10 instr` Ring3 window → hog core. Fix: preemption check at syscall exit if `ready_count>1` → `return_to_scheduler`, keep `SFMASK`+`sti`.
- `A1-5` wire type fragility `FS_NAME_LEN` vs `DIR_ENTRY_NAME_LEN` must match, `validate_user_ptr` not checking `USER_ACCESSIBLE`. Fix: `const_assert!(size_of::<DirEntryFlat>()==32)`, `validate_user_slice` walk page tables per page.
- `A1-6`/`ISSUE-P5` single-core userspace bottleneck core0 reserved, QEMU `cores=2` → one runnable. Fix: test `cores=4` with `MAX_CORES=16`, fix `log_splitter.py` mismatch, consider BSP userspace when idle.
- `A1-7` HHDM misuse class `BUG-11` pattern — enforce page-table walk only.

Medium P2: paging hygiene (PML4 copy check `lapic_pml4_idx` `debug_assert`), `2MiB` huge keep 4KiB, TLB shootdown future, lock ordering enforce, `on_timer_tick` accounting, `Dequeue` O(n) ok, `write_direntry` batch, `find_free_cluster` FSInfo hints, timestamps, cache size tunable, graphics double `Compositor::new` dead code delete, dirty rect, alpha blend, compositor own framebuffer, input routing, AP trampoline `0x8000` remove if Limine suffices, `X2APIC`, `4+` core testing, ELF `TEST_ELF` baked → wire `Sirius` to `sys_create_process(path)`, `create_process` dead path, `rustspace` `Arena` leak track, window double-buffer contract.

Low P3: drop `#![allow(warnings,unused)]` → `clippy` CI, `no_std` test harness `make check` with `isa-debug-exit`, formatting, `build.rs` pin, doc sync `docs/` canonical vs `notes/agents/` drift, `reading_asm_with_addresses.md` objdump recipes, `FAT32 8.3`, `FSInfo`, `text.rs`, `RENDER_SHADERS`.

Suggested order Week1 `A0-1` frame free + `A0-2` guard + `A0-3` cache; Week2 `A1-1` FAT mirror + `A1-2` fb + `A1-3` ACPI + `A1-4` preempt + `A1-5` wire + `A1-7` HHDM + `A1-6` SMP 4-core; Week3 `A2-3` FS batch + `A2-4` compositor + `A2-1` PML4 assert + `A2-5` trampoline/X2APIC + `A2-6` `sys_create_process` from Sirius; Ongoing `A3-1` clippy + `make check`.

---

## Agent Start Checklist

1. Read this `docs/initial_overview.md` (covers all subsystems)
2. For deep dive see `docs/{overview,memory,process,graphics,filesystem/hardware,syscalls,conventions,rust_coding_guidelines}.md`
3. Check build `make all`, run `make run` — logs `logs/<timestamp>/all.txt` + `core_N.txt` + `userspace_pid_N.txt`
4. Inspect `kernel/src/lib.rs:13` `MAX_CORES` vs `scripts/log_splitter.py:22`, QEMU `cores=` in `GNUmakefile`
5. Follow conventions: lock order `CORE_POOL→PROCESS_MANAGER→SCHEDULER→FRAME_ALLOCATOR→SERIAL`, never hold lock across `jump_to_userspace`, push `PROCESS_MANAGER` before `SCHEDULER` enqueue, `HHDM_OFFSET` via `BootInfo`, XRGB via `rgba_to_xrgb`
6. Code style: `docs/rust_coding_guidelines.md` — perf first, no `dyn`, slice APIs, `debug_assert`, `const` values, no emojis, no padded alignment, no `.` end comments
7. Verification per item: QEMU boots no `#PF`/`#GP`, 4-core interleaves, `fs_test` passes `fsck.fat -n` clean, `clippy` zero, objdump `mov %rcx,%cr3` not `rax`, frame free count stable 1000 cycles

---

*Last updated: 2026-08-21. Covers `syscalls.md:2` complete table, `filesystem.md:84` wire types, `process.md:333-359` gaps, `hardware.md:286` SMP notes.*
