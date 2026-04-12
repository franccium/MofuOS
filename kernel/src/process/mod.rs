pub mod process;
pub mod process_manager;
pub mod syscall;
pub mod scheduler;
pub mod elf_loader;
pub mod process_mem;
pub mod execution;

pub use process::{FileDescriptor, Process, PID, ProcessResources, ProcessState, ExecutionContext};
pub use process_manager::ProcessManager;
pub use syscall::{SystemCall};
pub use elf_loader::{ElfLoadInfo, ElfLoadError};
