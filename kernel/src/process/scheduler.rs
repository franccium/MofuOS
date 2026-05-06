use crate::data_structures::dequeue::Dequeue;
use crate::data_structures::vector::Vec;
/// Scheduler - Priority-based, preemptive scheduler with per-core queues
///
/// Uses the One-to-One threading model where each process is assigned a kernel thread
/// that runs on a dedicated CPU core. Each core has its own scheduler queue.
use crate::process::PID;
use crate::serial_println;
use spin::Mutex;

lazy_static::lazy_static! {
    pub static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
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

            serial_println!(
                "Core {}: Timer tick - Current PID: {:?}",
                core_id,
                curr,
            );
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

#[derive(Debug, Clone, Copy)]
pub struct SchedulerStats {
    pub core_count: u8,
    pub total_ready: usize,
    pub total_blocked: usize,
}
