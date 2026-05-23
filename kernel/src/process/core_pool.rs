use crate::data_structures::vector::Vec;
use crate::serial_println;
use spin::Mutex;
use crate::process::process::{PID, INVALID_PID};
use limine::mp::{MpInfo, MpGotoFunction};

lazy_static::lazy_static! {
    pub static ref CORE_POOL: Mutex<CorePool> = Mutex::new(CorePool::new());
}

pub const MAX_CORES: u8 = 64; // Limit to 64 cores for the availability bitmap
pub const CORE_IS_AVAILABLE: u64 = 1;
pub const CORE_IS_USED: u64 = 0;

pub struct CorePool {
    total_cores: u8,
    available_cores: u64,
    /// Map from core ID to assigned thread/PID (for tracking)
    core_assignments: [usize; MAX_CORES as usize],
    lapic_ids: [u8; MAX_CORES as usize],
}

impl CorePool {
    pub fn new() -> Self {
        Self {
            total_cores: 1,
            available_cores: 0b1, // Core 0 available by default
            core_assignments: [INVALID_PID; MAX_CORES as usize],
            lapic_ids: [0; MAX_CORES as usize],
        }
    }

    /// Called once during kernel initialization
    pub fn init_with_core_count(&mut self, core_count: u8, cpus: &[&MpInfo]) {
        if core_count == 0 {
            serial_println!("ERROR: Invalid core count: {}, using 1", core_count);
            return;
        }
        if core_count > MAX_CORES {
            serial_println!(
                "ERROR: Core count {} exceeds max supported {}, using {}",
                core_count,
                MAX_CORES,
                MAX_CORES
            );
            self.total_cores = MAX_CORES;
        }

        self.total_cores = core_count;
        self.available_cores = (1u64 << self.total_cores) - 1;

        // Core 0 is reserved for boot/kernel
        self.available_cores &= !1;

        for i in 0..core_count {
             self.core_assignments[i as usize] = INVALID_PID;
             self.lapic_ids[i as usize] = cpus[i as usize].lapic_id as u8;
        }

        serial_println!("CorePool initialized with {} cores", core_count);
    }

    pub fn total_cores(&self) -> u8 {
        self.total_cores
    }

    pub fn available_count(&self) -> u8 {
        self.available_cores.count_ones() as u8
    }

    pub fn is_available(&self, core_id: u8) -> bool {
        if core_id >= self.total_cores {
            return false;
        }
        (self.available_cores & (1u64 << core_id)) != 0
    }

    pub fn mark_available(&mut self, core_id: u8) {
        if core_id < self.total_cores {
            (self.available_cores |= (1u64 << core_id))
        }
    }

    pub fn allocate_core(&mut self, pid: usize) -> Option<u8> {
        for core_id in 0..self.total_cores {
            if self.is_available(core_id) {
                self.available_cores &= !(1u64 << core_id);
                let idx = core_id as usize;
                if idx < self.core_assignments.len() {
                    self.core_assignments[idx] = pid;
                }
                serial_println!("Allocated core {} to PID {}", core_id, pid);
                return Some(core_id);
            }
        }
        serial_println!("No available cores for PID {}", pid);
        None
    }

    pub fn try_allocate_given_core(&mut self, core_id: u8, pid: usize) -> bool {
        if !self.is_available(core_id) {
            return false;
        }
        self.available_cores &= !(1u64 << core_id);
        let idx = core_id as usize;
        if idx < self.core_assignments.len() {
            self.core_assignments[idx] = pid;
        }
        serial_println!("Allocated core {} to PID {}", core_id, pid);
        true
    }

    pub fn force_allocate_given_core(&mut self, core_id: u8, pid: usize) {
        self.available_cores &= !(1u64 << core_id);
        let idx = core_id as usize;
        if idx < self.core_assignments.len() {
            self.core_assignments[idx] = pid;
        }
        serial_println!("Force Allocated core {} to PID {}", core_id, pid);
    }

    pub fn release_core(&mut self, core_id: u8) {
        if core_id < self.total_cores {
            self.available_cores |= 1u64 << core_id;
            let idx = core_id as usize;
            if idx < self.core_assignments.len() {
                serial_println!("Released core {}", core_id);
                self.core_assignments[idx] = INVALID_PID;
            }
        }
    }

    pub fn get_core_assignment(&self, core_id: u8) -> usize {
        if core_id < self.total_cores {
            let idx = core_id as usize;
            self.core_assignments[idx]
        } else {
            INVALID_PID
        }
    }

    pub fn available_cores_list(&self) -> Vec<u8> {
        let mut available = Vec::new();
        for core_id in 0..self.total_cores {
            if self.is_available(core_id) {
                available.push(core_id);
            }
        }
        available
    }

    pub fn all_cores_used(&self) -> bool {
        self.available_cores == 0
    }

    pub fn get_stats(&self) -> CorePoolStats {
        CorePoolStats {
            total_cores: self.total_cores,
            available_cores: self.available_count(),
            used_cores: self.total_cores - self.available_count(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CorePoolStats {
    pub total_cores: u8,
    pub available_cores: u8,
    pub used_cores: u8,
}

impl core::fmt::Display for CorePoolStats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Cores: {}/{} available ({} in use)",
            self.available_cores, self.total_cores, self.used_cores
        )
    }
}
