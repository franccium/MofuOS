use core::sync::atomic::AtomicU64;

use crate::data_structures::dequeue::Dequeue;
use crate::data_structures::vector::Vec;
use crate::process::execution::jump_to_userspace;
use crate::process::process::INVALID_PID;
use crate::process::process_manager::PROCESS_MANAGER;
/// Scheduler - Priority-based, preemptive scheduler with per-core queues
///
/// Uses the One-to-One threading model where each process is assigned a kernel thread
/// that runs on a dedicated CPU core. Each core has its own scheduler queue.
use crate::process::{CORE_POOL, PID, Process};
use crate::util::cpuinfo::get_current_core_id;
use crate::{MAX_CORES, serial_println, serial_println_core};
use core::sync::atomic::Ordering;
use spin::Mutex;
use x86_64::PhysAddr;
use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::PhysFrame;

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

/// Per-core scheduler queue with priority support
pub struct CoreScheduler {
    /// CPU core ID this scheduler manages
    core_id: u8,
    /// Ready queue organized by priority (8 levels: 0=lowest, 7=highest)
    /// Threads with same priority use round-robin within the level
    ready_queues: [Dequeue<PID>; 8],
    /// Blocked/waiting threads (waiting for I/O or events)
    blocked_queue: Dequeue<PID>,
    /// Currently running thread on this core (if any)
    current_thread: Option<PID>,
}

impl CoreScheduler {
    pub fn new(core_id: u8) -> Self {
        Self {
            core_id,
            ready_queues: Default::default(),
            blocked_queue: Dequeue::new(),
            current_thread: None,
        }
    }

    /// Enqueue a thread in the appropriate priority queue
    pub fn enqueue_ready(&mut self, pid: PID, priority: u8) {
        let queue_idx = (priority as usize).min(7); // Clamp to valid range
        self.ready_queues[queue_idx].push_back(pid);
    }

    /// Dequeue next thread to run (picks highest priority ready thread)
    pub fn dequeue_next(&mut self) -> Option<PID> {
        // Search from highest to lowest priority
        for queue in self.ready_queues.iter_mut().rev() {
            if queue.len() > 0 {
                return Some(queue.pop_front());
            }
        }
        None
    }

    /// Move thread to blocked queue
    pub fn block_thread(&mut self, pid: PID) {
        if self.current_thread == Some(pid) {
            self.current_thread = None;
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

    /// Set the currently running thread
    pub fn set_current_running(&mut self, pid: PID) {
        self.current_thread = Some(pid);
    }

    /// Get current running thread
    pub fn current_running(&self) -> Option<PID> {
        self.current_thread
    }

    /// Check if any threads are ready to run
    pub fn has_ready_threads(&self) -> bool {
        self.ready_queues.iter().any(|q| !q.is_empty())
    }

    /// Count total ready threads
    pub fn ready_count(&self) -> usize {
        self.ready_queues.iter().map(|q| q.len()).sum()
    }

    /// Count total blocked threads
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
        // Default to 1 core; will be updated when core count is detected
        let mut per_core = Vec::new();
        per_core.push(CoreScheduler::new(0));

        Self {
            per_core,
            core_count: 1,
        }
    }

    /// Initialize scheduler with detected core count
    pub fn init_with_core_count(&mut self, core_count: u8) {
        if core_count == 0 || core_count > 64 {
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

    /// Get mutable reference to a specific core's scheduler
    fn get_core_scheduler_mut(&mut self, core_id: u8) -> Option<&mut CoreScheduler> {
        let idx = core_id as usize;
        if idx < self.per_core.len() {
            // Access via slice to get the reference safely
            Some(&mut self.per_core.as_mut_slice()[idx])
        } else {
            None
        }
    }

    /// Get immutable reference to a specific core's scheduler
    fn get_core_scheduler(&self, core_id: u8) -> Option<&CoreScheduler> {
        let idx = core_id as usize;
        if idx < self.per_core.len() {
            // Access via slice to get the reference safely
            Some(&self.per_core.as_slice()[idx])
        } else {
            None
        }
    }

    pub fn on_timer_tick(&mut self, core_id: u8) {
        if let Some(scheduler) = self.get_core_scheduler_mut(core_id) {
            let curr = scheduler.current_running();

            // serial_println!(
            //     "Core {}: Timer tick - Current PID: {:?}",
            //     core_id,
            //     curr,
            // );
        }
    }

    /// Enqueue a thread to a specific core's scheduler
    pub fn enqueue_on_core(&mut self, core_id: u8, pid: PID, priority: u8) {
        if let Some(scheduler) = self.get_core_scheduler_mut(core_id) {
            scheduler.enqueue_ready(pid, priority);
        }
    }

    /// Get next thread to run on a specific core
    pub fn get_next_on_core(&mut self, core_id: u8) -> Option<PID> {
        if let Some(scheduler) = self.get_core_scheduler_mut(core_id) {
            scheduler.dequeue_next()
        } else {
            None
        }
    }

    /// Block a thread on a specific core
    pub fn block_on_core(&mut self, core_id: u8, pid: PID) {
        if let Some(scheduler) = self.get_core_scheduler_mut(core_id) {
            scheduler.block_thread(pid);
        }
    }

    /// Unblock a thread on a specific core
    pub fn unblock_on_core(&mut self, core_id: u8, pid: PID, priority: u8) -> bool {
        if let Some(scheduler) = self.get_core_scheduler_mut(core_id) {
            scheduler.unblock_thread(pid, priority)
        } else {
            false
        }
    }

    /// Set current running thread on a core
    pub fn set_current_on_core(&mut self, core_id: u8, pid: PID) {
        if let Some(scheduler) = self.get_core_scheduler_mut(core_id) {
            scheduler.set_current_running(pid);
        }
    }

    /// Get current running thread on a core
    pub fn current_on_core(&self, core_id: u8) -> Option<PID> {
        self.get_core_scheduler(core_id)
            .and_then(|s| s.current_running())
    }

    /// Check if a core has ready threads
    pub fn has_ready_threads_on_core(&self, core_id: u8) -> bool {
        self.get_core_scheduler(core_id)
            .map(|s| s.has_ready_threads())
            .unwrap_or(false)
    }

    /// Get global scheduler statistics
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

const SCHEDULER_ACTUALLY_RUN_A_PROCESS: bool = true;

#[repr(C)]
struct KernelContext {
    rsp: u64,
    rbp: u64,
    rbx: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
}

/// Save the current kernel context
fn save_kernel_context() -> KernelContext {
    let mut ctx = KernelContext {
        rsp: 0,
        rbp: 0,
        rbx: 0,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
    };

    unsafe {
        core::arch::asm!(
            "mov {rsp}, rsp",
            "mov {rbp}, rbp",
            "mov {rbx}, rbx",
            "mov {r12}, r12",
            "mov {r13}, r13",
            "mov {r14}, r14",
            "mov {r15}, r15",
            "2:",
            rsp = out(reg) ctx.rsp,
            rbp = out(reg) ctx.rbp,
            rbx = out(reg) ctx.rbx,
            r12 = out(reg) ctx.r12,
            r13 = out(reg) ctx.r13,
            r14 = out(reg) ctx.r14,
            r15 = out(reg) ctx.r15,
            options(nostack)
        );
    }

    ctx
}

/// Restore kernel context (called after returning from userspace)
fn restore_kernel_context(ctx: &KernelContext) {
    unsafe {
        core::arch::asm!(
            "mov rsp, {rsp}",
            "mov rbp, {rbp}",
            "mov rbx, {rbx}",
            "mov r12, {r12}",
            "mov r13, {r13}",
            "mov r14, {r14}",
            "mov r15, {r15}",
            rsp = in(reg) ctx.rsp,
            rbp = in(reg) ctx.rbp,
            rbx = in(reg) ctx.rbx,
            r12 = in(reg) ctx.r12,
            r13 = in(reg) ctx.r13,
            r14 = in(reg) ctx.r14,
            r15 = in(reg) ctx.r15,
            options(noreturn, nostack)
        );
    }
}

/// Execute a process in userspace
/// This function returns when the process is preempted or makes a syscall
pub fn execute_process(process: &Process) {
    let pid = process.pid;
    let core_id = get_current_core_id();

    set_current_process_for_core(core_id, pid);

    // Save current kernel context
    let kernel_context = save_kernel_context();

    // Switch to the process's page table
    let page_table_frame = PhysFrame::containing_address(PhysAddr::new(
        process.execution_context.page_table_base_phys,
    ));
    let old_page_table = Cr3::read();
    unsafe {
        Cr3::write(page_table_frame, Cr3Flags::empty());
    }

    // Jump to userspace
    // The process will come back via:
    // 1. Syscall (handled by syscall_handler)
    // 2. Timer interrupt (handled by timer_interrupt_handler)
    // 3. Page fault or other exception
    unsafe {
        jump_to_userspace(process.execution_context.rip, process.execution_context.rsp);
    }
    serial_println_core!("returned");
    // When we get back (via sysret or iret in interrupt handler):
    // Restore kernel page table
    unsafe {
        let (frame, flags) = old_page_table;
        Cr3::write(frame, flags);
    }

    // Restore kernel context
    restore_kernel_context(&kernel_context);
}

/// The main scheduler loop for each core
/// This function runs forever on each core
pub fn run_on_core_loop(core_id: u8) -> ! {
    serial_println_core!("Entering scheduler loop");

    loop {
        // Disable interrupts for atomic scheduler check
        x86_64::instructions::interrupts::disable();
        //serial_println_core!("disabled interrupts");

        // Get next process for this core
        let next_pid = {
            let mut scheduler = SCHEDULER.lock();
            scheduler.get_next_on_core(core_id)
        };

        serial_println_core!("got next on core");

        if let Some(pid) = next_pid {
            {
                let mut scheduler = SCHEDULER.lock();
                scheduler.set_current_on_core(core_id, pid);
            }

            serial_println_core!("Start running PID {}", pid);

            // Re-enable interrupts before running the process
            x86_64::instructions::interrupts::enable();

            // SWITCH TO THE PROCESS
            // This is where you'd do a context switch
            // For now, just signal that we'd run it
            let process_priority = 4;

            serial_println_core!("set current on core");

            if SCHEDULER_ACTUALLY_RUN_A_PROCESS {
                // Extract the execution context while holding the PM lock, then
                // drop the lock BEFORE jumping to userspace.  jump_to_userspace
                // is `-> !` (it never returns here), so any lock held across it
                // is held forever.  When the process makes a syscall (e.g. exit)
                // the syscall handler tries to acquire PROCESS_MANAGER — which
                // would deadlock if we still held it here.
                let exec_ctx = {
                    let pm = PROCESS_MANAGER.lock();
                    match pm.get_process(pid) {
                        Ok(process) => Some((
                            process.pid,
                            process.execution_context.rip,
                            process.execution_context.rsp,
                            process.execution_context.page_table_base_phys,
                        )),
                        Err(_) => None,
                    }
                    // pm lock is released here
                };

                if let Some((proc_pid, rip, rsp, cr3)) = exec_ctx {
                    set_current_process_for_core(core_id, proc_pid);

                    let page_table_frame = PhysFrame::containing_address(PhysAddr::new(cr3));
                    unsafe {
                        Cr3::write(page_table_frame, Cr3Flags::empty());
                    }

                    unsafe {
                        jump_to_userspace(rip, rsp);
                    }
                }
            } else {
                serial_println!("Would run PID {}", pid);
            }

            mark_core_idle(core_id);

            {
                let mut scheduler = SCHEDULER.lock();
                scheduler.enqueue_on_core(core_id, pid, process_priority);
            }
        } else {
            // No process available - enable interrupts and halt
            x86_64::instructions::interrupts::enable();
            x86_64::instructions::hlt();
        }
    }
}

/// Mark a core as ready to receive work
pub fn core_ready(core_id: u8) {
    serial_println!("Core {}: Marked as ready", core_id);
    // Any additional initialization for the core in scheduler
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulerStats {
    pub core_count: u8,
    pub total_ready: usize,
    pub total_blocked: usize,
}
