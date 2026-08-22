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

Bump allocator with free-list reclamation over Limine's memory map. Allocates from
`MEMMAP_USABLE` entries; freed frames are returned to an intrusive free list and
reused before bumping.

```rust
pub struct MemoryMapFrameAllocator {
    memory_map: &'static [&'static Entry],
    curr_region_index: usize,
    frame_offset_in_region: u64,
    free_list: Vec<PhysFrame<Size4KiB>>,
}
```

`allocate_frame()` pops `free_list` first, else bumps `curr_region_index`/`frame_offset_in_region`.
`deallocate_frame(frame)` pushes onto `free_list` (debug_assert 4 KiB aligned).
`free_frame_count()` and `allocated_bump_frames()` expose reclamation for tests.

Usage: `memory::get_frame_allocator()` → `MutexGuard<MemoryMapFrameAllocator>`.
Global: `FRAME_ALLOCATOR: Once<Mutex<MemoryMapFrameAllocator>>`.

Init note: `Vec::new()` (not `with_capacity`) before `allocator::init_heap` to avoid
heap allocation before heap is mapped (`boot_common::bsp_early_init` order).

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
    pub heap_start: VirtAddr,  // 0x0000_0000_6000_0000
    pub heap_end: VirtAddr,    // bump cursor, starts == heap_start
    pub mapped_regions: Vec<MappedMemoryRegion>,
    pub next_alloc_vaddr: VirtAddr,       // 0x1000_0000 bump for CircularBuffer etc
    pub allocated_ranges: Vec<(u64,u64)>, // tracked for reclaim
}
```

`ProcessMemoryLayout::new` allocates a new address space via `UserMemoryManager` and
initializes `heap_start == heap_end == PROCESS_HEAP_VIRT_START` (empty heap). Before
2026-08-22 `heap_end` was incorrectly `VIRT_END`; fixed to `VIRT_START` so first
`sys_allocate` returns `0x6000_0000`.

### Heap growth and reclamation

- `allocate_heap(size, umm, fa) -> VirtAddr` – single-lock bump API used by `sys_allocate(5)`.
  Aligns `size` to 4 KiB, computes `new = heap_end + aligned`, calls `grow_heap(new)`,
  returns old `heap_end`. Replaces previous double-lock `old/new` arithmetic in `syscall.rs`.

- `grow_heap(new_heap_end, umm, fa)` – maps `heap_end..new_heap_end` via
  `UserMemoryManager::map_virt_mem_region` (`PRESENT|WRITABLE|USER_ACCESSIBLE`,
  per-page frame alloc, merges `PageAlreadyMapped` flags), pushes `MappedMemoryRegion`,
  bumps `heap_end`. Shrink path only moves cursor (no unmap; heap never shrinks in
  current `sys_allocate` path).

- `allocate_virtual_range(size) -> VirtAddr` – bump at `0x1000_0000..0x5000_0000`
  for `CircularBuffer` double-map and window buffers; `find_free_gap` is TODO.

- `free_address_space(user_mgr, frame_allocator)` – reclamation on `terminate_process`.
  Collects owned `PhysAddr` from `mapped_regions` + `stack` + `allocated_ranges` via
  `translate_user_virt_to_phys`, then:
  1. `user_mgr.unmap_all_user_pages_with_owned_set(pml4, owned, fa)` – walks user
     PML4 0..255 (handles 2 MiB huge), `set_unused` + `tlb::flush` per page,
     `deallocate_frame` for owned leaves (dedup via `already_freed`).
  2. `user_mgr.reclaim_empty_user_tables(pml4, fa)` – frees empty PT/PD/PDPT frames
     bottom-up (`is_unused` all entries) + `deallocate_frame`.
  3. `user_mgr.free_pml4_frame(pml4, fa)` – frees top-level table + `flush_all`.

  Intermediate page tables allocated via `FrameAllocator` during `map_to` are thus
  reclaimed. Verified by `test_usermem_reclaim` (`kernel/src/bin/test_usermem_reclaim.rs`)
  which does 4×528384 (≈2.06 MB > 2 MB heap) allocations, exits, and asserts
  `free_frame_count` returns to baseline (801 vs 0 before) and PID removed from
  `PROCESS_MANAGER` + `SCHEDULER`.

### UserMemoryManager helpers

- `unmap_all_user_pages_with_owned_set`, `reclaim_empty_user_tables`, `free_pml4_frame`
  in `memory/usermem.rs` implement the walk. `get_page_flags` used for merge on
  `PageAlreadyMapped`. `translate_kernel_heap_virt_to_phys` walks kernel PML4 (no
  `vaddr - HHDM_OFFSET` for heap – bug BUG-11).

## Global Memory Accessors

```rust
// Call after boot:
memory::init_memory_globals(frame_allocator, user_mem_manager);

// Use anywhere:
memory::get_frame_allocator() -> MutexGuard<MemoryMapFrameAllocator>
memory::get_user_mem_mgr()    -> MutexGuard<UserMemoryManager>
```

## Known Issues / TODOs

- `map_virt_mem_region` overlap frame leak fixed: on `PageAlreadyMapped` the spare
  frame is now `deallocate_frame`'d and flags merged. Huge-page intermediate tables
  are still 4 KiB-only; no 2 MiB alloc path.
- Process exit reclamation implemented (`free_address_space` + `unmap` + `reclaim`),
  but `find_free_gap` for `allocate_virtual_range` is still TODO (returns `None` if
  `next_alloc_vaddr` exceeds `0x5000_0000`). Reclaim is single-core; TLB shootdown
  via IPI not needed (per-process PML4 private) but kernel global unmap will need it.
- ACPI region mapping helper (`map_acpi_regions`) is currently commented out in kmain.
- `IdendtityAcpiHandler` (typo: should be "Identity") — most read/write methods are
  `todo!()` panics, only the mapping is implemented.
- Early `Vec::with_capacity` in `MemoryMapFrameAllocator::init` must stay `Vec::new`
  until after `init_heap` (heap before alloc).

## Tests and logs

- `cargo xtask usertest` (`kernel/src/bin/test_usermem_reclaim.rs`) exercises user heap
  reclamation: boots via `boot_common::bsp_early_init` + `init_shared_state`, waits for
  APs, snapshots `free_frame_count`, creates `memstress` ELF (`user/rustspace/src/bin/memstress.rs`
  4×528384), polls `PROCESS_MANAGER`/`SCHEDULER` for `Terminated`, asserts `free` reclaimed
  and `cleanup_dead` removes PID. QEMU `-serial stdio -serial file:` captures COM1+COM2.
- `cargo xtask test` and `usertest` save split logs like `scripts/log_splitter.py` to
  `test_logs/<test>/all.txt` + `core_N.txt` + `userspace_pid_N.txt` via `xtask/src/main.rs:save_test_logs`
  (ANSI stripped, `MAX_CORES` from `kernel/src/lib.rs:21`). Also `cargo xtask usertest 2>&1 | cat`
  now prints serial even on `ok` (previously only on fail).
