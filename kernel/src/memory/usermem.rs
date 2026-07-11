use crate::{HHDM_OFFSET, memory::memory::MemoryMapFrameAllocator, serial_println};
use x86_64::{
    PhysAddr, VirtAddr,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, Size4KiB,
        mapper::MapToError,
    },
};

const LEVEL_4_KERNEL_ENTRIES_START: usize = 256;
const LEVEL_4_KERNEL_ENTRIES_END: usize = 512;
pub const USER_STACK_TOP: u64 = 0x7FFF_FFFF_F000;
pub const USER_MEM_MAX_ADDRESS: usize = 0x0000_8000_0000_0000;

pub struct UserMemoryManager {
    pub kernel_page_table_phys: PhysAddr,
    pub phys_offset: u64,
}

impl UserMemoryManager {
    pub fn new(kernel_page_table_phys: PhysAddr, phys_offset: u64) -> Self {
        Self {
            kernel_page_table_phys,
            phys_offset,
        }
    }

    pub fn allocate_new_address_space(
        &self,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) -> Result<PhysAddr, MapToError<Size4KiB>> {
        let new_table_frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let new_table_pml4_phys = new_table_frame.start_address();
        let new_table_pml4_virt = VirtAddr::new(new_table_pml4_phys.as_u64() + self.phys_offset);

        let pml4_table = unsafe { &mut *(new_table_pml4_virt.as_u64() as *mut PageTable) };
        pml4_table.zero();

        let kernel_new_table_pml4_virt =
            VirtAddr::new(self.kernel_page_table_phys.as_u64() + self.phys_offset);
        let kernel_pml4_table =
            unsafe { &*(kernel_new_table_pml4_virt.as_u64() as *const PageTable) };

        for kernel_entry_idx in LEVEL_4_KERNEL_ENTRIES_START..LEVEL_4_KERNEL_ENTRIES_END {
            pml4_table[kernel_entry_idx] = kernel_pml4_table[kernel_entry_idx].clone();
        }

        // After mapping LAPIC, verify it's in the kernel PML4:
        let pml4_virt = VirtAddr::new(self.kernel_page_table_phys.as_u64() + HHDM_OFFSET);
        let pml4 = unsafe { &*(pml4_virt.as_u64() as *const PageTable) };
        let lapic_pml4_idx = ((0xFFFF_FFFF_FF80_0000u64 >> 39) & 0x1FF) as usize;
        serial_println!(
            "LAPIC PML4 entry {}: {:?}",
            lapic_pml4_idx,
            pml4[lapic_pml4_idx].flags()
        );

        serial_println!(
            "allocate_new_address_space: Created user address space: top-level table at {:?}",
            new_table_pml4_phys
        );

        Ok(new_table_pml4_phys)
    }

    pub fn translate_user_virt_to_phys(
        &self,
        user_page_table_phys: PhysAddr,
        user_vaddr: VirtAddr,
    ) -> Option<PhysAddr> {
        let pml4_virt = VirtAddr::new(user_page_table_phys.as_u64() + self.phys_offset);
        let pml4 = unsafe { &*(pml4_virt.as_u64() as *const PageTable) };

        let pml4_idx = ((user_vaddr.as_u64() >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((user_vaddr.as_u64() >> 30) & 0x1FF) as usize;
        let pd_idx = ((user_vaddr.as_u64() >> 21) & 0x1FF) as usize;
        let pt_idx = ((user_vaddr.as_u64() >> 12) & 0x1FF) as usize;
        let page_offset = user_vaddr.as_u64() & 0xFFF;

        let pml4_entry = &pml4[pml4_idx];
        if !pml4_entry.flags().contains(PageTableFlags::PRESENT) {
            serial_println!("translate_user_virt_to_phys: PML4 entry not present");
            return None;
        }

        let pdpt_phys = PhysAddr::new(pml4_entry.addr().as_u64());
        let pdpt_virt = VirtAddr::new(pdpt_phys.as_u64() + self.phys_offset);
        let pdpt = unsafe { &*(pdpt_virt.as_u64() as *const PageTable) };

        let pdpt_entry = &pdpt[pdpt_idx];
        if !pdpt_entry.flags().contains(PageTableFlags::PRESENT) {
            serial_println!("translate_user_virt_to_phys: PDPT entry not present");
            return None;
        }

        let pd_phys = PhysAddr::new(pdpt_entry.addr().as_u64());
        let pd_virt = VirtAddr::new(pd_phys.as_u64() + self.phys_offset);
        let pd = unsafe { &*(pd_virt.as_u64() as *const PageTable) };

        let pd_entry = &pd[pd_idx];
        if !pd_entry.flags().contains(PageTableFlags::PRESENT) {
            serial_println!("translate_user_virt_to_phys: PD entry not present");
            return None;
        }

        if pd_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            let phys_addr =
                PhysAddr::new(pd_entry.addr().as_u64() + (user_vaddr.as_u64() & 0x1FFFFF));
            return Some(phys_addr);
        }

        let pt_phys = PhysAddr::new(pd_entry.addr().as_u64());
        let pt_virt = VirtAddr::new(pt_phys.as_u64() + self.phys_offset);
        let pt = unsafe { &*(pt_virt.as_u64() as *const PageTable) };

        let pt_entry = &pt[pt_idx];
        if !pt_entry.flags().contains(PageTableFlags::PRESENT) {
            serial_println!("translate_user_virt_to_phys: PT entry not present");
            return None;
        }

        Some(PhysAddr::new(pt_entry.addr().as_u64() + page_offset))
    }

    /// Read the PageTableFlags of an already-mapped page by walking the page table manually
    /// Returns empty flags if the page is not mapped
    fn get_page_flags(&self, pml4_table_phys: PhysAddr, page: Page<Size4KiB>) -> PageTableFlags {
        let vaddr = page.start_address();
        let pml4_virt = VirtAddr::new(pml4_table_phys.as_u64() + self.phys_offset);
        let pml4 = unsafe { &*(pml4_virt.as_u64() as *const PageTable) };

        let pml4_entry = &pml4[((vaddr.as_u64() >> 39) & 0x1FF) as usize];
        if !pml4_entry.flags().contains(PageTableFlags::PRESENT) {
            return PageTableFlags::empty();
        }
        let pdpt = unsafe {
            &*(VirtAddr::new(pml4_entry.addr().as_u64() + self.phys_offset).as_u64()
                as *const PageTable)
        };

        let pdpt_entry = &pdpt[((vaddr.as_u64() >> 30) & 0x1FF) as usize];
        if !pdpt_entry.flags().contains(PageTableFlags::PRESENT) {
            return PageTableFlags::empty();
        }
        let pd = unsafe {
            &*(VirtAddr::new(pdpt_entry.addr().as_u64() + self.phys_offset).as_u64()
                as *const PageTable)
        };

        let pd_entry = &pd[((vaddr.as_u64() >> 21) & 0x1FF) as usize];
        if !pd_entry.flags().contains(PageTableFlags::PRESENT) {
            return PageTableFlags::empty();
        }
        let pt = unsafe {
            &*(VirtAddr::new(pd_entry.addr().as_u64() + self.phys_offset).as_u64()
                as *const PageTable)
        };

        let pt_entry = &pt[((vaddr.as_u64() >> 12) & 0x1FF) as usize];
        pt_entry.flags()
    }

    pub fn map_virt_mem_region(
        &self,
        pml4_table_phys: PhysAddr,
        virt_addr: VirtAddr,
        size_bytes: u64,
        protection_flags: PageTableFlags,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) -> Result<(), MapToError<Size4KiB>> {
        serial_println!(
            "UserMemoryManager: map_virt_mem_region: mapping vaddr: {:#x}, bytes: {}",
            virt_addr.as_u64(),
            size_bytes
        );
        let new_table_pml4_virt = VirtAddr::new(pml4_table_phys.as_u64() + self.phys_offset);
        let pml4_table = unsafe { &mut *(new_table_pml4_virt.as_u64() as *mut PageTable) };

        let mut user_page_mapper =
            unsafe { OffsetPageTable::new(pml4_table, VirtAddr::new(self.phys_offset)) };

        let user_flags = protection_flags | PageTableFlags::USER_ACCESSIBLE;

        let start_page = Page::containing_address(virt_addr);
        let end_page = Page::containing_address(virt_addr + size_bytes - 1u64);
        for page in Page::range_inclusive(start_page, end_page) {
            let phys_frame = frame_allocator
                .allocate_frame()
                .ok_or(MapToError::FrameAllocationFailed)?;
            unsafe {
                match user_page_mapper.map_to(page, phys_frame, user_flags, frame_allocator) {
                    Ok(flush) => {
                        flush.flush();
                    }
                    Err(MapToError::PageAlreadyMapped(_existing_frame)) => {
                        // This page was already mapped by a previous segment whose
                        // virtual range overlaps ours at a page boundary.
                        //
                        // Capability flags (PRESENT, WRITABLE, USER_ACCESSIBLE) are
                        // unioned: grant the permission if either segment needs it.
                        //
                        // NO_EXECUTE is a restriction flag — it must only be set
                        // when ALL segments that touch this page agree it is
                        // non-executable.  So we AND it: if the existing mapping
                        // does not have NO_EXECUTE (page is executable), the merged
                        // result must also not have NO_EXECUTE, regardless of what
                        // the new segment requests.
                        let existing_flags = self.get_page_flags(pml4_table_phys, page);

                        // OR all bits together first, then fix up NO_EXECUTE.
                        let mut merged_flags = existing_flags | user_flags;

                        // Only keep NO_EXECUTE if BOTH sides had it set.
                        let both_nx = existing_flags.contains(PageTableFlags::NO_EXECUTE)
                            && user_flags.contains(PageTableFlags::NO_EXECUTE);
                        if !both_nx {
                            merged_flags.remove(PageTableFlags::NO_EXECUTE);
                        }

                        match user_page_mapper.update_flags(page, merged_flags) {
                            Ok(flush) => {
                                flush.flush();
                                serial_println!(
                                    "  map_virt_mem_region: page {:#x} already mapped, merged flags {:?} | {:?} -> {:?}",
                                    page.start_address().as_u64(),
                                    existing_flags,
                                    user_flags,
                                    merged_flags,
                                );
                            }
                            Err(e) => {
                                serial_println!(
                                    "  map_virt_mem_region: page {:#x} already mapped, update_flags failed: {:?}",
                                    page.start_address().as_u64(),
                                    e,
                                );
                            }
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(())
    }

    /// Map a specific alloated physical frame into a user address space
    /// Does not allocate a new frame, caller provides the physical address
    pub fn map_specific_frame(
        &self,
        pml4_table_phys: PhysAddr,
        virt_addr: VirtAddr,
        phys_addr: PhysAddr,
        flags: PageTableFlags,
    ) -> Result<(), MapToError<Size4KiB>> {
        use x86_64::structures::paging::PhysFrame;

        let new_table_pml4_virt = VirtAddr::new(pml4_table_phys.as_u64() + self.phys_offset);
        let pml4_table = unsafe { &mut *(new_table_pml4_virt.as_u64() as *mut PageTable) };
        let mut user_page_mapper =
            unsafe { OffsetPageTable::new(pml4_table, VirtAddr::new(self.phys_offset)) };

        let page = Page::containing_address(virt_addr);
        let phys_frame = PhysFrame::containing_address(phys_addr);
        let user_flags = flags | PageTableFlags::USER_ACCESSIBLE;

        let mut fa = crate::memory::get_frame_allocator();
        unsafe {
            match user_page_mapper.map_to(page, phys_frame, user_flags, &mut *fa) {
                Ok(flush) => {
                    flush.flush();
                    Ok(())
                }
                Err(MapToError::PageAlreadyMapped(_existing)) => {
                    match user_page_mapper.update_flags(page, user_flags) {
                        Ok(flush) => {
                            flush.flush();
                            Ok(())
                        }
                        Err(e) => {
                            serial_println!("map_specific_frame: update_flags failed: {:?}", e);
                            Ok(())
                        }
                    }
                }
                Err(e) => Err(e),
            }
        }
    }

    /// Translate a kernel virtual address (e.g. a heap allocation) to its physical address
    /// by walking the kernel page table. The kernel heap is not HHDM-mapped, its pages
    /// were individually allocated by the frame allocator and mapped by init_heap, so
    /// phys != vaddr - phys_offset. We must walk the page table to find the real frame.
    pub fn translate_kernel_heap_virt_to_phys(&self, vaddr: u64) -> PhysAddr {
        self.translate_user_virt_to_phys(self.kernel_page_table_phys, VirtAddr::new(vaddr))
            .expect("translate_kernel_heap_virt_to_phys: address not mapped in kernel page table")
    }

    pub fn create_main_stack(
        &self,
        pml4_table_phys: PhysAddr,
        stack_size: u64,
        frame_allocator: &mut MemoryMapFrameAllocator,
    ) -> Result<VirtAddr, MapToError<Size4KiB>> {
        let stack_top = VirtAddr::new(USER_STACK_TOP);
        let stack_bottom = stack_top - stack_size;

        let stack_protection_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

        self.map_virt_mem_region(
            pml4_table_phys,
            stack_bottom,
            stack_size,
            stack_protection_flags,
            frame_allocator,
        )?;

        Ok(stack_top)
    }
}
