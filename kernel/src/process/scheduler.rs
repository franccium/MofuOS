use crate::data_structures::dequeue::Dequeue;
use crate::data_structures::vector::Vec;
use crate::process::execution::jump_to_userspace;
use crate::process::process::INVALID_PID;
use crate::process::process_manager::PROCESS_MANAGER;
use crate::process::{CORE_POOL, PID, Process};
use crate::util::cpuinfo::get_current_core_id;
use crate::{MAX_CORES, serial_println, serial_println_core};
/// Scheduler - Priority-based, preemptive scheduler with per-core queues
///
/// Uses the One-to-One threading model where each process is assigned a kernel thread
/// that runs on a dedicated CPU core. Each core has its own scheduler queue.
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use spin::Mutex;
use x86_64::PhysAddr;
use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::PhysFrame;

const RUN_A_PROCESS: bool = true;

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

impl CoreScheduler {
    pub fn new(core_id: u8) -> Self {
        Self {
            core_id,
            ready_queues: Default::default(),
            blocked_queue: Dequeue::new(),
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
        let queue_idx = priority as usize;
        self.ready_queues[queue_idx].push_back(pid);
    }

    /// Dequeue next thread to run (picks highest priority ready thread)
    pub fn dequeue_next(&mut self) -> (PID, u8) {
        // Search from highest to lowest priority
        for (priority, queue) in self.ready_queues.iter_mut().enumerate().rev() {
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

    pub fn on_timer_tick(&mut self, core_id: u8) {
        let scheduler = self.get_core_scheduler_mut(core_id);
        let curr = scheduler.current_running();
        // serial_println!(
        //     "Core {}: Timer tick - Current PID: {:?}",
        //     core_id,
        //     curr,
        // );
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
        self.get_core_scheduler(core_id)
            .and_then(|s| s.current_running())
    }

    pub fn has_ready_threads_on_core(&self, core_id: u8) -> bool {
        self.get_core_scheduler(core_id)
            .map(|s| s.has_ready_threads())
            .unwrap_or(false)
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
    unsafe {
        jump_to_userspace(process.execution_context.rip, process.execution_context.rsp);
    }

    serial_println_core!("Returned from userspace");

    // Restore kernel page table
    unsafe {
        let (frame, flags) = old_page_table;
        Cr3::write(frame, flags);
    }

    // Restore kernel context
    restore_kernel_context(&kernel_context);
}

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

        serial_println_core!("Got next process");

        let (pid, priority) = next_pid;
        if pid != INVALID_PID {
            {
                let mut scheduler = SCHEDULER.lock();
                scheduler.set_current_on_core(core_id, pid);
            }

            serial_println_core!("Start running PID {}", pid);

            // Re-enable interrupts before running the process
            x86_64::instructions::interrupts::enable();

            // Context switch to the proecss
            serial_println_core!("set current on core");

            if RUN_A_PROCESS {
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
                scheduler.enqueue_on_core(core_id, pid, priority);
            }
        } else {
            // No process available - enable interrupts and halt
            x86_64::instructions::interrupts::enable();
            x86_64::instructions::hlt();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulerStats {
    pub core_count: u8,
    pub total_ready: usize,
    pub total_blocked: usize,
}
