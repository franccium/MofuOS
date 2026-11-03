use crate::memory::usermem::USER_STACK_TOP;
use crate::{
    data_structures::vector::Vec,
    process::{
        ElfLoadInfo, KernelThread, ThreadState,
        elf_loader::ElfLoadFlags,
        process_mem::ProcessMemoryLayout,
        shared_state::{SHARED_STATE, SharedState},
    },
    serial_println,
};
use alloc::string::String;
use x86_64::{
    VirtAddr,
    structures::paging::{PageTableFlags, Size4KiB, mapper::MapToError},
};

// Marks terminated children
pub const INVALID_PID: usize = usize::MAX;

pub const MAX_PRIORITY: u8 = 8;
pub const RFLAGS_DEFAULT: u64 = 0x202;
pub const DEFAULT_NEW_PROCESS_STACK_SIZE: u64 = 1 * 1024 * 1024;

pub const PROCESS_USER_VADDR_ALLOC_MAX_SIZE: u64 = 0x0000_0000_5000_0000;
pub const PROCESS_USER_VADDR_ALLOC_START: u64 = 0x0000_0000_1000_0000;
pub const PROCESS_USER_VADDR_ALLOC_END: u64 =
    PROCESS_USER_VADDR_ALLOC_START + PROCESS_USER_VADDR_ALLOC_MAX_SIZE;

pub const PROCESS_WINDOW_BUFFER_BASE: u64 = 0x0000_0001_0000_0000;
// Hard clamp in case, we derive this from framebuffer size normally
pub const PROCESS_WINDOW_BUFFER_HARD_MAX_SIZE: u64 = 64 * 1024 * 1024;

// One page per window, indexed by window id, above the window buffer slots
pub const PROCESS_EVENT_BUFFER_BASE: u64 = 0x0000_0008_0000_0000;

pub const PROCESS_HEAP_VIRT_START: u64 = 0x0000_0000_6000_0000;
pub const PROCESS_HEAP_SIZE_BYTES: u64 = PROCESS_WINDOW_BUFFER_BASE - PROCESS_HEAP_VIRT_START;
pub const PROCESS_HEAP_VIRT_END: u64 = PROCESS_HEAP_VIRT_START + PROCESS_HEAP_SIZE_BYTES;

pub type PID = usize;

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

impl ProcessResources {
    pub fn default() -> Self {
        Self {
            memory_limit: 0,
            memory_used: 0,
            cpu_time_slice: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutionContext {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,

    pub rip: u64,
    pub rflags: u64,

    pub page_table_base_phys: u64,
}

const FPU_STATE_SIZE_BYTES: usize = 512;
const FXSAVE_FCW_OFFSET: usize = 0;
const FXSAVE_MXCSR_OFFSET: usize = 24;

// reset values, every FP exception masked
const FCW_DEFAULT: u16 = 0x37F;
const MXCSR_DEFAULT: u32 = 0x1F80;

/// FXSAVE image: x87, xmm0-15 and MXCSR
/// Holds no ymm state because CR4.OSXSAVE is off, so AVX instructions #UD
#[repr(C, align(16))]
pub struct FpuState {
    pub fxsave_area: [u8; FPU_STATE_SIZE_BYTES],
}

const _: () = assert!(core::mem::size_of::<FpuState>() == FPU_STATE_SIZE_BYTES);

impl FpuState {
    pub const fn new() -> Self {
        let mut fxsave_area = [0u8; FPU_STATE_SIZE_BYTES];
        let fcw_bytes = FCW_DEFAULT.to_le_bytes();
        let mxcsr_bytes = MXCSR_DEFAULT.to_le_bytes();

        fxsave_area[FXSAVE_FCW_OFFSET] = fcw_bytes[0];
        fxsave_area[FXSAVE_FCW_OFFSET + 1] = fcw_bytes[1];

        fxsave_area[FXSAVE_MXCSR_OFFSET] = mxcsr_bytes[0];
        fxsave_area[FXSAVE_MXCSR_OFFSET + 1] = mxcsr_bytes[1];
        fxsave_area[FXSAVE_MXCSR_OFFSET + 2] = mxcsr_bytes[2];
        fxsave_area[FXSAVE_MXCSR_OFFSET + 3] = mxcsr_bytes[3];

        Self { fxsave_area }
    }

    pub unsafe fn save_from_cpu(&mut self) {
        unsafe {
            asm!(
                "fxsave64 [{}]",
                in(reg) self.fxsave_area.as_mut_ptr(),
                options(nostack),
            );
        }
    }

    pub unsafe fn restore_to_cpu(&self) {
        unsafe {
            asm!(
                "fxrstor64 [{}]",
                in(reg) self.fxsave_area.as_ptr(),
                options(nostack, readonly),
            );
        }
    }
}

pub const FD_FLAG_READ: u8 = 0x01;
pub const FD_FLAG_WRITE: u8 = 0x02;

#[derive(Debug, Clone, Copy)]
pub struct FileDescriptor {
    pub node_id: usize,
    //pub offset: usize,
    pub flags: u8,
}

impl FileDescriptor {
    pub const INVALID_FD: u64 = u64::MAX;
}

pub struct Process {
    pub pid: PID,
    pub parent_pid: PID,
    pub priority: u8,
    pub state: ProcessState,
    pub name: String,
    pub children: Vec<PID>,
    pub file_descriptors: Vec<FileDescriptor>,

    pub resources: ProcessResources,
    pub exit_code: Option<i32>,
    pub is_out: bool,

    pub execution_context: ExecutionContext,
    pub fpu_state: FpuState,
    pub memory_layout: ProcessMemoryLayout,
}

unsafe impl Send for Process {}

impl ExecutionContext {
    pub fn new(entry_point: u64, stack_top: u64, page_table_base_phys: u64) -> Self {
        Self {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: stack_top,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: entry_point,
            rflags: RFLAGS_DEFAULT,
            page_table_base_phys,
        }
    }
}

impl Process {
    pub fn new(
        pid: usize,
        parent_pid: usize,
        priority: u8,
        name: String,
        is_out: bool,
        resources: ProcessResources,
        entry_point: u64,
        stack_top: u64,
        page_table_base_phys: u64,
    ) -> Result<Self, MapToError<Size4KiB>> {
        let memory_layout =
            ProcessMemoryLayout::with_page_table(x86_64::PhysAddr::new(page_table_base_phys));

        Ok(Self {
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
            execution_context: ExecutionContext::new(entry_point, stack_top, page_table_base_phys),
            fpu_state: FpuState::new(),
            memory_layout,
        })
    }

    pub fn create_with_elf(
        elf_info: &ElfLoadInfo,
        name: &str,
        pid: PID,
        parent_pid: PID,
    ) -> Result<Self, MapToError<Size4KiB>> {
        serial_println!("Process::create_with_elf()");
        //TODO: safer
        let address_space_manager = crate::memory::get_user_mem_mgr();
        let mut frame_allocator = crate::memory::get_frame_allocator();

        let mut memory_layout =
            ProcessMemoryLayout::new(address_space_manager, &mut frame_allocator)?;

        for segment in &elf_info.segments {
            let vaddr = VirtAddr::new(segment.vaddr);
            let in_memory_size = segment.in_memory_size as u64;

            let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
            if segment.flags & ElfLoadFlags::Writable as u32 != 0 {
                flags |= PageTableFlags::WRITABLE;
            }
            if segment.flags & ElfLoadFlags::Executable as u32 == 0 {
                flags |= PageTableFlags::NO_EXECUTE;
            }

            serial_println!(
                "  Mapping ELF segment: vaddr={:#x}, size={:#x}, elf_flags={:#x}",
                vaddr.as_u64(),
                in_memory_size,
                segment.flags,
            );

            if let Err(e) = memory_layout.map_owned_region(
                vaddr,
                in_memory_size,
                flags,
                address_space_manager,
                &mut frame_allocator,
            ) {
                memory_layout.free_address_space(address_space_manager, &mut frame_allocator);
                return Err(e);
            }

            // Copy segment file data into the mapped pages via HHDM.
            // translate_user_virt_to_phys returns the physical address of the
            // page containing the given vaddr, so we drive the copy page-by-page.
            let mut bytes_copied: u64 = 0;
            let file_size = segment.in_file_size as u64;
            while bytes_copied < file_size {
                let src_vaddr = vaddr + bytes_copied;
                let phys = address_space_manager
                    .translate_user_virt_to_phys(memory_layout.top_page_table_phys, src_vaddr)
                    .expect("Failed to translate user vaddr for segment copy");
                let hhdm_vaddr = phys.as_u64() + address_space_manager.phys_offset;

                // How many bytes remain in this 4 KiB page?
                let page_offset = src_vaddr.as_u64() & 0xFFF;
                let bytes_in_page = (0x1000 - page_offset).min(file_size - bytes_copied);

                unsafe {
                    core::ptr::copy_nonoverlapping(
                        segment.data.as_ptr().add(bytes_copied as usize),
                        hhdm_vaddr as *mut u8,
                        bytes_in_page as usize,
                    );
                }
                bytes_copied += bytes_in_page;
            }
            serial_println!("  Copied {} bytes to {:#x}", file_size, vaddr.as_u64());

            // Zero-fill the BSS (in-memory > in-file) via HHDM, page by page.
            if segment.in_memory_size > segment.in_file_size {
                let bss_offset = segment.in_file_size as u64;
                let bss_size = (segment.in_memory_size - segment.in_file_size) as u64;
                let mut bytes_zeroed: u64 = 0;

                while bytes_zeroed < bss_size {
                    let bss_vaddr = vaddr + bss_offset + bytes_zeroed;
                    let phys = address_space_manager
                        .translate_user_virt_to_phys(memory_layout.top_page_table_phys, bss_vaddr)
                        .expect("Failed to translate user vaddr for BSS zeroing");
                    let hhdm_vaddr = phys.as_u64() + address_space_manager.phys_offset;

                    let page_offset = bss_vaddr.as_u64() & 0xFFF;
                    let bytes_in_page = (0x1000 - page_offset).min(bss_size - bytes_zeroed);

                    unsafe {
                        core::ptr::write_bytes(hhdm_vaddr as *mut u8, 0u8, bytes_in_page as usize);
                    }
                    bytes_zeroed += bytes_in_page;
                }
                serial_println!(
                    "  Zeroed BSS: {:#x} bytes starting at user vaddr {:#x}",
                    bss_size,
                    (vaddr + bss_offset).as_u64(),
                );
            }
        }

        let shared_state = SHARED_STATE.get().unwrap().lock();
        shared_state
            .map_into_process(
                address_space_manager,
                &mut memory_layout,
                &mut frame_allocator,
            )
            .unwrap();

        let stack_size = DEFAULT_NEW_PROCESS_STACK_SIZE;
        let stack_top = VirtAddr::new(USER_STACK_TOP);
        let stack_flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        if let Err(e) = memory_layout.map_owned_region(
            stack_top - stack_size,
            stack_size,
            stack_flags,
            address_space_manager,
            &mut frame_allocator,
        ) {
            memory_layout.free_address_space(address_space_manager, &mut frame_allocator);
            return Err(e);
        }

        let context = ExecutionContext::new(
            elf_info.entry_point,
            stack_top.as_u64(),
            memory_layout.top_page_table_phys.as_u64(),
        );

        Ok(Self {
            pid,
            parent_pid: parent_pid,
            priority: 1,
            state: ProcessState::Ready,
            name: String::from(name),
            children: Vec::new(),
            file_descriptors: Vec::new(),
            resources: ProcessResources::default(),
            exit_code: None,
            is_out: true,
            execution_context: context,
            fpu_state: FpuState::new(),
            memory_layout,
        })
    }
}

use core::arch::asm;

/// Save the current CPU context into a thread's ExecutionContext
pub unsafe fn save_thread_context(thread: &mut KernelThread) {
    asm!(
        "mov {}, rsp",
        "mov {}, rbp",
        "mov {}, rbx",
        "mov {}, r12",
        "mov {}, r13",
        "mov {}, r14",
        "mov {}, r15",
        out(reg) thread.context.rsp,
        out(reg) thread.context.rbp,
        out(reg) thread.context.rbx,
        out(reg) thread.context.r12,
        out(reg) thread.context.r13,
        out(reg) thread.context.r14,
        out(reg) thread.context.r15,
        options(nomem, nostack)
    );

    // Get the return address (RIP) from the stack
    asm!(
        "mov {}, [rsp]",
        out(reg) thread.context.rip,
        options(nostack)
    );

    // Get current RFLAGS
    asm!(
        "pushfq; pop {}",
        out(reg) thread.context.rflags,
        options(nostack)
    );

    // CR3 is saved when switching address spaces
    // We'll handle it separately during context switch
}

/// Restore a thread's ExecutionContext to the CPU
pub unsafe fn restore_thread_context(thread: &KernelThread) -> ! {
    // Switch page table if this is a user process
    let cr3 = thread.context.page_table_base_phys;
    if cr3 != 0 {
        asm!(
            "mov cr3, {}",
            in(reg) cr3,
            options(nostack)
        );
    }

    // Restore registers and jump to the thread
    asm!(
        // Restore general purpose registers
        "mov rsp, {}",
        "mov rbp, {}",
        "mov rbx, {}",
        "mov r12, {}",
        "mov r13, {}",
        "mov r14, {}",
        "mov r15, {}",
        // Push return address and flags, then IRETQ or RET
        "push {}",
        "push {}",
        "add rsp, 8", // Skip the push for RIP alignment
        "ret",
        in(reg) thread.context.rsp,
        in(reg) thread.context.rbp,
        in(reg) thread.context.rbx,
        in(reg) thread.context.r12,
        in(reg) thread.context.r13,
        in(reg) thread.context.r14,
        in(reg) thread.context.r15,
        in(reg) thread.context.rflags,
        in(reg) thread.context.rip,
        options(noreturn)
    );

    unreachable!();
}

pub unsafe fn switch_threads(current: &mut KernelThread, next: &KernelThread) {
    // Save current thread's context
    save_thread_context(current);

    // Update thread states
    current.state = ThreadState::Ready;

    // Restore next thread's context
    restore_thread_context(next);
}
