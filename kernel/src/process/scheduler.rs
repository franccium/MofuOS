//TODO: Priority-based, preemptive; implement TSC-Deadline timer first

use alloc::string::String;
use alloc::vec::Vec;
use spin::MutexGuard;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::process::{Process, ProcessState, ProcessResources, ExecutionContext, PID};
use crate::data_structures::dequeue::Dequeue;



pub struct Scheduler {
    ready_queue: Dequeue<PID>,
    waiting_queue: Dequeue<PID>,
    //TODO: priority queue
}

