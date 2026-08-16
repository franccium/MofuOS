use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use x86_64::{
    PhysAddr, VirtAddr,
    structures::paging::{FrameAllocator, Page, PageTableFlags, PhysFrame, Size4KiB},
};

use crate::{memory::{
    memory::{MemoryMapFrameAllocator, PAGE_SIZE, align_up},
    usermem::UserMemoryManager,
}, process::process_mem::ProcessMemoryLayout};

pub struct CircularBuffer {
    data: VirtAddr,
    size: usize,
    curr_write_pos: usize,
    total_written_bytes: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CircularBufferCreateRequest {
    pub size_bytes: usize,
    pub page_flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CircularBufferInfo {
    pub virtual_base: u64,
    pub view_size: u64,
    pub total_virtual_size: u64,
}

impl CircularBuffer {
    pub fn map_for_user(
        size_bytes: usize,
        page_flags: PageTableFlags,
        user_memory_manager: &UserMemoryManager,
        proces_memory_layout: &mut ProcessMemoryLayout,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) -> Option<CircularBufferInfo> {
        let data_size = align_up(size_bytes as u64, PAGE_SIZE as u64) as usize;
        let total_virt_size = 2 * data_size;

        if let Some(virt_view_base) = proces_memory_layout.allocate_virtual_range(total_virt_size) {
            let virt_second_view_base = virt_view_base + data_size as u64;

            let page_table = proces_memory_layout.top_page_table_phys;

            let frame_count = data_size / PAGE_SIZE;
            for i in 0..frame_count {
                let frame = frame_allocator.allocate_frame()?;

                user_memory_manager.map_specific_frame(
                    page_table,
                    virt_view_base + (i * PAGE_SIZE) as u64,
                    frame.start_address(),
                    page_flags,
                    frame_allocator,
                ).ok()?;

                user_memory_manager.map_specific_frame(
                    page_table,
                    virt_second_view_base + (i * PAGE_SIZE) as u64,
                    frame.start_address(),
                    page_flags,
                    frame_allocator,
                ).ok()?;
            }

            return Some(CircularBufferInfo {
                virtual_base: virt_view_base.as_u64(),
                view_size: data_size as u64,
                total_virtual_size: total_virt_size as u64,
            })
        }
        None
    }
}
