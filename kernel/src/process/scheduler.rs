use crate::data_structures::dequeue::Dequeue;
use crate::data_structures::vector::Vec;
use crate::process::execution::jump_to_userspace;
use crate::process::process::INVALID_PID;
use crate::process::process_manager::PROCESS_MANAGER;
use crate::process::{CORE_POOL, PID};
use crate::util::cpuinfo::get_current_core_id;
use crate::{MAX_CORES, serial_println, serial_println_core};
/// Scheduler - Priority-based, preemptive scheduler with per-core queues
///
/// Uses the One-to-One threading model where each process is assigned a kernel thread
/// that runs on a dedicated CPU core. Each core has its own scheduler queue.
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

lazy_static::lazy_static! {
    pub static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
}

static CURRENT_PROCESS_ON_CORE: [AtomicU64; MAX_CORES as usize] = {
    const EMPTY: AtomicU64 = AtomicU64::new(INVALID_PID as u64);
    [EMPTY; MAX_CORES as usize]
};

pub fn set_current_process_for_core(core_id: u8, pid: PID) {
    CURRENT_PROCESS_ON_CORE[core_id as usize].store(pid as u64, Ordering::SeqCst);
}

pub fn get_current_process_for_core(core_id: u8) -> PID {
    CURRENT_PROCESS_ON_CORE[core_id as usize].load(Ordering::SeqCst) as PID
}

pub fn mark_core_idle(core_id: u8) {
    CURRENT_PROCESS_ON_CORE[core_id as usize].store(INVALID_PID as u64, Ordering::SeqCst);
    CORE_POOL.lock().mark_available(core_id);
}

// Saved kernel RSP for each core, set just before jump_to_userspace.
// sys_exit restores this to unwind back into run_on_core_loop.
// Must be in writable memory (.data / static mut) — the asm writes via raw ptr.
static mut KERNEL_RSP_ON_CORE: [u64; MAX_CORES as usize] = [0u64; MAX_CORES as usize];

/// Called by sys_exit to resume the scheduler loop on the current core.
/// Restores the kernel RSP saved before jump_to_userspace and returns into
/// run_on_core_loop at the instruction after the save point.
pub fn return_to_scheduler() -> ! {
    let core_id = get_current_core_id();
    let kernel_rsp = unsafe { KERNEL_RSP_ON_CORE[core_id as usize] };
    debug_assert!(
        kernel_rsp != 0,
        "return_to_scheduler: no saved RSP for core {}",
        core_id
    );
    unsafe {
        core::arch::asm!(
            "mov rsp, {rsp}",
            "ret",
            rsp = in(reg) kernel_rsp,
            options(noreturn, nostack)
        );
    }
}

const MAX_PRIORITY: usize = 7;

pub struct CoreScheduler {
    core_id: u8,
    /// Ready queue organized by priority (8 levels: 0=lowest, 7=highest)
    /// Threads with same priority use round-robin within the level
    ready_queues: [Dequeue<PID>; MAX_PRIORITY + 1],
    /// Blocked/waiting threads (waiting for I/O or events)
    blocked_queue: Dequeue<PID>,
    /// Currently running thread on this core
    current_thread: PID,
}

const QUEUE_INITIAL_CAPACITY: usize = 8;

impl CoreScheduler {
    pub fn new(core_id: u8) -> Self {
        Self {
            core_id,
            ready_queues: core::array::from_fn(|_| Dequeue::with_capacity(QUEUE_INITIAL_CAPACITY)),
            blocked_queue: Dequeue::with_capacity(QUEUE_INITIAL_CAPACITY),
            current_thread: INVALID_PID,
        }
    }

    /// Enqueue a thread in the appropriate priority queue
    pub fn enqueue_ready(&mut self, pid: PID, priority: u8) {
        debug_assert!(
            priority <= MAX_PRIORITY as u8,
            "Priority must be between 0 and {}",
            MAX_PRIORITY
        );
        serial_println_core!(
            "Scheduler: Enqueuing PID {} on core {} with priority {}",
            pid,
            self.core_id,
            priority
        );
        let queue_idx = priority as usize;
        self.ready_queues[queue_idx].push_back(pid);
    }

    /// Dequeue next thread to run (picks highest priority ready thread)
    pub fn dequeue_next(&mut self) -> (PID, u8) {
        // Search from highest to lowest priority
        serial_println_core!("Scheduler: Dequeuing next thread on core {}", self.core_id);
        for (priority, queue) in self.ready_queues.iter_mut().enumerate().rev() {
            serial_println_core!(
                "Scheduler: Checking priority {} queue (len={})",
                priority,
                queue.len()
            );
            if queue.len() > 0 {
                return (queue.pop_front(), priority as u8);
            }
        }
        (INVALID_PID, 0)
    }

    /// Move thread to blocked queue
    pub fn block_thread(&mut self, pid: PID) {
        if self.current_thread == pid {
            self.current_thread = INVALID_PID;
        }
        self.blocked_queue.push_back(pid);
    }

    /// Try to move a blocked thread back to ready queue
    pub fn unblock_thread(&mut self, pid: PID, priority: u8) -> bool {
        // Check if thread is in blocked queue
        let mut found = false;
        let mut new_blocked = Dequeue::new();
        while self.blocked_queue.len() > 0 {
            let blocked_pid = self.blocked_queue.pop_front();
            if blocked_pid == pid {
                found = true;
            } else {
                new_blocked.push_back(blocked_pid);
            }
        }
        self.blocked_queue = new_blocked;

        if found {
            self.enqueue_ready(pid, priority);
        }
        found
    }

    pub fn set_current_running(&mut self, pid: PID) {
        self.current_thread = pid;
    }

    pub fn current_running(&self) -> PID {
        self.current_thread
    }

    pub fn has_ready_threads(&self) -> bool {
        self.ready_queues.iter().any(|q| !q.is_empty())
    }

    pub fn ready_count(&self) -> usize {
        self.ready_queues.iter().map(|q| q.len()).sum()
    }

    pub fn blocked_count(&self) -> usize {
        self.blocked_queue.len()
    }

    pub fn core_id(&self) -> u8 {
        self.core_id
    }
}

/// Global multi-core scheduler
pub struct Scheduler {
    /// Per-core schedulers (index = core_id)
    per_core: Vec<CoreScheduler>,
    /// Total number of cores
    core_count: u8,
}

impl Scheduler {
    pub fn new() -> Self {
        let mut per_core = Vec::new();
        per_core.push(CoreScheduler::new(0));

        Self {
            per_core,
            core_count: 1,
        }
    }

    pub fn init_with_core_count(&mut self, core_count: u8) {
        if core_count == 0 || core_count > MAX_CORES {
            serial_println!("ERROR: Invalid core count for scheduler: {}", core_count);
            return;
        }

        self.per_core.clear();
        for core_id in 0..core_count {
            self.per_core.push(CoreScheduler::new(core_id));
        }
        self.core_count = core_count;

        serial_println!("Scheduler initialized with {} cores", core_count);
    }

    fn get_core_scheduler_mut(&mut self, core_id: u8) -> &mut CoreScheduler {
        let idx: usize = core_id as usize;
        debug_assert!(idx < self.per_core.len());
        &mut self.per_core.as_mut_slice()[idx]
    }

    fn get_core_scheduler(&self, core_id: u8) -> &CoreScheduler {
        let idx: usize = core_id as usize;
        debug_assert!(idx < self.per_core.len());
        &self.per_core.as_slice()[idx]
    }

    pub fn on_timer_tick(&mut self, core_id: u8) {
        let scheduler = self.get_core_scheduler_mut(core_id);
        let curr = scheduler.current_running();
        // serial_println!(
        //     "Core {}: Timer tick - Current PID: {:?}",
    }

    pub fn enqueue_on_core(&mut self, core_id: u8, pid: PID, priority: u8) {
        let scheduler = self.get_core_scheduler_mut(core_id);
        scheduler.enqueue_ready(pid, priority);
    }

    pub fn get_next_on_core(&mut self, core_id: u8) -> (PID, u8) {
        let scheduler = self.get_core_scheduler_mut(core_id);
        scheduler.dequeue_next()
    }

    pub fn block_on_core(&mut self, core_id: u8, pid: PID) {
        let scheduler = self.get_core_scheduler_mut(core_id);
        scheduler.block_thread(pid);
    }

    pub fn unblock_on_core(&mut self, core_id: u8, pid: PID, priority: u8) -> bool {
        let scheduler = self.get_core_scheduler_mut(core_id);
        scheduler.unblock_thread(pid, priority)
    }

    pub fn set_current_on_core(&mut self, core_id: u8, pid: PID) {
        let scheduler = self.get_core_scheduler_mut(core_id);
        scheduler.set_current_running(pid);
    }

    pub fn current_on_core(&self, core_id: u8) -> PID {
        self.get_core_scheduler(core_id).current_running()
    }

    pub fn has_ready_threads_on_core(&self, core_id: u8) -> bool {
        self.get_core_scheduler(core_id).has_ready_threads()
    }

    pub fn get_stats(&self) -> SchedulerStats {
        let mut total_ready = 0;
        let mut total_blocked = 0;

        for core_scheduler in self.per_core.iter() {
            total_ready += core_scheduler.ready_count();
            total_blocked += core_scheduler.blocked_count();
        }

        SchedulerStats {
            core_count: self.core_count,
            total_ready,
            total_blocked,
        }
    }
}

pub fn run_on_core_loop(core_id: u8) -> ! {
    // Switch to a dedicated per-core scheduler stack before doing anything
    // that touches the stack. The Limine AP boot stack is small and not
    // guaranteed to have enough space for the scheduler loop's call depth.
    serial_println_core!(
        "run_on_core_loop: switching to per-core scheduler stack (core_id={})",
        core_id
    );
    let scheduler_stack_top = crate::gdt::get_scheduler_stack_top(core_id);
    let core_id_saved: u64;
    unsafe {
        core::arch::asm!(
            "mov {saved}, {id}",
            "mov rsp, {top}",
            saved = out(reg) core_id_saved,
            id = in(reg) core_id as u64,
            top = in(reg) scheduler_stack_top,
            options(nostack)
        );
    }
    let core_id = core_id_saved as u8;
    serial_println_core!("Entering scheduler loop (core_id={})", core_id);

    loop {
        x86_64::instructions::interrupts::disable();

        let (pid, priority) = {
            let mut scheduler = SCHEDULER.lock();
            scheduler.get_next_on_core(core_id)
        };

        if pid == INVALID_PID {
            x86_64::instructions::interrupts::enable();
            x86_64::instructions::hlt();
            continue;
        }

        {
            let mut scheduler = SCHEDULER.lock();
            scheduler.set_current_on_core(core_id, pid);
        }

        serial_println_core!("Running PID {}", pid);

        let exec_ctx = {
            let pm = PROCESS_MANAGER.lock();
            pm.get_process(pid).ok().map(|p| {
                (
                    p.execution_context.rip,
                    p.execution_context.rsp,
                    p.execution_context.page_table_base_phys,
                )
            })
        };

        if let Some((rip, rsp, cr3)) = exec_ctx {
            set_current_process_for_core(core_id, pid);

            // Add this before your asm block to test
            let kernel_rsp_slot = unsafe { &mut KERNEL_RSP_ON_CORE[core_id as usize] as *mut u64 };
            // write RSP into slot
            let rsp_value: u64;
            unsafe {
                core::arch::asm!(
                    "mov {}, rsp",
                    out(reg) rsp_value,
                    options(nostack, nomem)
                );
            }
            serial_println_core!("Saving kernel RSP for core {}: {:#x}", core_id, rsp_value);
            unsafe {KERNEL_RSP_ON_CORE[core_id as usize] = rsp_value};


            x86_64::instructions::interrupts::enable();

            let rip_value = x86_64::registers::read_rip();
            serial_println_core!("Current RIP before jump: {:#x}", rip_value.as_u64());
            

            // All five steps must be in one asm block so the forward label "2:"
            // is visible to the lea. The CR3 write is inside the block so the
            // RSP save happens while the kernel page table is still active.
            unsafe {
                core::arch::asm!(
                    // push return address while still on the kernel page table
                    "lea rax, [rip + 2f]",
                    "push rax",
                    // save RSP (kernel page table still active here)
                    //"mov [{slot}], rsp", // THIS CAUSES PageFaultErrorCode(PROTECTION_VIOLATION | CAUSED_BY_WRITE)
                    // switch to user page table
                    "mov cr3, {cr3}",
                    // jump to userspace — iretq, never returns normally
                    "jmp {jump}",
                    // return_to_scheduler() ret lands here
                    "2:",
                    //slot = in(reg) kernel_rsp_slot,
                    cr3 = in(reg) cr3,
                    jump = sym jump_to_userspace,
                    in("rdi") rip,
                    in("rsi") rsp,
                    lateout("rax") _,
                    options(nostack)
                );
            }
        }

        serial_println_core!(
            "Core {}: Returned from userspace PID {}",
            core_id,
            pid
        );
        // Process exited or was not found. Clean up for this iteration.
        // Disable interrupts immediately — we're back on the kernel stack and
        // about to acquire locks. The timer handler also acquires SCHEDULER,
        // so leaving interrupts on here causes a deadlock.
        x86_64::instructions::interrupts::disable();
        mark_core_idle(core_id);

        // Re-enqueue only if the process is still alive (not terminated by sys_exit).
        let is_terminated = {
            let pm = PROCESS_MANAGER.lock();
            pm.get_process(pid)
                .map(|p| p.state == crate::process::process::ProcessState::Terminated)
                .unwrap_or(true)
        };
        if !is_terminated {
            let mut scheduler = SCHEDULER.lock();
            scheduler.enqueue_on_core(core_id, pid, priority);
        } else {
            serial_println_core!("PID {} terminated, not re-enqueueing", pid);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulerStats {
    pub core_count: u8,
    pub total_ready: usize,
    pub total_blocked: usize,
}
