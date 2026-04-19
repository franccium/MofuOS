//TODO: Priority-based, preemptive; implement TSC-Deadline timer first

use crate::process::{PID};
use crate::data_structures::dequeue::Dequeue;



pub struct Scheduler {
    ready_queue: Dequeue<PID>,
    waiting_queue: Dequeue<PID>,
    //TODO: priority queue
}

