# MofuOS — Process Subsystem

## Files

```
kernel/src/process/
  mod.rs              — public re-exports
  process.rs          — Process, ExecutionContext, ProcessState, context save/restore
  process_manager.rs  — ProcessManager, PROCESS_MANAGER lazy_static
  process_mem.rs      — ProcessMemoryLayout, MappedMemoryRegion
  elf_loader.rs       — ElfLoadInfo, LoadSegment, merge_segments, TEST_ELF
  scheduler.rs        — Scheduler, CoreScheduler, run_on_core_loop, SCHEDULER
  core_pool.rs        — CorePool, CORE_POOL
  kernel_thread.rs    — KernelThread, ThreadGroup, ThreadState
  execution.rs        — execute_process_direct, jump_to_userspace
  syscall.rs          — syscall_handler (naked), handle_syscall_inner, init_syscall
kernel/src/process_start.rs  — create_init_process, create_userspace_processes
```

## Process Model

Monolithic kernel. Multiple processes can share the same AP core via the
scheduler queue. Threads are not hard-pinned — at creation time the process
is assigned to the least-loaded AP core (by ready queue depth), but multiple
processes can be queued on the same core.

Process tree root: **arche** (PID 0). All other processes are children of arche or
of their creating process. If a parent exits without `kill_children`, orphaned children
are re-parented to arche.

Embedded ELF binaries (in `elf_loader.rs`):
- `TEST_ELF`: `include_bytes!("user/programs/test/test")` — multi-suite test program
- `PING_ELF`: `include_bytes!("user/programs/ping/ping")` — minimal write+yield loop

Currently `create_userspace_processes` launches two `ping` instances (PID 1, PID 2)
on core 1 for scheduler testing. The default active binary can be swapped by editing
`process_start.rs`.

## Core Pool vs Scheduler Queue

`CORE_POOL` tracks which core is *currently executing* a process (set by
`set_current_process_for_core`, cleared by `mark_core_idle`). It does NOT gate
process creation — multiple processes can be queued on the same core.

`create_process_from_elf` picks the target core by comparing
`scheduler.ready_count_on_core()` across all AP cores (least-loaded first).
It does NOT call `allocate_core`. This was a bug where the second process
creation would fail with `NoCoresAvailable` because core 1 was still marked
occupied from the first process.

## Core Types

### Process

```rust
pub struct Process {
    pub pid: PID,                      // usize
    pub parent_pid: PID,
    pub priority: u8,                  // 0-7 (MAX_PRIORITY = 7 in scheduler)
    pub state: ProcessState,           // Ready/Running/Waiting/Terminated
    pub name: String,
    pub children: Vec<PID>,
    pub file_descriptors: Vec<FileDescriptor>,
    pub resources: ProcessResources,
    pub exit_code: Option<i32>,
    pub is_out: bool,                  // true = independent top-level process
    pub execution_context: ExecutionContext,
    pub memory_layout: ProcessMemoryLayout,
}
```

`INVALID_PID = usize::MAX` — sentinel for "no process" / terminated.
`RFLAGS_DEFAULT = 0x202` — IF=1, reserved bit 1=1.
`DEFAULT_NEW_PROCESS_STACK_SIZE = 1 * 1024 * 1024` (1 MB).

### ExecutionContext

Full register set snapshot for context switching:
```rust
pub struct ExecutionContext {
    rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp,
    r8-r15, rip, rflags,
    page_table_base_phys: u64,  // CR3 value
}
```

### KernelThread

```rust
pub struct KernelThread {
    pub pid: PID,
    pub priority: u8,
    pub name: String,
    pub state: ThreadState,   // Ready/Running/Blocked/Terminated
    pub context: ExecutionContext,
    pub core_id: Option<u8>,
}
```

`ThreadGroup` holds a PID and a KernelThread (main thread). Matches 1:1 with a Process.

## Process Manager (PROCESS_MANAGER)

`lazy_static! PROCESS_MANAGER: Mutex<ProcessManager>`

Created with `init_arche()` which inserts arche (PID 0) at startup.

`new_pid` counter starts at 1 (arche is 0, incremented manually).

### create_process_from_elf

Main API for creating userspace processes from ELF:

1. Verify parent exists in `processes` Vec.
2. Assign `new_pid`, increment counter.
3. `CORE_POOL.lock().allocate_core(new_pid)` — get a free core.
4. `Process::create_with_elf(elf_info, name, pid, parent_pid)` — maps ELF, stack.
5. Create `KernelThread` with ELF entry point + stack from process execution context.
6. Assign thread to core: `kernel_thread.assign_to_core(core_id)`.
7. Create `ThreadGroup`.
8. **Push process and thread group FIRST** (`self.processes.push(process)` etc.).
9. **Then** `SCHEDULER.lock().enqueue_on_core(core_id, new_pid, priority)`.

The ordering in steps 8-9 is critical: the AP core scheduler loop calls
`PROCESS_MANAGER.get_process(pid)` immediately after dequeuing. If enqueued before
pushed, it finds `ProcessNotFound` and the core jumps to userspace with an invalid
context — memory corruption ensues.

### terminate_process

Sets state to Terminated, terminates main KernelThread, releases core via `CORE_POOL`.
If `cascade=true`, recursively terminates children.
If `cascade=false`, orphans children to arche (PID 0).

### get_process / get_process_mut

Linear scan of `processes: Vec<Process>`. O(n). Fine for current scale.

## ELF Loader

File: `process/elf_loader.rs`

`TEST_ELF: &[u8] = include_bytes!("../../../user/programs/test/test")`

The test binary is statically embedded in the kernel image at compile time. This is
the only ELF currently loaded. Its bytes live in kernel `.rodata`.

`ElfLoadInfo::from_elf_data(elf_data)`:
1. Parse with `elf::ElfBytes` (no-std, no alloc crate).
2. Validate architecture = EM_X86_64.
3. Iterate PT_LOAD segments: collect vaddr, filesz, memsz, flags, raw data.
4. Call `merge_segments` — sorts by vaddr, merges overlapping segments by combining
   data and OR-ing flags. This handles multi-segment ELFs that share page boundaries.
5. Returns `ElfLoadInfo { entry_point, min_vaddr, max_vaddr, segments }`.

`LoadSegment.flags` uses ELF PF_ bits: Executable=1, Writable=2, Readable=4.

## Process Creation from ELF (Process::create_with_elf)

File: `process/process.rs`

1. Get `UserMemoryManager` and `FrameAllocator` from globals.
2. `ProcessMemoryLayout::new(...)` — allocates a new PML4 address space.
3. For each `LoadSegment`:
   - Compute `PageTableFlags` from ELF flags (PRESENT | USER_ACCESSIBLE | optionally WRITABLE | NO_EXECUTE).
   - `map_virt_mem_region(pml4_phys, vaddr, in_memory_size, flags, frame_alloc)`.
   - Copy file data via `translate_user_virt_to_phys` → HHDM pointer → `copy_nonoverlapping`.
   - Zero BSS (in_memory_size > in_file_size) via same translation path.
   - Track region in `memory_layout.mapped_regions`.
4. `create_main_stack(pml4_phys, stack_size, frame_alloc)` → stack top VirtAddr.
5. Build `ExecutionContext::new(entry_point, stack_top, pml4_phys)`.

## Scheduler

File: `process/scheduler.rs`

`lazy_static! SCHEDULER: Mutex<Scheduler>`

### Scheduler Structure

```rust
pub struct Scheduler {
    per_core: Vec<CoreScheduler>,  // index = core_id
    core_count: u8,
}

pub struct CoreScheduler {
    core_id: u8,
    ready_queues: [Dequeue<PID>; 8],  // 8 priority levels (0=lowest, 7=highest)
    blocked_queue: Dequeue<PID>,
    current_thread: PID,
}
```

`SCHEDULER_STACKS: [GuardedKernelStack; MAX_CORES]` `36 KiB` (`guard+stack`, `align(4096)`, `guard !PRESENT` after `boot_common::bsp_early_init`, `top-0x1000` headroom for `mov rsp` frame). `gdt::scheduler_bounds` / `assert_rsp_in_bounds` (timer `RPL==Ring3` only).

Priority: 8 levels (0-7). Higher number = higher priority. Within a level: FIFO
(Dequeue pop_front).

### run_on_core_loop

Called on each AP core after boot initialization:

```
loop:
  disable interrupts
  get_next_on_core(core_id) -> (pid, priority)
  if pid == INVALID_PID:
    enable interrupts, hlt, continue

  set_current_on_core(core_id, pid)
  extract exec context from PROCESS_MANAGER (lock then drop)
  set_current_process_for_core(core_id, pid)
  Cr3::write(user_pml4)
  enable interrupts

  // push return label address, save RSP, jmp to jump_to_userspace
  asm:
    lea rax, [rip + 2f]      // return label address
    push rax                  // return address on stack
    mov [KERNEL_RSP_ON_CORE[core_id]], rsp   // save RSP
    jmp jump_to_userspace    // one-way trip to userspace
  "2:"                       // return_to_scheduler() ret lands here

  // post-exit cleanup
  mark_core_idle(core_id)
  if process is NOT terminated: re-enqueue(pid, priority)
  else: log "PID N terminated"
```

The `jmp` (not `call`) into `jump_to_userspace` means no extra stack frame is
created. The return address is the manually pushed label `"2:"`. This is the
same pattern used by the Linux kernel for `iret`-based userspace entry.

### KERNEL_RSP_ON_CORE and return_to_scheduler

```rust
static KERNEL_RSP_ON_CORE: [AtomicU64; MAX_CORES]
```

Set by the scheduler loop just before entering userspace. Read by `return_to_scheduler()`.

```rust
pub fn return_to_scheduler() -> ! {
    let core_id = get_current_core_id();
    let kernel_rsp = KERNEL_RSP_ON_CORE[core_id].load(SeqCst);
    asm!("mov rsp, {rsp}; ret", rsp = in(reg) kernel_rsp, options(noreturn));
}
```

Called from `sys_exit`. Restores kernel RSP and `ret` — pops the saved label
address and resumes the scheduler loop at the instruction after the asm block.

This mechanism is also the foundation for future preemption: the timer interrupt
handler can call `return_to_scheduler()` to preempt a running process mid-slice.

### CURRENT_PROCESS_ON_CORE

```rust
static CURRENT_PROCESS_ON_CORE: [AtomicU64; MAX_CORES]
```

Stores current PID for each core. Used by syscall handler to identify calling process.
`INVALID_PID as u64` = idle.

## Core Pool

File: `process/core_pool.rs`

`lazy_static! CORE_POOL: Mutex<CorePool>`

Tracks which CPU cores are available for process assignment.

```rust
pub struct CorePool {
    total_cores: u8,
    available_cores: u64,  // bitmask: bit N=1 means core N is free
    core_assignments: [usize; MAX_CORES],  // core_id -> PID
    lapic_ids: [u8; MAX_CORES],
}
```

- Core 0 is reserved for kernel/BSP: `available_cores &= !1` during init.
- `allocate_core(pid)` → linear scan for first available bit, marks it used.
- `release_core(core_id)` → sets bit, clears assignment.
- Hard limit: MAX_CORES=16, bitmask fits in u64.

## Jump to Userspace

File: `process/execution.rs`

```rust
pub unsafe fn jump_to_userspace(entry_point: u64, stack_pointer: u64, rflags: u64) -> !
```

1. Disable interrupts.
2. Load user data selectors into DS/ES/FS/GS.
3. Build iretq frame on stack: SS, RSP, RFLAGS (from param), CS, RIP.
4. `iretq` — switches to ring 3, jumps to entry_point, stack at stack_pointer.

The `rflags` parameter is `RFLAGS_DEFAULT (0x202)` on first entry
(`ExecutionContext::new`) and the saved user RFLAGS on resumed entry
(set by sys_yield or timer preemption).

## Syscall Interface

File: `process/syscall.rs`

Mechanism: SYSCALL/SYSRET (64-bit). Configured in `init_syscall()` per AP core:
- EFER.SCE = 1 (enable SYSCALL)
- STAR MSR: kernel CS=0x08, user CS base=0x10 (so sysretq → CS=0x23, SS=0x1B)
- LSTAR MSR: address of `syscall_handler` (naked function)
- SFMASK: `1 << 9` (masks IF on SYSCALL entry — see `docs/syscalls.md:1.4` and BUG-09)
- KERNEL_GS_BASE: `&PER_CORE_SYSCALL[core_id]` (per-core 64 KiB stack, `swapgs`+`gs:0`)

> **Canonical reference:** `docs/syscalls.md` — full Ring 3 -> Ring 0 switch, `SyscallFrame` layout, and per-syscall spec. The table below is a summary; `docs/syscalls.md:2` is authoritative.

### syscall_handler (naked asm) — see `docs/syscalls.md:1.3` for full asm

1. `swapgs; mov r15,rsp; mov rsp,gs:0; swapgs` — save user RSP, switch to per-core kernel stack via `KERNEL_GS_BASE`.
2. Push: user RSP (R15), user RIP (RCX), user RFLAGS (R11), `rax` (syscall_num — BUG-08 fix), args `rdi,rsi,rdx,r10,r8,r9`, callee-saved `rbx,rbp,r12,r13,r14`.
3. `mov rdi, rsp` → pointer to `SyscallFrame`.
4. `call handle_syscall_inner` (does `sti` at top, `cli` before `return_to_scheduler`).
5. Pop callee-saved, `add rsp,8` skip syscall_num slot (rax holds return), restore RFLAGS/RIP/RSP.
6. `mov rsp,r15; sysretq`.

### SyscallFrame (repr C)

```
r14, r13, r12, rbx, rbp        (callee-saved)
arg6 (r9), arg5 (r8), arg4 (r10), arg3 (rdx), arg2 (rsi), arg1 (rdi)
syscall_num (rax)
rflags (r11), user_rip (rcx), user_rsp (r15)
```

### Implemented Syscalls — summary (full spec: `docs/syscalls.md:2`)

| Number | Name                  | Args                          | Behavior                                              |
|--------|-----------------------|-------------------------------|-------------------------------------------------------|
| 0      | sys_create_process    | path_ptr, path_len, name_ptr, name_len | **partial** — parses `ODYS_ELF` and `create_process_from_elf`; FS path not yet wired (`ISSUE-P8`) |
| 2      | sys_write             | fd, buf, count                | fd=1/2: write to COM2; returns count                  |
| 5      | sys_allocate          | size                          | grows user heap at `0x6000_0000`, returns old heap_end |
| 10     | sys_create_window     | w, h, x, y                    | creates compositor window, returns window_id          |
| 11     | sys_destroy_window    | window_id                     | marks window invisible, recycles id                   |
| 12     | sys_map_window_buffer | window_id                     | maps **back+front** buffers to user VA `0x1_0000_0000+id*8MB`, returns base |
| 13     | sys_present_window    | window_id                     | triggers back->front swap in WindowBuffer             |
| 14     | sys_get_window_size   | window_id                     | returns (width << 32) | height                        |
| 15     | sys_focus_window      | window_id                     | brings window to front                                |
| 16     | sys_get_window_info   | window_id, info_ptr           | fills `WindowInfo {width,height,event_buffer_vaddr}`; `0`/`MAX` |
| 20     | sys_open_file         | path_ptr, path_len, flags     | resolves path via Sirius, adds fd to process, returns fd index |
| 21     | sys_close_file        | fd                            | swap-removes fd from process table                    |
| 22     | sys_read_file         | fd, offset, buf_ptr, count    | reads `count` at `offset` via Sirius; returns bytes read |
| 23     | sys_write_file        | fd, offset, buf_ptr, count    | writes `count` at `offset` via Sirius; returns bytes written (writeback if cached) |
| 24     | sys_stat_file         | path_ptr, path_len, stat_ptr  | fills StatFlat at stat_ptr; returns 0 or u64::MAX     |
| 25     | sys_list_dir          | path_ptr, path_len, out_ptr, out_len | fills DirEntryFlat array; returns entry count  |
| 26     | sys_create_file       | path_ptr, path_len            | creates file on disk; returns 0 or u64::MAX           |
| 27     | sys_create_dir        | path_ptr, path_len            | creates directory on disk; returns 0 or u64::MAX      |
| 28     | sys_delete            | path_ptr, path_len            | deletes file or empty directory; returns 0 or u64::MAX|
| 30     | sys_pin_file          | path_ptr, path_len            | pin in cache `Resident` (cfg `use_cached_fs`); `0`/`MAX` |
| 31     | sys_unpin_file        | path_ptr, path_len            | unpin to `Normal`; `0`/`MAX`                          |
| 32     | sys_reserve_cache     | path_ptr, path_len, importance(0..6) | set `CacheImportance` hint on dir; `0`/`MAX`   |
| 33     | sys_evict_directory   | path_ptr, path_len            | drop cached children of dir; returns `freed_bytes`    |
| 34     | sys_get_cache_stats   | stats_ptr                     | fills `CacheStatsFlat`; `0`/`MAX`                     |
| 35     | sys_flush_file_cache  | —                             | flush open-fd nodes of calling pid; `0`/`MAX`        |
| 600    | sys_create_circular_buffer | size, page_flags, info_ptr | double-maps `size` bytes at `2*size` contiguous VA; returns `CircularBufferInfo` |
| 970    | sys_get_cpu_info      | buf_ptr, buf_len              | fills `CpuInfoFlat`; `0` ok, `2` invalid ptr          |
| 997    | sys_get_pid           | -                             | returns calling process PID (alias `sys_echo`)        |
| 998    | sys_yield             | -                             | save context, re-enqueue, return to scheduler         |
| 999    | sys_exit              | exit_code                     | terminate process, return to scheduler                |

Unimplemented (return u64::MAX): 1 (terminate_process), 3 (read), 4 (get_line), 8 (load_file), 9 (unload_file), 996 (get_process_info).

### FileDescriptor

Each process has `file_descriptors: Vec<FileDescriptor>`. A fd index is its position
in that Vec. `sys_close_file` swap-removes (O(1); fd indices are not stable after close).

```rust
pub struct FileDescriptor {
    pub node_id: usize,   // FileNodeHandle — packed FAT32 cluster + attrs
    pub flags: u8,        // FD_FLAG_READ=0x01, FD_FLAG_WRITE=0x02
    // offset removed — ReadFile/WriteFile take explicit offset arg (rsi)
}
```

### Filesystem Pointer Validation

`validate_user_ptr(ptr, len)` — all syscalls taking userspace pointers call this first.
Rejects null, kernel-space addresses (>= 0x0000_8000_0000_0000), and wrap-around.

### Wire Types (kernel + rustspace must match)

```rust
// DirEntryFlat — written by sys_list_dir into user-provided buffer
// StatFlat     — written by sys_stat_file into user-provided buffer
#[repr(C)]
pub struct DirEntryFlat {
    pub name: [u8; FS_NAME_LEN],   // FS_NAME_LEN matches kernel DIR_ENTRY_NAME_LEN
    pub name_len: u8,
    pub is_dir: u8,
    pub _pad: [u8; 6],
    pub size: u64,
    pub created_time: u32,
    pub modified_time: u32,
}
```

CRITICAL: `FS_NAME_LEN` in rustspace and `DIR_ENTRY_NAME_LEN` in the kernel
must be identical. If they diverge the struct layout breaks silently.

### SyscallFrame push order

The naked asm pushes registers in this exact order (lowest address first = first struct field):
```
push r14, r13, r12, rbp, rbx       <- callee-saved
push r9(arg6), r8(arg5), r10(arg4), rdx(arg3), rsi(arg2), rdi(arg1)
push rax                            <- syscall_num  *** must not be omitted ***
push r11                            <- rflags (saved by CPU in r11 on SYSCALL)
push rcx                            <- user_rip (saved by CPU in rcx on SYSCALL)
push r15                            <- user_rsp (saved before stack switch)
```
On the pop side, the syscall_num slot is skipped with `add rsp, 8` because RAX already
holds the return value from `handle_syscall_inner` and must reach userspace unchanged.

### SFMASK and interrupt safety

SFMASK = `1 << 9` (masks IF on SYSCALL entry). This prevents the timer from firing
while the handler is on the kernel stack but SS is still in kernel state (Intel SYSRETQ
does not restore SS.RPL=3 — see BUG-09 in bugs.md).

Interrupts are re-enabled at the start of `handle_syscall_inner` (sti) so long-running
syscalls can still be preempted. Before calling `return_to_scheduler()` in sys_yield
and sys_exit, interrupts are disabled again (cli) to prevent the timer firing during
the stack unwind.

### sys_yield (998)

Saves `user_rip`, `user_rsp`, `rflags` from the syscall frame into
`process.execution_context`, then calls `return_to_scheduler()`. The scheduler
loop sees the process is not Terminated, re-enqueues it, and runs the next
process. When it comes back, `jump_to_userspace` resumes at the instruction
after the `syscall` instruction (sysretq return address in `frame.user_rip`).

Note: syscall 998 is only effective as a yield when the process is in a tight
loop. If the process spends most of its time in the kernel (e.g. tight
sys_write loop), preemption via the timer may not fire often enough to
interleave with another process. See ISSUE-P9.

### Syscall Stack + Guard

Per-core. Each core gets `64 KiB` stack plus `4 KiB` guard inside `GuardedSyscallSlot` (total `72 KiB`).

```rust
const SYSCALL_STACK_SIZE: usize = 4096 * 16; // 64 KiB
const GUARD_PAGE_SIZE: usize = 4096;

#[repr(C, align(64))]
struct PerCoreSyscallData {
    stack_top: u64,              // must be first field — naked asm reads gs:0
    _stack: [u8; SYSCALL_STACK_SIZE],
}

#[repr(C, align(4096))]
struct GuardedSyscallSlot { guard: [u8; 4096], data: PerCoreSyscallData } // guard at slot base

static mut SYSCALL_SLOTS: [GuardedSyscallSlot; MAX_CORES] // 4 * 72 KiB, guard !PRESENT
// KERNEL_GS_BASE = &SYSCALL_SLOTS[core].data (not guard)
```

`stack_top` first field (`repr(C)`) so `gs:0` reads directly. `guard` at `slot` base `page-aligned`, `data` at `+0x1000`, `_stack` at `data+8` (`bottom &0xFFF==8`, `page(bottom)==guard+0x1000`), `top = _stack+64KiB`. Guard unmapped after `init_heap` via `stack_guard::unmap_guard_page` (splits `2MiB` huge from Limine if needed, `FrameAllocator` `PT` alloc, `flush`). Overflow at `guard` -> `#PF` `present=0` instead of silent `TSS` corrupt.

`init_syscall()` per AP:
1. `stack_top = &slot.data._stack[64KiB]` (one past end, grows down)
2. `slot.data.stack_top = top`
3. `WRMSR KERNEL_GS_BASE = &slot.data` (`gs:0==top`)

Naked asm prologue:
```asm
swapgs; mov r15,rsp; mov rsp,gs:0; swapgs // save user RSP, switch to per-core top
```

`assert_syscall_stack_bounds(core_id)` (`mov rsp` `debug_assert!(bottom<=rsp<=top)`) at top of `handle_syscall_inner`.

`install_syscall_guard_pages(&mut FrameAllocator)` called after `init_heap` in `boot_common::bsp_early_init`.

## Userspace Test Program

File: `user/programs/test/test.c`
Built with: clang via `user/Makefile`, linked with `user/linker.ld`
CRT: `user/crt0.c` (handles `_start` → `main`)
Libc: `user/libc/syscall.h` + `user/libc/syscall.o`

Test suites:
1. `suite_userspace_logic` — pure C logic, no syscalls (verifies stack, BSS, arithmetic)
2. `suite_write` — sys_write return value tests
3. `suite_echo_math` — syscall 997 argument passing
4. `suite_unimplemented` — unimplemented syscalls return -1
5. `suite_write_stress` — 20 sequential writes

Output goes to COM2 via sys_write(fd=1). The log splitter routes it to
`userspace_pid_N.txt`.

## Known Issues / TODOs

- Stubs still return u64::MAX: 1, 3, 4, 8, 9, 996. `CreateProcess (0)` is partial (hardcodes `ODYS_ELF`).
- No frame deallocation when processes terminate.
- Preemption of tight syscall loops: sti inside handle_syscall_inner helps, but a process
  running a tight sys_write loop still hogs the core during each handler invocation.
- Process exit code from sys_exit does not propagate to parent via wait() (no wait).
- `current_on_core` in Scheduler returns INVALID_PID sentinel rather than Option<PID>.
