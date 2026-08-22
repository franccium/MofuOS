use crate::memory::memory::{MemoryMapFrameAllocator, PAGE_SIZE};
use crate::memory::usermem::UserMemoryManager;
use crate::process::process::{
    PROCESS_HEAP_VIRT_END, PROCESS_HEAP_VIRT_START, PROCESS_USER_VADDR_ALLOC_END, PROCESS_USER_VADDR_ALLOC_START,
};
use crate::{serial_println, serial_println_core};
use alloc::vec::Vec;
use spin::MutexGuard;
use x86_64::structures::paging::Size4KiB;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::{PhysAddr, VirtAddr, structures::paging::PageTableFlags};

#[derive(Debug, Clone)]
pub struct ProcessMemoryLayout {
    pub top_page_table_phys: PhysAddr,
    pub stack_top: VirtAddr,
    pub stack_size: u64,
    pub heap_start: VirtAddr,
    pub heap_end: VirtAddr,
    pub mapped_regions: Vec<MappedMemoryRegion>,
    pub next_alloc_vaddr: VirtAddr,
    pub allocated_ranges: Vec<(u64, u64)>, // (start, size)
}

#[derive(Debug, Clone)]
pub struct MappedMemoryRegion {
    pub start_virt: VirtAddr,
    pub size_bytes: u64,
    pub page_flags: PageTableFlags,
}

const fn align_to_page_size(size: u64) -> u64 {
    ((size + PAGE_SIZE as u64 - 1) / PAGE_SIZE as u64) * PAGE_SIZE as u64
}

impl ProcessMemoryLayout {
    pub fn new(
        address_space_manager: &UserMemoryManager,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) -> Result<Self, MapToError<Size4KiB>> {
        let top_page_table_phys =
            address_space_manager.allocate_new_address_space(frame_allocator)?;

        Ok(Self {
            top_page_table_phys,
            mapped_regions: Vec::new(),
            stack_top: VirtAddr::new(0),
            stack_size: 0u64,
            heap_start: VirtAddr::new(PROCESS_HEAP_VIRT_START),
            heap_end: VirtAddr::new(PROCESS_HEAP_VIRT_END),
            next_alloc_vaddr: VirtAddr::new(PROCESS_USER_VADDR_ALLOC_START),
            allocated_ranges: Vec::new(),
        })
    }

    pub fn grow_heap(
        &mut self,
        new_heap_end: VirtAddr,
        address_space_manager: MutexGuard<'_, UserMemoryManager>,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) -> Result<VirtAddr, MapToError<Size4KiB>> {
        if new_heap_end < self.heap_start {
            serial_println!(
                "ProcessMemoryLayout: grow_heap: new_heap_end ({}) < current heap_start ({})",
                new_heap_end.as_u64(),
                self.heap_start.as_u64()
            );
            return Ok(self.heap_end);
        }

        if new_heap_end > self.heap_end {
            let grow_by = align_to_page_size(new_heap_end - self.heap_end);
            if grow_by > 0 {
                let protection_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

                address_space_manager.map_virt_mem_region(
                    self.top_page_table_phys,
                    self.heap_end,
                    grow_by as u64,
                    protection_flags,
                    frame_allocator,
                )?;
                self.mapped_regions.push(MappedMemoryRegion {
                    start_virt: self.heap_end,
                    size_bytes: grow_by,
                    page_flags: protection_flags,
                });
                self.heap_end += grow_by;

                serial_println!(
                    "ProcessMemoryLayout: grow_heap: Grew heap by {} bytes (new heap_end: {})",
                    grow_by,
                    self.heap_end.as_u64()
                );
            }
        } else if new_heap_end < self.heap_end {
            let shrink_by = self.heap_end - new_heap_end;
            self.heap_end = new_heap_end;

            serial_println!(
                "ProcessMemoryLayout: grow_heap: Shrunk heap by {} bytes (new heap_end: {})",
                shrink_by,
                self.heap_end.as_u64()
            );
        }

        Ok(self.heap_end)
    }

    pub fn allocate_virtual_range(&mut self, size_bytes: usize) -> Option<VirtAddr> {
        let aligned_size = align_to_page_size(size_bytes as u64);

        let alloc_vaddr = self.next_alloc_vaddr.as_u64();
        let range_end = alloc_vaddr + aligned_size;

        if range_end > PROCESS_USER_VADDR_ALLOC_END {
            serial_println_core!(
                "allocate_virtual_range: range_end is over the user vaddr alloc end: {}, trying to find a free gap",
                range_end
            );
            return self.find_free_gap(aligned_size as usize);
        }

        self.next_alloc_vaddr = VirtAddr::new(range_end);
        self.allocated_ranges.push((alloc_vaddr, aligned_size));

        Some(VirtAddr::new(alloc_vaddr))
    }

    fn find_free_gap(&self, size_bytes: usize) -> Option<VirtAddr> {
        //TODO:
        None
    }

    pub fn free_address_space(
        &self,
        user_mgr: &UserMemoryManager,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) {
        if self.top_page_table_phys.as_u64() == 0 {
            return;
        }
        let pml4_phys = self.top_page_table_phys;
        let mut owned_phys: Vec<PhysAddr> = Vec::new();

        let mut push_owned = |phys: PhysAddr| {
            for &existing in owned_phys.iter() {
                if existing == phys {
                    return;
                }
            }
            owned_phys.push(phys);
        };

        for region in self.mapped_regions.iter() {
            let start = region.start_virt;
            let size = region.size_bytes;
            if size == 0 {
                continue;
            }
            let start_page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(start);
            let end_page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(
                start + size - 1u64,
            );
            for page in x86_64::structures::paging::Page::range_inclusive(start_page, end_page) {
                if let Some(phys) = user_mgr.translate_user_virt_to_phys(pml4_phys, page.start_address()) {
                    push_owned(phys);
                }
            }
        }

        if self.stack_size > 0 {
            let stack_bottom = self.stack_top - self.stack_size;
            let start_page =
                x86_64::structures::paging::Page::<Size4KiB>::containing_address(stack_bottom);
            let end_page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(
                self.stack_top - 1u64,
            );
            for page in x86_64::structures::paging::Page::range_inclusive(start_page, end_page) {
                if let Some(phys) = user_mgr.translate_user_virt_to_phys(pml4_phys, page.start_address()) {
                    push_owned(phys);
                }
            }
        }

        for &(start_u64, size_u64) in self.allocated_ranges.iter() {
            if size_u64 == 0 {
                continue;
            }
            let start = VirtAddr::new(start_u64);
            let start_page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(start);
            let end_page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(
                start + size_u64 - 1u64,
            );
            for page in x86_64::structures::paging::Page::range_inclusive(start_page, end_page) {
                if let Some(phys) = user_mgr.translate_user_virt_to_phys(pml4_phys, page.start_address()) {
                    push_owned(phys);
                }
            }
        }

        serial_println!(
            "free_address_space: pml4 {:#x} owned_frames {}",
            pml4_phys.as_u64(),
            owned_phys.len()
        );

        user_mgr.unmap_all_user_pages_with_owned_set(pml4_phys, &owned_phys, frame_allocator);
        user_mgr.reclaim_empty_user_tables(pml4_phys, frame_allocator);
        user_mgr.free_pml4_frame(pml4_phys, frame_allocator);

        serial_println!(
            "free_address_space: freed pml4 {:#x} free_list now {}",
            pml4_phys.as_u64(),
            frame_allocator.free_frame_count()
        );
    }
}
