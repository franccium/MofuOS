use crate::{data_structures::vector::Vec};
use alloc::string::String;
use x86_64::VirtAddr;

// Marks terminated children
pub const INVALID_PID: usize = usize::MAX;
pub const MAX_PRIORITY: u8 = 8;
pub const RFLAGS_DEFAULT: u64 = 0x202;

pub type PID = usize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProcessState {
    Ready,
    Running,
    Waiting,
    Terminated,
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessResources {
    pub memory_limit: usize,
    pub memory_used: usize,
    pub cpu_time_slice: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutionContext {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,

    //TODO: sse? maybe somewhere seperate because aint storing the whole avx
    
    pub rip: u64,
    pub rflags: u64,
    
    pub page_table_base_phys: u64,
}


//TODO: when we have a fs/vfs
pub struct FileDescriptor {
    pub handle: usize,
}

pub struct Process {
    pub pid: PID,
    pub parent_pid: PID,
    pub priority: u8,
    pub state: ProcessState,
    pub name: String,
    pub children: Vec<PID>,
    pub file_descriptors: Vec<FileDescriptor>,
    
    pub resources: ProcessResources,
    pub exit_code: Option<i32>,
    pub is_out: bool,
    
    pub execution_context: ExecutionContext,
}

unsafe impl Send for Process {}

impl ExecutionContext {
    pub fn new(entry_point: u64, stack_top: u64, page_table_base_phys: u64) -> Self {
        Self {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: stack_top,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: entry_point,
            rflags: RFLAGS_DEFAULT,
            page_table_base_phys,
        }
    }
}

impl Process {
    pub fn new(
        pid: usize,
        parent_pid: usize,
        priority: u8,
        name: String,
        is_out: bool,
        resources: ProcessResources,
        entry_point: u64,
        stack_top: u64,
        page_table_base_phys: u64,
    ) -> Self {
        Self {
            pid,
            parent_pid,
            priority,
            state: ProcessState::Ready,
            name,
            children: Vec::new(),
            file_descriptors: Vec::new(),
            resources,
            exit_code: None,
            is_out,
            execution_context: ExecutionContext::new(entry_point, stack_top, page_table_base_phys),
        }
    }
}
