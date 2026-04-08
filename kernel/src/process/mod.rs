pub mod process;
pub mod process_manager;
pub mod syscall;
pub mod scheduler;

pub use process::{FileDescriptor, Process, PID, ProcessResources, ProcessState, ExecutionContext};
pub use process_manager::ProcessManager;
pub use syscall::{SystemCall};
