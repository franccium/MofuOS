use crate::data_structures::vector::Vec;
use crate::process::process::{ExecutionContext, PID};
use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThreadState {
    /// Waiting in scheduler queue for CPU core
    Ready,
    /// Currently executing on a CPU core
    Running,
    /// Blocked on I/O or other event
    Blocked,
    /// Thread has terminated
    Terminated,
}

/// A kernel thread that executes a user process
pub struct KernelThread {
    pub pid: PID,
    pub core_id: Option<u8>,
    pub priority: u8,
    pub state: ThreadState,
    pub exit_code: Option<i32>,
    pub context: ExecutionContext,
    /// cache warmth tracker
    pub last_core_id: Option<u8>,
    /// Thread name for debugging
    pub name: String,
}

impl KernelThread {
    pub fn new(
        pid: PID,
        priority: u8,
        name: String,
        entry_point: u64,
        stack_top: u64,
        page_table_base_phys: u64,
    ) -> Self {
        Self {
            pid,
            core_id: None,
            state: ThreadState::Ready,
            context: ExecutionContext::new(entry_point, stack_top, page_table_base_phys),
            priority,
            name,
            exit_code: None,
            last_core_id: None,
        }
    }

    pub fn assign_to_core(&mut self, core_id: u8) {
        self.last_core_id = self.core_id;
        self.core_id = Some(core_id);
    }

    pub fn release_core(&mut self) {
        self.core_id = None;
    }

    pub fn is_runnable(&self) -> bool {
        self.core_id.is_some() && self.state == ThreadState::Ready
    }

    pub fn terminate(&mut self, exit_code: i32) {
        self.state = ThreadState::Terminated;
        self.exit_code = Some(exit_code);
    }
}

/// Thread group manages all threads for a single process
pub struct ThreadGroup {
    pub pid: PID,
    pub threads: Vec<KernelThread>,
}

impl ThreadGroup {
    pub fn new(pid: PID, main_thread: KernelThread) -> Self {
        let mut threads = Vec::new();
        threads.push(main_thread);
        Self { pid, threads }
    }

    pub fn main_thread_mut(&mut self) -> Option<&mut KernelThread> {
        if self.threads.len() > 0 {
            Some(&mut self.threads.as_mut_slice()[0])
        } else {
            None
        }
    }

    pub fn main_thread(&self) -> Option<&KernelThread> {
        if self.threads.len() > 0 {
            Some(&self.threads.as_slice()[0])
        } else {
            None
        }
    }

    pub fn runnable_count(&self) -> usize {
        self.threads
            .as_slice()
            .iter()
            .filter(|t| t.is_runnable())
            .count()
    }

    pub fn all_terminated(&self) -> bool {
        self.threads
            .as_slice()
            .iter()
            .all(|t| t.state == ThreadState::Terminated)
    }
}
