# Scheduler Rework Plan

Goal: turn the cooperative dispatcher in `kernel/src/process/scheduler.rs` into a preemptive SMP scheduler with per-task kernel stacks, sleep/wakeup, per-core run queues and work stealing, and remove the races in the current switch and kill paths.

Target model: xv6/Linux style. Each task has its own kernel stack. A task enters the per-core scheduler context through `switch_context` from kernel mode. User registers live in a frame on the task's kernel stack and are restored by the normal syscall or interrupt return. Nothing is ever unwound or discarded.

## Rules for the implementer

- All code must follow `docs/rust_coding_guidelines.md`. Read it first. In short: no magic numbers, no dyn, debug_assert over Result, no padded alignment, comments only when non-obvious and not ending with a dot, ASCII only, breathing space between logical blocks.
- Read `docs/initial_overview.md`, `docs/process.md`, `docs/syscalls.md` and `docs/knowledge_gaps.md` before starting.
- Work phase by phase in order. A phase is done only when the kernel builds and `cargo xtask test` and `cargo xtask usertest` pass, including the tests added in that phase.
- Do not commit. Leave all changes in the working tree. Do not touch `/home/bawercx/repos/cloned/bigos`.
- If a phase cannot be completed, stop there and report exactly what failed, with log excerpts. Do not weaken, skip or delete a test to get a pass. Do not work around a failure with a sleep or a longer timeout.
- Per-switch logging must be off by default behind `const SCHED_DEBUG_PRINT: bool = false`, following the `TIMER_DEBUG_PRINT` pattern. Never log while holding a run queue lock.
- Kernel code must stay free of xmm instructions. See `docs/knowledge_gaps.md`.

## Findings addressed

| ID | Finding | Phase |
| --- | --- | --- |
| Y1 | Yield does not restore GPRs, leaks kernel register values to user | 1 |
| Y2 | Higher priority task never yields to lower priority | 4 |
| Y3 | Lone task yield spins, `read_blocking` busy-waits | 4, 5 |
| Y4 | Yield path is heavy: 8 log lines, 6 lock acquisitions | 1, 4 |
| S1 | No time slices, `on_timer_tick` empty | 2, 4 |
| S2 | Task running on another core cannot be killed | 3 |
| S3 | Preemption saves only rip/rsp/rflags | 2 |
| S4 | No sleep, blocked queue and states are dead | 5 |
| S5 | Syscall stack is per core, cannot block inside a syscall | 1 |
| S6 | No balancing after creation | 6 |
| S7 | No reschedule IPI, up to 10ms wake latency | 2 |
| S8 | Per-core queues sit behind one global lock | 4 |
| R1 | Kill vs dispatch window, `scheduler.rs:406` to `:422` | 1, 3 |
| R2 | Kill vs re-enqueue window, `scheduler.rs:493` to `:501` | 1, 3 |
| R3 | `asm!` at `scheduler.rs:466` has undeclared clobbers | 1 |
| R4 | `mov rsp` inside a Rust function at `scheduler.rs:368` | 1 |
| C1 | `CorePool` allocation, `KernelThread`, `ThreadGroup`, duplicate current tracking | 7 |

## Phase 0: baseline

1. Run `cargo xtask test` and `cargo xtask usertest`. Record the result.
2. Run `cargo xtask usertest test_user_fpu_switch` at least 5 times. One earlier run timed out with exit 124 and the log stopped inside `create_process_from_elf` for PID 5, with core 0 holding `PROCESS_MANAGER`. The cause is unknown.
3. If it reproduces, find the root cause before moving on. Check lock order between `PROCESS_MANAGER`, `SCHEDULER`, `CORE_POOL`, the frame allocator and serial. If the fix belongs to a later phase, say so in the report and continue.

Environment notes: a fresh checkout failed to build `user/rustspace` because the locked `x86_64 0.15.4` does not compile on the current nightly. Only if that happens in this tree, run `cargo +nightly update -p x86_64 --precise 0.15.5` in `user/rustspace` and report it.

## Phase 1: per-task kernel stacks and real context switch

Fixes Y1, S5, R1 and R2 partly, R3, R4, part of Y4.

Task table:
- Add `MAX_TASKS: usize = 64`. Add `static mut KERNEL_TASK_STACKS: [GuardedKernelStack; MAX_TASKS]` in `gdt.rs`. No allocator, the pool is static BSS. Unmap each guard page at boot through `ensure_guard_unmapped`, the same way as existing stacks.
- Add a static `TASKS: [TaskSlot; MAX_TASKS]` in a new `kernel/src/process/task.rs`. A slot has: `pid`, `state`, `priority`, `saved_rsp`, `kernel_stack_top`, `cr3`, `fpu_state`, `home_core`, `on_cpu`, `kill_pending`, `exit_code_request`, `cpu_ticks`. Use atomics where a field is touched from more than one core.
- Move `fpu_state` out of `Process` into `TaskSlot` so the switch path takes no `PROCESS_MANAGER` lock. `FpuState` must be 16-byte aligned for `fxsave64`.
- Slot exhaustion makes process creation return a `ProcessError`.
- Stale references are detected by comparing a stored `(slot, pid)` pair to the slot's current pid. PIDs are never reused.

Stack switching:
- Add `#[unsafe(naked)] unsafe extern "C" fn switch_context(save_rsp: *mut u64, load_rsp: u64)`. Push `rbx rbp r12 r13 r14 r15`, store rsp, load rsp, pop, `ret`.
- Add a naked trampoline that loads the scheduler stack and calls `run_on_core_loop(core_id)`, and change its callers. Delete the `mov rsp` block and the 4KiB headroom hack.
- Delete `KERNEL_RSP_ON_CORE`, `return_to_scheduler` and the `asm!` block at `scheduler.rs:466`.
- Add per-core `scheduler_rsp`.

Dispatch, in `run_on_core_loop`, with interrupts off:
1. In one critical section, pick the next task and mark it current. No separate lock windows.
2. Set `on_cpu`, set current pid for the core.
3. Write `SYSCALL_SLOTS[core].data.stack_top` and TSS `privilege_stack_table[0]` to the task's `kernel_stack_top`. Ring 3 entries then land on the task stack.
4. Restore FPU, write CR3, `switch_context`.
5. After `switch_context` returns, clear `on_cpu`.

`core_run_scheduling(core_id)`:
- May be called from kernel mode, on a task stack, with no locks held.
- Saves IF, disables interrupts, saves FPU, requeues if the task is still runnable, `switch_context` to the scheduler, restores IF.
- `sys_yield` becomes a call to `core_run_scheduling(core_id)` that returns normally through `syscall_handler` and `sysretq`. Return value 0. Delete `SyscallFrame::store_into_context`.
- Remove the GPR fields from `ExecutionContext`. Keep what process creation still needs, or remove the struct if nothing does.

First entry:
- At creation, build on the task's kernel stack a `switch_context` frame returning into a naked `task_first_entry`, above it a `TrapFrame` with entry rip, user rsp, default rflags, user cs and ss.
- `task_first_entry` loads user data selectors into ds, es, fs, gs, as `jump_to_userspace` does, clears every GPR and does `iretq`. Delete `jump_to_userspace`.

Watch out:
- `syscall_handler` assumes a stack top that is 8 mod 16, hence `sub rsp, 8`. A 16-byte aligned task stack top must drop that adjustment. It pushes 16 qwords. Confirm with a debug_assert in Rust that rsp alignment is correct at the call.
- `assert_rsp_in_bounds`, `assert_syscall_stack_bounds` and `test_guard_pages` assume per-core stacks. Change the checks to the current task's stack and extend `test_guard_pages` to cover task stacks.
- GS handling stays as is: the kernel runs with `GS.base` 0 and `KERNEL_GS_BASE` pointing at the per-core slot. `user_rsp_scratch` stays per core because IF is off during entry.

Tests:
- Add `user/rustspace/src/bin/regprobe.rs` and `kernel/src/bin/test_user_regprobe.rs`. A working starting point is at `/tmp/claude-1000/-home-bawercx-repos-MofuOS/2e06109c-aebc-48ee-8134-a3b9ecadc4e0/scratchpad/wt/user/rustspace/src/bin/regprobe.rs` and `.../wt/kernel/src/bin/test_user_regprobe.rs`, plus the `REGPROBE_ELF` line in `.../wt/kernel/src/process/elf_loader.rs`. Register the binary in `user/rustspace/Cargo.toml`. Rewrite it to meet the coding guidelines. It must also check that the yield return value is 0. It must pass.
- Add a `fpustress` variant or mode that does not save callee-saved registers by itself.

## Phase 2: full register traps, preemption, IPI

Fixes S1 partly, S3, S7.

- Define `#[repr(C)] struct TrapFrame` with all 15 GPRs followed by the hardware frame.
- Add a macro that generates a naked interrupt stub: push all GPRs, call a Rust handler with a frame pointer, pop, `iretq`. Convert Timer, Keyboard and Mouse from `x86-interrupt`, and add a new `InterruptIndex::Reschedule = 0xF0`. Replace the IDT entries.
- Interrupted ring 0 code: do the work, EOI, return. No rescheduling, the kernel stays non-preemptive.
- Interrupted ring 3 code: do the work, EOI first, then run `exit_to_user()`.
- `exit_to_user()`, with interrupts off, loops until clear: if `CORE_SHOULD_BE_RESCHEDULED`, call `core_run_scheduling(core_id)`; in phase 3, if `kill_pending`, exit. It returns with interrupts off.
- Syscall path: rename the current `handle_syscall_inner` body to `dispatch_syscall`. The new `handle_syscall_inner` calls it, keeps the return value, then runs `exit_to_user()`. All early returns are then covered.
- The stack segment hack in `timer_interrupt_handler` (`ss_slot.add(4)`) currently applies only to the timer. Find out why SS needs it. Remove it if the reason is gone. Otherwise apply it in the common stub for all ring 3 returns and explain the cause in a comment.
- Timer tick: decrement a per-core `slice_remaining_ticks`, increment `cpu_ticks`, set `CORE_SHOULD_BE_RESCHEDULED` on zero when another task is ready. Remove `PREEMPTION_ENABLED` and `on_timer_tick`.
- Replace `LAPIC_ADDRESS: Mutex` with an atomic pointer. `interrupt_over` runs in IRQ context and must not spin on a lock.
- Add `send_reschedule_ipi(core_id)` through the LAPIC ICR. Keep a `static` core id to LAPIC id table so it needs no lock.
- Idle must use `x86_64::instructions::interrupts::enable_and_hlt()`. The current `enable(); hlt();` loses a wakeup that lands between the two.
- Enqueue on a remote core sends the IPI when that core is idle or runs a lower priority.

Test: `test_user_preempt`, with `AP_CORE_COUNT + 1` or more spinners that never yield and each bump a counter in a shared page or report through exit. All must make progress and register values must hold under preemption.

## Phase 3: one exit path, kill by request

Fixes S2, R1, R2.

Rule: a task's address space, windows and task slot are torn down only by the task itself, on its own kernel stack.

- `TerminateProcess` on another task: set `kill_pending` and `exit_code_request`, wake it if blocked, send a reschedule IPI to its core, return 0. Remove the "running on core busy" refusal.
- `exit_to_user()` checks `kill_pending`. `task_first_entry` checks it too, so a task killed before its first run exits without entering ring 3.
- `exit_current(code)`: write kernel CR3, `destroy_windows_of_process`, tear down in `PROCESS_MANAGER`, set `Terminated` and `exit_code`, set slot state `Dead`, `core_run_scheduling(core_id)`. It never returns.
- The scheduler loop reaps a `Dead` task after `switch_context` returns: release the stack and slot. The stack is no longer in use at that point.
- `sys_exit` and self `TerminateProcess` call `exit_current`.
- `terminate_process` cascade becomes kill requests to children. Remove its unconditional `interrupts::enable()`.
- Check every caller of `terminate_process` and `sys_terminate_process`, in kernel, tests and userspace, for code that assumes the target is gone when the call returns. Adapt them. In particular check `CORE_POOL` release there, and `remove_pid`, which should no longer be needed.

Test: `test_user_kill`. A parent kills a spinner running on another core and the target reaches `Terminated` within 1s. Then a stress loop of at least 200 spawn and kill rounds with no fault and with `free_frame_count` back to its start value.

## Phase 4: per-core run queues and fair priorities

Fixes Y2, Y3 partly, Y4, S1, S8.

- Add `IrqMutex<T>` around `spin::Mutex`: saves IF, disables interrupts, locks. Guard drop unlocks, then restores IF.
- Replace `SCHEDULER` with `static CORE_RUN_QUEUES: [CacheAligned<IrqMutex<CoreRunQueue>>; MAX_CORES]`. Never hold two run queue locks at once.
- `CoreRunQueue` has two banks, `active` and `expired`. Each bank has `MAX_PRIORITY + 1` queues and a `u8` bitmap of non-empty levels. queues are intrusive `next: u16` links through `TaskSlot`, so no allocation and no copying.
- `pick_next_task` takes the highest set bit in `active`. If `active` is empty, swap the banks.
- Slice length is `(priority + 1) * BASE_SLICE_TICKS`. Higher priority runs first and longer.
- A task that exhausts its slice or yields goes to `expired`. A woken task goes to `active`. So every runnable task runs once per epoch, a priority 7 yielder gives way to priority 1, and nothing starves.
- Yield with nothing else ready returns at once without switching.
- Per-core `RUNNABLE_TASKS_COUNT: AtomicU32` drives placement in `create_process_from_elf` without locks. Core 0 stays out of scheduling.
- Keep `get_current_process_for_core` as the API, so `syscall.rs` does not churn. It is the only record of the current task.

Test: `test_user_priority`. Fill every AP core with priority 7 yield loops, plus low priority tasks that must finish within the timeout. A second case does the same with non-yielding spinners.

## Phase 5: blocking and sleep

Fixes S4, Y3.

- Make `TaskSlot.state` an `AtomicU8` state machine: `Ready`, `Running`, `Blocked`, `Dead`.
- `wake_task((slot, pid))`: check pid, then `CAS(Blocked, Ready)`. On success lock the home run queue, enqueue, and send an IPI if needed. Any other state is a no-op, so the call is idempotent. `home_core` is stable while `Blocked`, because blocked tasks do not migrate.
- `core_run_scheduling(core_id)` does `CAS(Running, Ready)` and enqueues on success. `Blocked`, or `Ready` from an early wake, is not enqueued again.
- A woken task may sit in a queue before it has switched out. That is safe because only its home core dequeues it, and that core does so after the switch. Phase 6 stealing must check `on_cpu`.
- Add `WaitQueue`, a fixed-capacity list of `(slot, pid)` under `IrqMutex`.
- Add `block_current_task_until_event(queue, condition)`, generic with no dyn, with this exact order to avoid lost wakeups:

```rust
loop {
    if condition() || current_kill_pending() {
        return;
    }

    queue.add_current_task_and_mark_blocked();

    if condition() || current_kill_pending() {
        wake_task(current_task_ref());
    }

    core_run_scheduling(core_id);
}
```

- Add per-core sleeping_tasks, checked in the timer tick under the run queue lock. Add `sys_sleep_ms` in kernel and `rustspace`.
- Add a syscall that blocks until the calling process has an input event. The event producer wakes it. Read `kernel/src/events` and find how events reach a process, including from IRQ context. Any lock the IRQ path takes must be IRQ-safe.
- Change `read_blocking` in `user/rustspace/src/lib.rs` to use it, so waiting costs no CPU.
- Replace the pacing `sys_yield` loops in `window_app` and `theophe` where they are waiting rather than yielding.
- Delete `blocked_queue` and the old block functions.

Test: `test_user_sleep`. Sleep 100ms and assert elapsed time is at least 100ms and that the sleeping task's `cpu_ticks` stay near zero. Also cover kill during sleep, and a wake racing with block in a loop of at least 1000 rounds between two tasks.

## Phase 6: work stealing

Fixes S6.

- When its local queue is empty, an idle core scans the other AP cores with `try_lock`. It pops a task with `on_cpu == false`, drops the core_from lock, sets `home_core`, then enqueues locally. Skip core 0. Never hold two locks.
- Add a per-task migration counter for tests.
- Always write CR3 on dispatch. A migrated task may have unmapped memory on another core, so "same CR3" is not proof that the TLB is valid.
- Load the kernel CR3 before `hlt`. An idle core must not keep a CR3 whose tables another core may free after the task migrates.

Test: `test_user_steal`. Create more tasks than one core should hold and check that core_migration_count happen and all tasks finish. Rerun every usertest with this enabled.

## Phase 7: cleanup and docs

Fixes C1.

- Delete `kernel_thread.rs`, `ThreadGroup` use in `ProcessManager`, `save_thread_context`, `restore_thread_context`, `switch_threads`, `execute_process_direct`, unused `CorePool` allocation code and `SchedulerStats` if unused. Keep only what is still used, such as total core count and the LAPIC table.
- Update `docs/process.md`, `docs/syscalls.md`, `docs/knowledge_gaps.md`, `docs/overview.md`, `docs/initial_overview.md`. Remove stale statements about `return_to_scheduler` and GPR restore.
- Build the kernel in release: `cargo build -p kernel --target x86_64-unknown-none -Z build-std=core,alloc --release`. Boot `test_user_regprobe` and `test_user_preempt` in release if xtask can be extended for that with a small change. Otherwise report it as not verified.
- Run `cargo xtask test` and `cargo xtask usertest` three times in a row. Run `cargo xtask clippy` and `cargo xtask fmt`.
- Confirm no xmm in the kernel with the `objdump` check from `docs/knowledge_gaps.md`.
- Delete this plan only if asked. Otherwise leave it.
