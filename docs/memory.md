# MofuOS — Memory Subsystem

## Files

```
kernel/src/memory/
  mod.rs        — global statics, init_memory_globals(), accessor functions
  memory.rs     — OffsetPageTable setup, MemoryMapFrameAllocator, ACPI mapping helpers
  allocator.rs  — FixedSizeBlockAllocator + linked_list_allocator fallback (heap)
  usermem.rs    — UserMemoryManager (user address space creation + mapping)
```

## Address Space Layout

All addresses are virtual (x86_64, 4-level paging).

| Region                      | Virtual Address             | Notes                        |
|-----------------------------|-----------------------------|------------------------------|
| Kernel image                | 0xffffffff80000000+         | Limine higher-half mapping   |
| HHDM (identity-like map)    | 0xFFFF_8000_0000_0000+      | HHDM_OFFSET constant         |
| Kernel heap                 | 0xFFFF_8080_0000_0000       | HEAP_POINTER, 16 MB          |
| LAPIC MMIO (kernel)         | 0xFFFF_FFFF_0000_0000       | LAPIC_VIRT_BASE in interrupts.rs |
| IOAPIC MMIO (kernel)        | 0xFFFF_FFFF_FF00_0000       | IOAPIC_VIRT_BASE             |
| User stack top              | 0x7FFF_FFFF_F000            | USER_STACK_TOP in usermem.rs |
| User heap                   | 0x0000_0000_6000_0000       | heap_start in ProcessMemoryLayout |

PML4 split: indices 0-255 = user space, indices 256-511 = kernel space. When a new
user address space is created, all kernel PML4 entries (256-511) are copied from the
kernel's own PML4 into the new page table.

## Physical Frame Allocator (MemoryMapFrameAllocator)

File: `memory/memory.rs`

Simple bump allocator over Limine's memory map. Only allocates from `MEMMAP_USABLE`
entries. No deallocation support (frames are never returned). Once the kernel is
initialized the frame allocator should not be used for hot-path allocations.

```rust
pub struct MemoryMapFrameAllocator {
    memory_map: &'static [&'static Entry],
    curr_region_index: usize,
    frame_offset_in_region: u64,
}
```

Usage: `memory::get_frame_allocator()` → `MutexGuard<MemoryMapFrameAllocator>`.
Global: `FRAME_ALLOCATOR: Once<Mutex<MemoryMapFrameAllocator>>`.

KNOWN LIMITATION: There is no `deallocate_frame`. Pages allocated for processes
are never reclaimed. This is a known TODO.

## Offset Page Table

`init_offset_page_table(hhdm_offset)` reads CR3, adds HHDM offset to get the virtual
address of the PML4 table, and returns an `OffsetPageTable` (from the `x86_64` crate).
This is used only during boot for setting up the heap, ACPI regions, and LAPIC mapping.

After boot, direct page table manipulation is done through `UserMemoryManager` which
re-creates its own `OffsetPageTable` from the physical address of a user PML4.

Helper functions:
```rust
pub const fn align_up(x: u64, align: u64) -> u64
pub const fn align_down(x: u64, align: u64) -> u64
```

## Heap (FixedSizeBlockAllocator)

File: `memory/allocator.rs`

Two-tier allocator:

1. Fixed-size slab lists for sizes: `[8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096]` bytes.
   Each size class is a free list (`AllocatorListNode`). On `alloc`, pop from the
   matching list (or fall through to the fallback). On `dealloc`, push back to the list.

2. Fallback: `linked_list_allocator::Heap` handles sizes not covered by the slab lists
   and backs new slab blocks.

`#[global_allocator]` static: `ALLOCATOR: MutexWrapper<FixedSizeBlockAllocator>`.

Heap virtual range: `HEAP_POINTER..HEAP_POINTER + HEAP_SIZE_BYTES`
- `HEAP_POINTER = 0xFFFF_8080_0000_0000`
- `HEAP_SIZE_BYTES = 16 * 1024 * 1024` (16 MB)

`init_heap` maps all heap pages (frame allocator → mapper) then calls
`init_fallback_allocator`. Must be called after the frame allocator and
page table are initialized.

Debug mode: `ALLOC_DEBUG` const in allocator.rs — set true to get verbose logs.

## User Memory Manager (UserMemoryManager)

File: `memory/usermem.rs`

Manages the creation of per-process virtual address spaces and mapping of ELF segments
and stacks into them.

```rust
pub struct UserMemoryManager {
    pub kernel_page_table_phys: PhysAddr,  // BSP PML4 physical address
    pub phys_offset: u64,                  // HHDM_OFFSET
}
```

Global: `USER_MEMORY_MANAGER: Once<Mutex<UserMemoryManager>>`.
Access: `memory::get_user_mem_mgr()`.

### allocate_new_address_space

1. Allocate a new 4KB physical frame for the new PML4.
2. Zero it.
3. Copy kernel PML4 entries (indices 256-511) from the kernel's PML4 into the new PML4.
   This ensures the kernel is accessible from any user process's address space.
4. Returns the physical address of the new PML4 frame.

NOTE: The LAPIC MMIO mapping must already be in the kernel PML4 at the time of
address space creation; it is verified by checking pml4[lapic_pml4_idx].

### map_virt_mem_region

Maps a virtual address range in a given user PML4. Each page:
1. Allocates a physical frame.
2. Calls `map_to` with `protection_flags | USER_ACCESSIBLE`.
3. On `PageAlreadyMapped`: merges flags (OR capability bits, AND NO_EXECUTE).
   This handles overlapping ELF segments sharing a page boundary.

### create_main_stack

Maps `stack_size` bytes ending at `USER_STACK_TOP = 0x7FFF_FFFF_F000`.
Flags: `PRESENT | WRITABLE` (no USER_ACCESSIBLE added explicitly here — added by
`map_virt_mem_region`).

### translate_user_virt_to_phys

Manually walks the 4-level page table (PML4→PDPT→PD→PT) to convert a user virtual
address to a physical address. Used during ELF loading to copy segment data and zero BSS.

### get_page_flags

Walks the page table to read the PageTableFlags of an already-mapped page.
Used for flag-merging on `PageAlreadyMapped`.

## ProcessMemoryLayout

File: `process/process_mem.rs`

```rust
pub struct ProcessMemoryLayout {
    pub top_page_table_phys: PhysAddr,
    pub stack_top: VirtAddr,
    pub stack_size: u64,
    pub heap_start: VirtAddr,  // 0x0000_0000_6000_0000 (placeholder)
    pub heap_end: VirtAddr,    // same (heap not yet implemented for userspace)
    pub mapped_regions: Vec<MappedMemoryRegion>,
}
```

`ProcessMemoryLayout::new` allocates a new address space via `UserMemoryManager`.

## Global Memory Accessors

```rust
// Call after boot:
memory::init_memory_globals(frame_allocator, user_mem_manager);

// Use anywhere:
memory::get_frame_allocator() -> MutexGuard<MemoryMapFrameAllocator>
memory::get_user_mem_mgr()    -> MutexGuard<UserMemoryManager>
```

## Known Issues / TODOs

- No frame deallocation: leaked physical memory on process exit.
- No per-process heap (user heap_start == heap_end == 0x6000_0000, not mapped).
- `map_virt_mem_region` allocates one physical frame per virtual page even on overlap;
  the unused frame from an `AlreadyMapped` error is leaked.
- ACPI region mapping helper (`map_acpi_regions`) is currently commented out in kmain.
- `IdendtityAcpiHandler` (typo: should be "Identity") — most read/write methods are
  `todo!()` panics, only the mapping is implemented.
