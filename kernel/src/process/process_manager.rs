use crate::data_structures::vector::Vec;
use crate::process::process::ProcessResources;
//use alloc::vec::Vec;
use crate::process::core_pool::CORE_POOL;
use crate::process::elf_loader::ElfLoadError;
use crate::process::kernel_thread::{KernelThread, ThreadGroup, ThreadState};
use crate::process::process::{INVALID_PID, MAX_PRIORITY, Process, ProcessState};
use crate::process::process_mem::MappedMemoryRegion;
use crate::process::scheduler::SCHEDULER;
use crate::serial_println;
use alloc::string::String;
use spin::Mutex;

pub const ARCHE_PID: usize = 0;

lazy_static::lazy_static! {
    pub static ref PROCESS_MANAGER: Mutex<ProcessManager> = {
        let mut pm = ProcessManager::new();
        pm.init_arche();
        Mutex::new(pm)
    };
}

#[derive(Debug)]
pub enum ProcessError {
    ProcessNotFound,
    ParentNotFound,
    DoubleDelete,
    ElfLoadError(ElfLoadError),
    NoCoresAvailable,
    SchedulerError,
}

/// Manages processes and their associated kernel threads
/// Implements task parallelism using a one-to-one threading model
pub struct ProcessManager {
    processes: Vec<Process>,
    thread_groups: Vec<ThreadGroup>,
    new_pid: usize,
}

unsafe impl Send for ProcessManager {}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            processes: Vec::with_capacity(16),
            thread_groups: Vec::with_capacity(16),
            new_pid: 1,
        }
    }

    pub fn init_arche(&mut self) -> usize {
        let arche = Process::new(
            ARCHE_PID,
            ARCHE_PID,
            MAX_PRIORITY,
            String::from("arche"),
            true,
            ProcessResources {
                memory_limit: usize::MAX,
                memory_used: 0,
                cpu_time_slice: 0,
            },
            0,
            0,
            0,
        );
        serial_println!("Initialized arche process with PID 0");
        self.processes.push(arche.unwrap());
        0
    }

    pub fn create_process(
        &mut self,
        parent_pid: usize,
        priority: u8,
        name_ptr: *const u8,
        name_len: u8,
        is_out: bool,
        entry_point: u64,
        stack_top: u64,
        page_table_base: u64,
    ) -> Result<usize, ProcessError> {
        assert!(
            parent_pid != INVALID_PID,
            "Parent PID cannot be INVALID_PID"
        );

        let _parent = self
            .get_process(parent_pid)
            .map_err(|_| ProcessError::ParentNotFound)?;

        let new_pid = self.new_pid;
        self.new_pid += 1;

        // Extract process name
        let name_str = unsafe {
            let slice = core::slice::from_raw_parts(name_ptr, name_len as usize);
            String::from_utf8_lossy(slice).into_owned()
        };

        // Allocate a CPU core for the new process from the core pool
        let mut core_pool = CORE_POOL.lock();
        let core_id = core_pool
            .allocate_core(new_pid)
            .ok_or(ProcessError::NoCoresAvailable)?;
        drop(core_pool);

        // Create kernel thread for this process (one-to-one model)
        let mut kernel_thread = KernelThread::new(
            new_pid,
            priority,
            String::from(&name_str),
            entry_point,
            stack_top,
            page_table_base,
        );
        kernel_thread.assign_to_core(core_id);

        // Create thread group
        let thread_group = ThreadGroup::new(new_pid, kernel_thread);

        // Create process structure
        let new_process = Process {
            pid: new_pid,
            parent_pid,
            priority,
            state: ProcessState::Ready,
            name: String::from(&name_str),
            children: Vec::new(),
            file_descriptors: Vec::new(),
            resources: crate::process::process::ProcessResources::default(),
            exit_code: None,
            is_out,
            execution_context: crate::process::process::ExecutionContext::new(
                entry_point,
                stack_top,
                page_table_base,
            ),
            memory_layout: crate::process::process_mem::ProcessMemoryLayout {
                top_page_table_phys: x86_64::PhysAddr::new(page_table_base),
                stack_top: x86_64::VirtAddr::new(stack_top),
                stack_size: 0,
                heap_start: x86_64::VirtAddr::new(0),
                heap_end: x86_64::VirtAddr::new(0),
                mapped_regions: alloc::vec::Vec::<MappedMemoryRegion>::new(),
            },
        };

        // Store process and thread group BEFORE making the PID visible to the
        // scheduler — same ordering guarantee as create_process_from_elf.
        self.processes.push(new_process);
        self.thread_groups.push(thread_group);

        // Enqueue thread in scheduler for its assigned core only after the
        // process is fully stored and visible via get_process().
        {
            let mut scheduler = SCHEDULER.lock();
            scheduler.enqueue_on_core(core_id, new_pid, priority);
        }

        serial_println!(
            "Created process {} (PID {}) on core {} with priority {}",
            name_str,
            new_pid,
            core_id,
            priority
        );

        Ok(new_pid)
    }

    /// Create a process from ELF data, integrate with scheduler and core pool
    /// This is the main API for creating user processes from ELF binaries
    pub fn create_process_from_elf(
        &mut self,
        parent_pid: usize,
        elf_info: &crate::process::ElfLoadInfo,
        name: &str,
        priority: u8,
    ) -> Result<usize, ProcessError> {
        // Verify parent exists
        serial_println!("create_process_from_elf");

        let _parent = self
            .get_process(parent_pid)
            .map_err(|_| ProcessError::ParentNotFound)?;

        let new_pid = self.new_pid;
        self.new_pid += 1;

        serial_println!("create_process_from_elf: assigned pid: {}", new_pid);

        // Allocate a CPU core for this process
        let mut core_pool = CORE_POOL.lock();
        let core_id = core_pool
            .allocate_core(new_pid)
            .ok_or(ProcessError::NoCoresAvailable)?;
        drop(core_pool);

        serial_println!("create_process_from_elf: assigned Core ID: {}", core_id);

        // Create process structure with ELF-loaded memory
        let mut process = Process::create_with_elf(elf_info, name, new_pid, parent_pid)
            .map_err(|_| ProcessError::SchedulerError)?;

        // Set priority and state
        process.priority = priority;
        process.state = ProcessState::Ready;

        // Create kernel thread for this process (one-to-one model)
        let mut kernel_thread = KernelThread::new(
            new_pid,
            priority,
            String::from(name),
            elf_info.entry_point,
            process.execution_context.rsp,
            process.memory_layout.top_page_table_phys.as_u64(),
        );
        kernel_thread.assign_to_core(core_id);

        serial_println!(
            "create_process_from_elf: assigned kernel thread to Core ID: {} for process name: {}, pid: {}",
            core_id,
            kernel_thread.name,
            kernel_thread.pid
        );

        // Create thread group
        let thread_group = ThreadGroup::new(new_pid, kernel_thread);

        // Store process and thread group BEFORE enqueuing to the scheduler.
        // Core 1's scheduler loop can pick up the PID the instant it appears in
        // the scheduler queue and will immediately call get_process(pid).  If we
        // enqueue first (old order), get_process returns Err(ProcessNotFound)
        // because processes.push hasn't run yet, causing a race that can corrupt
        // memory when core 1 jumps to userspace while core 0 is still setting up
        // the same process's page tables.
        self.processes.push(process);
        self.thread_groups.push(thread_group);

        // Only now make the process visible to the scheduler — it is fully
        // constructed and stored at this point.
        {
            let mut scheduler = SCHEDULER.lock();
            scheduler.enqueue_on_core(core_id, new_pid, priority);
        }

        serial_println!(
            "create_process_from_elf: enqueued process PID {} on core {}; priority: {}",
            new_pid,
            core_id,
            priority
        );

        serial_println!(
            "Created userspace process {} (PID {}) from ELF on core {} with priority {}",
            name,
            new_pid,
            core_id,
            priority
        );

        Ok(new_pid)
    }

    pub fn terminate_process(
        &mut self,
        pid: usize,
        exit_code: i32,
        cascade: bool,
    ) -> Result<(), ProcessError> {
        if pid == INVALID_PID {
            return Err(ProcessError::DoubleDelete);
        }

        let process = self.get_process_mut(pid)?;
        let children = process.children.clone();
        process.state = ProcessState::Terminated;
        process.exit_code = Some(exit_code);

        // Get the core assignment for cleanup
        let core_id = self.get_process_core(pid);

        // Terminate thread and release core
        if let Some(thread_group) = self.thread_groups.iter_mut().find(|tg| tg.pid == pid) {
            if let Some(thread) = thread_group.main_thread_mut() {
                thread.terminate(exit_code);
                if let Some(core) = thread.core_id {
                    let mut core_pool = CORE_POOL.lock();
                    core_pool.release_core(core);
                    serial_println!("Released core {} for terminated process {}", core, pid);
                }
            }
        }

        // Handle children
        if cascade {
            for child_pid in children.iter().copied() {
                let _ = self.terminate_process(child_pid, exit_code, true);
            }
        } else {
            for child_pid in children.iter().copied() {
                if let Ok(child) = self.get_process_mut(child_pid) {
                    child.parent_pid = ARCHE_PID;
                }

                if let Ok(arche) = self.get_process_mut(ARCHE_PID) {
                    arche.children.push(child_pid);
                }
            }
        }

        Ok(())
    }

    /// Clean up dead processes (garbage collection)
    pub fn cleanup_dead(&mut self) {
        self.processes
            .retain(|p| p.state != ProcessState::Terminated);
        self.thread_groups.retain(|tg| !tg.all_terminated());
    }

    pub fn get_process(&self, pid: usize) -> Result<&Process, ProcessError> {
        assert!(pid != INVALID_PID, "get_process: PID cannot be INVALID_PID");
        self.processes
            .iter()
            .find(|p| p.pid == pid)
            .ok_or(ProcessError::ProcessNotFound)
    }

    pub fn get_process_mut(&mut self, pid: usize) -> Result<&mut Process, ProcessError> {
        assert!(
            pid != INVALID_PID,
            "get_process_mut: PID cannot be INVALID_PID"
        );
        self.processes
            .iter_mut()
            .find(|p| p.pid == pid)
            .ok_or(ProcessError::ProcessNotFound)
    }

    /// Get the CPU core ID assigned to a process
    pub fn get_process_core(&self, pid: usize) -> Option<u8> {
        self.thread_groups
            .iter()
            .find(|tg| tg.pid == pid)
            .and_then(|tg| tg.main_thread())
            .and_then(|thread| thread.core_id)
    }

    /// Get reference to a kernel thread
    pub fn get_kernel_thread(&self, pid: usize) -> Option<&KernelThread> {
        self.thread_groups
            .iter()
            .find(|tg| tg.pid == pid)
            .and_then(|tg| tg.main_thread())
    }

    /// Get mutable reference to a kernel thread
    pub fn get_kernel_thread_mut(&mut self, pid: usize) -> Option<&mut KernelThread> {
        self.thread_groups
            .iter_mut()
            .find(|tg| tg.pid == pid)
            .and_then(|tg| tg.main_thread_mut())
    }

    pub fn get_process_list(&self) -> &Vec<Process> {
        &self.processes
    }

    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    pub fn thread_count(&self) -> usize {
        self.thread_groups.len()
    }

    pub fn processes_on_core(&self, core_id: u8) -> Vec<usize> {
        self.thread_groups
            .iter()
            .filter(|tg| {
                tg.main_thread()
                    .map(|t| t.core_id == Some(core_id))
                    .unwrap_or(false)
            })
            .map(|tg| tg.pid)
            .collect()
    }

    pub fn get_parallelism_stats(&self) -> ParallelismStats {
        let active_cores = self
            .thread_groups
            .iter()
            .filter_map(|tg| tg.main_thread().and_then(|t| t.core_id))
            .collect::<alloc::collections::BTreeSet<_>>()
            .len();

        let mut running_count = 0;
        let mut ready_count = 0;
        let mut blocked_count = 0;

        for tg in self.thread_groups.iter() {
            if let Some(thread) = tg.main_thread() {
                match thread.state {
                    ThreadState::Running => running_count += 1,
                    ThreadState::Ready => ready_count += 1,
                    ThreadState::Blocked => blocked_count += 1,
                    ThreadState::Terminated => {}
                }
            }
        }

        ParallelismStats {
            total_processes: self.processes.len(),
            active_cores,
            running_threads: running_count,
            ready_threads: ready_count,
            blocked_threads: blocked_count,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ParallelismStats {
    pub total_processes: usize,
    pub active_cores: usize,
    pub running_threads: usize,
    pub ready_threads: usize,
    pub blocked_threads: usize,
}
