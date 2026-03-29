use crate::{data_structures::vector::Vec};
use alloc::string::String;

// Marks terminated children
pub const INVALID_PID: usize = usize::MAX;
pub const MAX_PRIORITY: u8 = u8::MAX;

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

//TODO: when we have a fs/vfs
pub struct FileDescriptor {
    pub handle: usize,
}

pub struct Process {
    pub pid: usize,
    pub parent_pid: usize,
    pub priority: u8,
    pub state: ProcessState,
    pub name: String,
    pub children: Vec<usize>,
    pub file_descriptors: Vec<FileDescriptor>,

    pub resources: ProcessResources,
    pub exit_code: Option<i32>,
    pub is_out: bool,
}

unsafe impl Send for Process {}

impl Process {
    pub fn new(
        pid: usize,
        parent_pid: usize,
        priority: u8,
        name: String,
        is_out: bool,
        resources: ProcessResources,
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
        }
    }
}
