# MofuOS — Process Subsystem

## Files

```
kernel/src/process/
  mod.rs              — public re-exports, PROCESS_DEBUG_PRINT
  process.rs          — Process, ProcessState
  process_manager.rs  — ProcessManager, PROCESS_MANAGER lazy_static
  process_mem.rs      — ProcessMemoryLayout, MappedMemoryRegion
  elf_loader.rs       — ElfLoadInfo, LoadSegment, merge_segments, embedded ELFs
  task.rs             — TaskSlot, TASKS, FpuState, request_kill, exit_current, task_first_entry
  scheduler.rs        — per-core run queues, switch_context, run_on_core_loop, schedule, work stealing
  wait.rs             — WaitQueue, block_current_task_until_event, sleep_current_task_ms
  core_pool.rs        — CorePool (core count and LAPIC ids), CORE_POOL
  syscall.rs          — syscall_handler (naked), handle_syscall_inner, dispatch_syscall, init_syscall
kernel/src/trap.rs            — TrapFrame, interrupt_stub!, interrupt_return_path
kernel/src/util/irq_mutex.rs  — IrqMutex
kernel/src/process_start.rs  — create_init_process, create_userspace_processes
```

## Process Model

Monolithic kernel. Every process has exactly one task, a slot in the static `TASKS`
table with its own kernel stack. Multiple tasks share an AP core through the
per-core run queue. A task is assigned to the least-loaded AP core at creation
(`scheduler::core_runnable_tasks_count`) and an idle core steals waiting tasks from a busy one.
Core 0 runs the boot code and the tests and never schedules a task.

Process tree root: **arche** (PID 0). All other processes are children of arche or
of their creating process. If a parent exits without `kill_children`, orphaned children
are re-parented to arche.

Embedded ELF binaries (in `elf_loader.rs`):
- `TEST_ELF`: `include_bytes!("user/programs/test/test")` — multi-suite test program
- `PING_ELF`: `include_bytes!("user/programs/ping/ping")` — minimal write+yield loop

Currently `create_userspace_processes` launches two `ping` instances (PID 1, PID 2)
on core 1 for scheduler testing. The default active binary can be swapped by editing
`process_start.rs`.

## Tasks

File: `process/task.rs`

`static TASKS: [TaskSlot; MAX_TASKS]` (`MAX_TASKS = 64`). Process creation fails with
`ProcessError::OutOfTaskSlots` when no slot is free. A slot is named by a `TaskRef { slot, pid }`,
valid while `TASKS[slot].pid == pid`. PIDs are never reused, so a stale reference is detected.

```rust
pub struct TaskSlot {
    pid, state: AtomicU8 (TaskState), priority, home_core, on_cpu, kill_pending,
    exit_code_request, cpu_ticks, core_migration_count, cr3, kernel_stack_top,
    next: AtomicU16,                 // intrusive run queue link
    saved_rsp: UnsafeCell<u64>,      // written by switch_context
    fpu_state: UnsafeCell<FpuState>, // 512 byte FXSAVE image, 16 byte aligned
}
```

`TaskState`: `Free`, `Creating`, `Ready`, `Running`, `Blocked`, `Dead`. Changes between them go through
`TaskSlot::transition(from, to)` (a compare and swap), so a double wake or a wake racing a block is harmless.

Each slot owns a static kernel stack, `gdt::KERNEL_TASK_STACKS[slot]`: 64 KiB plus an unmapped 4 KiB guard
page. Syscalls and ring 3 interrupts of the task run on it. The per-core syscall stack no longer exists.

`FpuState` is saved in `core_run_scheduling(core_id)` and restored by the scheduler loop right before it switches to the task.
New tasks start with FCW=0x37F and MXCSR=0x1F80. There is no ymm state because CR4.OSXSAVE is off.

### Lifecycle

- Created by `create_process_from_elf`: slot allocated, a first-entry stack built by `init_task_stack`
  (callee-saved frame whose return address is `task_first_entry`, above it a `TrapFrame` with the user
  rip, rsp, `RFLAGS_DEFAULT`, user CS and SS), state `Ready`, queued on the chosen core.
- `task_first_entry` (naked) exits right away if a kill is pending, else loads the user data selectors,
  pops the all-zero `TrapFrame` registers and `iretq`s to ring 3.
- A task tears itself down. `exit_current(code)` loads the kernel CR3, destroys the windows, runs
  `terminate_process` (frees the address space, sets `Terminated`), marks the slot `Dead` and calls
  `core_run_scheduling(core_id)`, which never returns. The scheduler loop releases the slot after it switched away from the
  stack. Nothing is ever unwound or discarded.
- Killing another process is a request: `task::request_kill(pid, code)` sets `kill_pending` and
  `exit_code_request`, wakes the task if it is blocked and sends a reschedule IPI to its core. The task
  calls `exit_current` at its next `exit_to_user` or at its first entry. A task in a long syscall exits
  when the syscall returns.

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
    pub memory_layout: ProcessMemoryLayout,
}
```

`INVALID_PID = usize::MAX` — sentinel for "no process" / terminated.
`RFLAGS_DEFAULT = 0x202` — IF=1, reserved bit 1=1.
`DEFAULT_NEW_PROCESS_STACK_SIZE = 1 * 1024 * 1024` (1 MB).

The registers of a process are not stored in `Process`. A task's user registers live in a frame on its kernel
stack and its kernel registers are saved by `switch_context` on the same stack.

### Userspace heap and virtual-range allocators

File: `process/process_mem.rs`, `process/process.rs`

- Heap: `PROCESS_HEAP_VIRT_START 0x6000_0000` up to `PROCESS_WINDOW_BUFFER_BASE`, `heap_end == START` at creation (empty). `allocate_heap(size)` (used by `sys_allocate(5)`) aligns to 4 KiB, `grow_heap(new)` calls `map_owned_region` with `PRESENT|WRITABLE|NO_EXECUTE`, which tracks the region (merging with the previous heap region, so the heap stays one entry) and then bumps `heap_end`. Both take `&UserMemoryManager`.

- Virtual range: `PROCESS_USER_VADDR_ALLOC_START 0x1000_0000` .. `0x5000_0000` for `CircularBuffer` double-map and future `mmap`. `allocate_virtual_range` only reserves (bumps `next_alloc_vaddr`, or `find_free_gap` over the sorted `regions` once the window is exhausted), the caller maps the range right away with `map_frames`.

All of it lives in `ProcessMemoryLayout::regions` with explicit `FrameOwnership` and is reclaimed by `free_address_space`. See `docs/memory.md` for the layout API.

## Process Manager (PROCESS_MANAGER)

`lazy_static! PROCESS_MANAGER: Mutex<ProcessManager>`

Created with `init_arche()` which inserts arche (PID 0) at startup.

`new_pid` counter starts at 1 (arche is 0, incremented manually).

### create_process_from_elf

Main API for creating userspace processes from ELF:

1. Verify parent exists in `processes` Vec.
2. Assign `new_pid`, increment counter.
3. Pick the AP core with the lowest `scheduler::core_runnable_tasks_count` (no lock).
4. `Process::create_with_elf(elf_info, name, pid, parent_pid)` — maps ELF, stack.
5. `task::allocate_task_slot(...)`, on failure free the address space and return `OutOfTaskSlots`.
6. `task::init_task_stack(slot, entry_point, USER_STACK_TOP)`.
7. **Push the process FIRST** (`self.processes.push(process)`).
8. Set the slot `Ready` and `scheduler::core_enqueue_ready_task(core_id, slot, priority)`, which also sends an
   IPI when the core is idle or runs a lower priority task.

The ordering in steps 7-8 matters: an AP core can dequeue the task the instant it is queued and the first
syscall looks the process up in `PROCESS_MANAGER`.

### terminate_process (with reclamation)

`PROCESS_MANAGER::terminate_process(pid, exit_code, cascade)` is called by `exit_current` on the dying task's own
kernel stack, after the task switched to the kernel CR3. It reclaims all user resources:

1. Under `get_process_mut`, copy `children`, set `Terminated`/`exit_code`.
2. If `top_page_table_phys != 0`, call `memory_layout.free_address_space(get_user_mem_mgr(), &mut frame_allocator)` directly on the process layout (no clone), then zero `top_page_table_phys` (guard against double free) and clear `regions`:
   - `UserMemoryManager::unmap_user_region` for every region in `regions`: `Mapper::unmap` per page, absent pages skipped, frames freed only for `FrameOwnership::Owned`.
   - `free_user_tables` frees all PT/PD/PDPT frames, with a `debug_assert` that they are empty.
   - `free_pml4_frame` frees the top table and `flush_all`.
   The windows of the process are destroyed before this, by `exit_current`, because window cleanup locks the compositor and then `PROCESS_MANAGER`.
3. Orphan children to arche `PID 0`, or with `cascade` send each child a kill request.

The run queues are not touched: the task is running, so it is in no queue, and the scheduler loop frees its slot.

`cleanup_dead()` (`retain Terminated`) is called by the tests to verify a PID is fully removed.
`free_frame_count` + `allocated_bump_frames` in `MemoryMapFrameAllocator` expose reclamation for tests.

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
   - `memory_layout.map_owned_region(vaddr, in_memory_size, flags, ..)`, which maps, rolls back on failure and tracks the region.
   - Copy file data via `translate_user_virt_to_phys` → HHDM pointer → `copy_nonoverlapping`.
   - Zero BSS (in_memory_size > in_file_size) via same translation path.
4. Map the shared data page (`SharedState::map_into_process`, `Borrowed`) and the main stack (`map_owned_region` ending at `USER_STACK_TOP`).

## Scheduler

File: `process/scheduler.rs`

Preemptive, per-core, with work stealing. Per-switch logging is behind `SCHED_DEBUG_PRINT` (off) and never
happens under a run queue lock.

### Run queues

`static CORE_RUN_QUEUES: [CacheAligned<IrqMutex<CoreRunQueue>>; MAX_CORES]`. Never hold two of these locks at
once. `IrqMutex` (`util/irq_mutex.rs`) saves IF, disables interrupts, then locks; the guard unlocks first and
restores IF afterwards. Everything an interrupt handler touches must use it.

`CoreRunQueue` has two banks, `active` and `expired`. A bank has one queue per priority (0-7, 7 highest), made of
`TaskSlot::next` links, plus a `u8` bitmap of the non-empty levels. `pick_next_task` takes the oldest task of the
highest set level of `active`, and swaps the banks when `active` is empty.

- New and woken tasks go to `active`.
- A task that used its slice or yielded goes to `expired`. One that was only displaced by a higher priority
  task keeps its place in `active`.
- Slice length is `(priority + 1) * BASE_SLICE_TICKS` timer ticks (tick = 10 ms).
- So every runnable task runs once per epoch, a priority 7 task that yields gives way to priority 1 and
  nothing starves.

The queue also holds the per-core sleeping_tasks, see `docs/syscalls.md` (SleepMs).

Lock-free per-core state: `RUNNABLE_TASKS_COUNT` (tasks queued or running, changes only when a task becomes runnable,
blocks or dies; it drives placement and "does anyone wait"), `CORE_SHOULD_BE_RESCHEDULED`, `SLICE_REMAINING_TICKS`,
`CURRENT_PROCESS_ON_CORE` and `CURRENT_SLOT_ON_CORE`. `get_current_process_for_core(core)` is the only record
of the current process.

### Switching

`switch_context(save_rsp, load_rsp)` (naked) pushes rbx rbp r12-r15, stores rsp, loads the other rsp, pops, `ret`.
`run_on_core_loop(core_id)` moves onto the per-core scheduler stack (`SCHEDULER_STACKS`, 36 KiB with guard) through
the naked `enter_scheduler_stack` and runs `scheduler_loop`, with interrupts off:

```
loop:
  pick_next_task from the own queue
  if none: try to steal; else load the kernel CR3, enable_and_hlt
  run_task:
    state = Running, on_cpu = true, current task of the core = slot
    clear CORE_SHOULD_BE_RESCHEDULED, reset the slice
    syscall stack top and TSS rsp0 = kernel stack top of the task
    restore FPU, write CR3 (always), switch_context to the task
  // back here when the task called core_run_scheduling(core_id)
    on_cpu = false; if the task is Dead, release the slot
```

`core_run_scheduling(core_id)` runs on a task stack with no locks held: saves IF, disables interrupts, saves the FPU, does
`CAS(Running, Ready)` and queues the task on success (a `Blocked` or `Dead` task is not queued), then switches to
the scheduler loop. When the task runs again it restores IF. `core_yield_executing_tasks(core_id)` is `core_run_scheduling` that requeues to
`expired` and returns at once when nothing else is runnable on the core.

A woken task may sit in a run queue before it has switched out. That is safe: only its home core dequeues it and
only after the switch. Stealing checks `on_cpu`.

### Preemption and the exit to ring 3

Timer, keyboard, mouse and reschedule interrupts use naked stubs (`trap::interrupt_stub!`) that push a
`TrapFrame`, call the Rust handler, then `interrupt_return_path`. Interrupted ring 0 code just returns, the kernel
is not preemptible. For ring 3 the stub restores the user SS selector (after `sysretq` the CPU pushes SS=0x18,
RPL 0, which makes `iretq` fault) and calls `scheduler::exit_to_user()`.

`exit_to_user()` (interrupts off, also run at the end of every syscall) loops: exit if `kill_pending`; if
`CORE_SHOULD_BE_RESCHEDULED` call `core_run_scheduling(core_id)`; otherwise return.

`on_timer_tick(core)` (interrupt context, lock-free except for the sleeper check) counts `cpu_ticks`, wakes due
sleeping_tasks and, when the slice is used up and another task waits, sets `CORE_SHOULD_BE_RESCHEDULED`.

Reschedule IPI (`InterruptIndex::Reschedule = 0xF0`, `send_reschedule_ipi`) is sent when a task is queued on a
remote core that is idle or runs a lower priority, and when a kill is requested for a running task. The LAPIC
address is an atomic pointer and the core-to-LAPIC-id table is a static, so IPIs and EOI take no lock.

### Blocking

`TaskState` is a state machine, `wake_task(TaskRef)` does `CAS(Blocked, Ready)` and on success queues the
task on its home core (blocked tasks do not migrate), so it is idempotent. `process/wait.rs`:

- `WaitQueue`: fixed capacity list of `TaskRef` under an `IrqMutex`.
- `block_current_task_until_event(queue, condition)`: check; join the queue and become `Blocked` in one step; check again (wake
  yourself if it now holds); `core_run_scheduling(core_id)`; loop. A wake between the first check and the block is not lost.
- `sleep_current_task_ms(ms)` / `scheduler::core_block_current_task_until_wake_time(t)`: the task is added to the sleeping_tasks of its core and
  blocked; the timer tick of that core wakes it. A kill wakes it early and the sleep loop ends.
- Static queues: `INPUT_WAIT_QUEUE` (woken by the compositor after it forwarded input to a window buffer) and
  `FUTEX_WAIT_QUEUE`.

### Work stealing

An idle core scans the other AP cores, skipping core 0, for a core_from with more than one runnable task and
`try_lock`s its queue. It takes a task whose `on_cpu` is false (highest priority first), drops the core_from lock,
sets `home_core`, bumps the task's `core_migration_count` counter and queues it locally. CR3 is written on every dispatch
and the kernel CR3 is loaded before `hlt`, so no idle core keeps the page tables of a task that exits elsewhere.
`WORK_STEALING_ENABLED` switches it off.

### CURRENT_PROCESS_ON_CORE

Stores the current PID for each core, `INVALID_PID` = idle. Used by the syscall handler to identify the caller.

## Core Pool

File: `process/core_pool.rs`

`lazy_static! CORE_POOL: Mutex<CorePool>` holds the total core count and the LAPIC id of each core
(`interrupts::set_lapic_id_of_core` mirrors the ids into a lock-free table). It does not allocate cores,
placement is `scheduler::core_runnable_tasks_count`. `TOTAL_CORE_COUNT` is the same count as a static.

## Entering Ring 3

A task enters ring 3 the first time through `task_first_entry` and afterwards through the normal return of the
path that took it into the kernel: `sysretq` at the end of `syscall_handler`, or `iretq` at the end of an
interrupt stub. Both restore every user register from the frame on the task's kernel stack (a yield returns 0 in
rax like any syscall). `RFLAGS_DEFAULT (0x202)` is the first-entry flags value.

## Syscall Interface

File: `process/syscall.rs`

Mechanism: SYSCALL/SYSRET (64-bit). Configured in `init_syscall()` per AP core:
- EFER.SCE = 1 (enable SYSCALL)
- STAR MSR: kernel CS=0x08, user CS base=0x10 (so sysretq → CS=0x23, SS=0x1B)
- LSTAR MSR: address of `syscall_handler` (naked function)
- SFMASK: `1 << 9` (masks IF on SYSCALL entry — see `docs/syscalls.md:1.4` and BUG-09)
- KERNEL_GS_BASE: `&SYSCALL_DATA[core_id]` (`stack_top` = kernel stack top of the task running on the core, set on every dispatch, `swapgs`+`gs:0`)

> **Canonical reference:** `docs/syscalls.md` — full Ring 3 -> Ring 0 switch, `SyscallFrame` layout, and per-syscall spec. The table below is a summary; `docs/syscalls.md:2` is authoritative.

### syscall_handler (naked asm) — see `docs/syscalls.md:1.3` for full asm

1. `swapgs`, stash user rsp in `gs:[user_rsp_scratch]`, `mov rsp,gs:0`, push the user rsp, `swapgs` — switch to the kernel stack of the running task.
2. Push: user RSP, user RIP (RCX), user RFLAGS (R11), `rax` (syscall_num — BUG-08 fix), args `rdi,rsi,rdx,r10,r8,r9`, callee-saved `rbx,rbp,r12,r13,r14,r15`. 16 pushes on a 16 byte aligned stack top leave rsp aligned for the call.
3. `mov rdi, rsp` → pointer to `SyscallFrame`.
4. `call handle_syscall_inner`: `sti`, `dispatch_syscall`, `cli`, `exit_to_user()` (pending kill or reschedule), return value in rax.
5. `cli`, pop callee-saved and args, `add rsp,8` skip syscall_num slot (rax holds return), restore RFLAGS/RIP, `pop rsp`.
6. `sysretq`.

### SyscallFrame (repr C)

```
r15, r14, r13, r12, rbx, rbp   (callee-saved)
arg6 (r9), arg5 (r8), arg4 (r10), arg3 (rdx), arg2 (rsi), arg1 (rdi)
syscall_num (rax)
rflags (r11), user_rip (rcx), user_rsp (r15)
```

### Implemented Syscalls — summary (full spec: `docs/syscalls.md:2`)

| Number | Name                  | Args                          | Behavior                                              |
|--------|-----------------------|-------------------------------|-------------------------------------------------------|
| 0      | sys_create_process    | path_ptr, path_len, name_ptr, name_len | **partial** — parses `ODYS_ELF` and `create_process_from_elf`; FS path not yet wired (`ISSUE-P8`) |
| 1      | sys_terminate_process | pid, exit_code                | self: `exit_current`; other: kill request, returns `0` at once, the target exits later |
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
| 990    | sys_sleep_ms          | duration_ms                   | blocks the caller for at least that long, no cpu use  |
| 991    | sys_wait_for_input    | -                             | blocks until a window of the caller has input waiting |
| 992    | sys_futex_wait        | word_ptr, expected            | blocks while `*word == expected` and no FutexWake came |
| 993    | sys_futex_wake        | -                             | wakes every FutexWait waiter, they recheck their word |
| 998    | sys_yield             | -                             | gives the core to another runnable task, returns 0    |
| 999    | sys_exit              | exit_code                     | `exit_current`: the task tears itself down, never returns |

Unimplemented (return u64::MAX): 3 (read), 4 (get_line), 8 (load_file), 9 (unload_file), 996 (get_process_info).

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
push r15, r14, r13, r12, rbp, rbx  <- callee-saved
push r9(arg6), r8(arg5), r10(arg4), rdx(arg3), rsi(arg2), rdi(arg1)
push rax                            <- syscall_num  *** must not be omitted ***
push r11                            <- rflags (saved by CPU in r11 on SYSCALL)
push rcx                            <- user_rip (saved by CPU in rcx on SYSCALL)
push user_rsp                       <- saved in the per-core scratch before the stack switch (pushed first)
```
On the pop side, the syscall_num slot is skipped with `add rsp, 8` because RAX already
holds the return value from `handle_syscall_inner` and must reach userspace unchanged.

### SFMASK and interrupt safety

SFMASK = `1 << 9` (masks IF on SYSCALL entry). This keeps the timer from firing while the handler is on the
kernel stack but SS is still in the state `syscall` left it in, and protects `user_rsp_scratch`, which is per core.
Interrupts are re-enabled at the start of `handle_syscall_inner` (`sti`) so long syscalls are not blocked from
interrupts, and disabled again before `exit_to_user`.

### sys_yield (998)

`scheduler::core_yield_executing_tasks(core_id)`: the task is queued in the `expired` bank and the core runs the next task. It returns
0 through the normal syscall return, so no user register is lost. With nothing else runnable on the core it
returns at once.

### Syscall Stack + Guard

There is no per-core syscall stack. `syscall_handler` runs on the kernel stack of the task, `gdt::KERNEL_TASK_STACKS`
(64 KiB + unmapped 4 KiB guard per slot, 16 byte aligned top). `run_task` writes the stack top of the next task
to `SYSCALL_DATA[core].stack_top` (read through `gs:0`) and to `TSS.privilege_stack_table[0]`, so ring 3 entries by
interrupt and by `syscall` land on the same stack.

```rust
#[repr(C, align(64))]
struct PerCoreSyscallData {
    stack_top: u64,          // must be first field, naked asm reads gs:0
    user_rsp_scratch: u64,
}
static mut SYSCALL_DATA: [PerCoreSyscallData; MAX_CORES]
// KERNEL_GS_BASE = &SYSCALL_DATA[core]
```

`gdt::assert_rsp_in_task_stack(core_id)` (`debug_assert!(bottom<=rsp<=top)` against the current task) runs at the
top of `handle_syscall_inner` and in the timer handler for ring 3 interrupts. `handle_syscall_inner` also asserts
that the frame pointer is 16 byte aligned.

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

- Stubs still return u64::MAX: 3, 4, 8, 9, 996. `CreateProcess (0)` is partial (hardcodes `ODYS_ELF`).
- The kernel is not preemptible: a long syscall delays the reschedule and a kill until it returns.
- Process exit code from sys_exit does not propagate to parent via wait() (no wait).
- User pointers passed to syscalls are range checked only, not walked through the page tables.
- A task blocked forever (for example `sys_wait_for_input` without a window) only leaves through a kill.
