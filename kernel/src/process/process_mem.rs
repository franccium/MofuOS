use crate::memory::memory::{MemoryMapFrameAllocator, PAGE_SIZE};
use crate::memory::usermem::UserMemoryManager;
use crate::process::process::{
    PROCESS_HEAP_SIZE_BYTES, PROCESS_HEAP_VIRT_END, PROCESS_HEAP_VIRT_START,
    PROCESS_USER_VADDR_ALLOC_END, PROCESS_USER_VADDR_ALLOC_MAX_SIZE,
    PROCESS_USER_VADDR_ALLOC_START,
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
    pub owned_regions: Vec<MappedMemoryRegion>,
    pub next_alloc_vaddr: VirtAddr,
}

#[derive(Debug, Clone)]
pub struct MappedMemoryRegion {
    pub start_virt: VirtAddr,
    pub size_bytes: u64,
    pub page_flags: PageTableFlags,
}

#[derive(Debug)]
pub enum ProcessMemoryError {
    OutOfVirtualRange,
    MapTo(MapToError<Size4KiB>),
}

impl From<MapToError<Size4KiB>> for ProcessMemoryError {
    fn from(error: MapToError<Size4KiB>) -> Self {
        Self::MapTo(error)
    }
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
            owned_regions: Vec::new(),
            stack_top: VirtAddr::new(0),
            stack_size: 0u64,
            heap_start: VirtAddr::new(PROCESS_HEAP_VIRT_START),
            heap_end: VirtAddr::new(PROCESS_HEAP_VIRT_START),
            next_alloc_vaddr: VirtAddr::new(PROCESS_USER_VADDR_ALLOC_START),
        })
    }

    pub fn allocate_heap(
        &mut self,
        size_bytes: usize,
        address_space_manager: MutexGuard<'_, UserMemoryManager>,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) -> Result<VirtAddr, ProcessMemoryError> {
        if size_bytes == 0 {
            return Ok(self.heap_end);
        }

        if size_bytes as u64 > PROCESS_HEAP_SIZE_BYTES {
            serial_println!(
                "ProcessMemoryLayout: allocate_heap: size {} exceeds heap capacity {}",
                size_bytes,
                PROCESS_HEAP_SIZE_BYTES
            );
            return Err(ProcessMemoryError::OutOfVirtualRange);
        }

        let aligned_size = align_to_page_size(size_bytes as u64);
        let old_heap_end = self.heap_end;
        let new_heap_end = old_heap_end + aligned_size;
        self.grow_heap(new_heap_end, address_space_manager, frame_allocator)?;
        Ok(old_heap_end)
    }

    pub fn grow_heap(
        &mut self,
        new_heap_end: VirtAddr,
        address_space_manager: MutexGuard<'_, UserMemoryManager>,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) -> Result<VirtAddr, ProcessMemoryError> {
        if new_heap_end <= self.heap_end {
            return Ok(self.heap_end);
        }

        if new_heap_end > VirtAddr::new(PROCESS_HEAP_VIRT_END) {
            serial_println!(
                "ProcessMemoryLayout: grow_heap: new_heap_end ({}) exceeds heap limit ({})",
                new_heap_end.as_u64(),
                PROCESS_HEAP_VIRT_END
            );
            return Err(ProcessMemoryError::OutOfVirtualRange);
        }

        let grow_by = align_to_page_size(new_heap_end - self.heap_end);
        if grow_by == 0 {
            return Ok(self.heap_end);
        }

        let protection_flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;

        address_space_manager.map_virt_mem_region(
            self.top_page_table_phys,
            self.heap_end,
            grow_by,
            protection_flags,
            frame_allocator,
        )?;
        self.owned_regions.push(MappedMemoryRegion {
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

        Ok(self.heap_end)
    }

    pub fn allocate_virtual_range(
        &mut self,
        size_bytes: usize,
        page_flags: PageTableFlags,
    ) -> Option<VirtAddr> {
        if size_bytes as u64 > PROCESS_USER_VADDR_ALLOC_MAX_SIZE {
            return None;
        }
        let aligned_size = align_to_page_size(size_bytes as u64);

        let alloc_vaddr = self.next_alloc_vaddr.as_u64();
        let range_end = alloc_vaddr + aligned_size;

        if range_end > PROCESS_USER_VADDR_ALLOC_END {
            serial_println_core!(
                "allocate_virtual_range: range_end is over the user vaddr alloc end: {}, trying to find a free gap",
                range_end
            );
            let gap_vaddr = self.find_free_gap(aligned_size as usize)?;
            self.owned_regions.push(MappedMemoryRegion {
                start_virt: gap_vaddr,
                size_bytes: aligned_size,
                page_flags,
            });
            return Some(gap_vaddr);
        }

        self.next_alloc_vaddr = VirtAddr::new(range_end);
        self.owned_regions.push(MappedMemoryRegion {
            start_virt: VirtAddr::new(alloc_vaddr),
            size_bytes: aligned_size,
            page_flags,
        });

        Some(VirtAddr::new(alloc_vaddr))
    }

    fn find_free_gap(&self, size_bytes: usize) -> Option<VirtAddr> {
        let aligned_size = align_to_page_size(size_bytes as u64);
        if aligned_size == 0 {
            return None;
        }

        let window_start = PROCESS_USER_VADDR_ALLOC_START;
        let window_end = PROCESS_USER_VADDR_ALLOC_END;

        let mut window_regions: Vec<(u64, u64)> = Vec::new();
        for region in self.owned_regions.iter() {
            if region.size_bytes == 0 {
                continue;
            }
            let region_start = region.start_virt.as_u64();
            let region_end = region_start.saturating_add(region.size_bytes);
            if region_end <= window_start || region_start >= window_end {
                continue;
            }
            let clipped_start = align_to_page_size(region_start.max(window_start));
            let clipped_end = align_to_page_size(region_end.min(window_end));
            if clipped_start < clipped_end {
                window_regions.push((clipped_start, clipped_end));
            }
        }
        window_regions.sort_unstable_by_key(|region| region.0);

        let mut cursor = window_start;
        for (region_start, region_end) in window_regions {
            if region_start >= cursor.saturating_add(aligned_size) {
                return Some(VirtAddr::new(cursor));
            }
            cursor = cursor.max(region_end);
        }

        if cursor.saturating_add(aligned_size) <= window_end {
            return Some(VirtAddr::new(cursor));
        }

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

        for region in self.owned_regions.iter() {
            let start = region.start_virt;
            let size = region.size_bytes;
            if size == 0 {
                continue;
            }

            let start_page =
                x86_64::structures::paging::Page::<Size4KiB>::containing_address(start);
            let end_page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(
                start + size - 1u64,
            );

            for page in x86_64::structures::paging::Page::range_inclusive(start_page, end_page) {
                if let Some(phys) =
                    user_mgr.translate_user_virt_to_phys(pml4_phys, page.start_address())
                {
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
