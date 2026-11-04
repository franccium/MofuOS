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
| User heap                   | 0x0000_0000_6000_0000       | PROCESS_HEAP_VIRT_START |

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

Global: `USER_MEMORY_MANAGER: Once<UserMemoryManager>`. It has only `&self` methods and write-once fields,
so it needs no mutex.
Access: `memory::get_user_mem_mgr()` returns `&'static UserMemoryManager`.

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
1. Allocates a zeroed physical frame.
2. Calls `map_to` with `protection_flags | USER_ACCESSIBLE`.
3. On `PageAlreadyMapped`: merges flags (OR capability bits, AND NO_EXECUTE).
   This handles overlapping ELF segments sharing a page boundary.

If a page cannot be mapped, the pages already mapped by the same call are unmapped again and
their frames freed (`rollback_partial_mapping`), so a failed call leaves nothing behind.

### map_specific_frame

Maps a caller supplied physical frame at a user page, no frame is allocated. If the page is
already mapped to the same frame only the flags are updated. If it is mapped to a different
frame it returns `PageAlreadyMapped` (with a `debug_assert`), so a stale mapping is never
silently kept.

### unmap_user_region

Takes a `MappedMemoryRegion` and unmaps every page with `Mapper::unmap`. Absent pages are
skipped, so a page shared by two regions (ELF segments) is released once, by whichever region
clears its entry first. The frame is freed only if the region is `FrameOwnership::Owned`.
Cost is O(pages) plus one `flush_all` per region.

### free_user_tables

Frees every PT, PD and PDPT of the user half after all regions are unmapped. It has a
`debug_assert` that each page table is empty, so a mapping that was never tracked in a region
shows up in debug builds.

### translate_user_virt_to_phys

Manually walks the 4-level page table (PML4→PDPT→PD→PT) to convert a user virtual
address to a physical address. Used during ELF loading to copy segment data and zero BSS.

### get_page_flags

Walks the page table to read the PageTableFlags of an already-mapped page.
Used for flag-merging on `PageAlreadyMapped`.

## ProcessMemoryLayout

File: `process/process_mem.rs`

```rust
pub enum FrameOwnership {
    Owned,    // freed when the region is unmapped
    Borrowed, // kernel owned frames, or a second view of frames owned by another region
}

pub struct MappedMemoryRegion {
    pub start_virt: VirtAddr,
    pub size_bytes: u64,
    pub page_flags: PageTableFlags,
    pub frame_ownership: FrameOwnership,
}

pub struct ProcessMemoryLayout {
    pub top_page_table_phys: PhysAddr,
    pub heap_end: VirtAddr,         // bump cursor, starts at PROCESS_HEAP_VIRT_START
    pub next_alloc_vaddr: VirtAddr, // 0x1000_0000 bump for the reservation window
    pub regions: Vec<MappedMemoryRegion>, // sorted by start_virt, page aligned
}
```

Every user mapping of a process lives in `regions`, with explicit frame ownership. There is one
mapping path and one teardown path for all of it. `ProcessMemoryLayout::with_page_table` is the
single constructor, `new` allocates an address space and calls it.

Mapping API, all taking `&UserMemoryManager` and `&mut MemoryMapFrameAllocator`:

- `map_owned_region(start, size, flags, ..)` – zeroed frames owned by the process. Used by ELF
  segments, the stack and the heap. Rolls back on failure, tracks on success.
- `map_frames(start, frames: &[PhysAddr], flags, ownership, ..)` – explicit frames. Used by circular
  buffers, window buffers, event buffers and the shared data page. On failure the pages mapped by
  the call are unmapped again and the frames are left to the caller.
- `unmap_region(start, size, ..)` – unmaps a page aligned range inside one tracked region, frees
  frames only for `Owned`, and splits the region if the range is in the middle.
- `track_region` (private) – inserts sorted and merges with a touching or overlapping region of
  equal flags and ownership. This keeps the heap a single region however often it grows.
- `is_range_tracked(start, size)` – true if one region covers the range.

### Heap growth

- `allocate_heap(size, umm, fa) -> VirtAddr` – used by `sys_allocate(5)`. Aligns `size` to 4 KiB,
  calls `grow_heap(heap_end + aligned)`, returns the old `heap_end`.
- `grow_heap(new_heap_end, umm, fa)` – `map_owned_region` for `heap_end..new_heap_end` with
  `PRESENT|WRITABLE|NO_EXECUTE`, then bumps `heap_end`. A partial out of memory failure is rolled
  back, nothing is leaked.

### Reservation window

`allocate_virtual_range(size) -> Option<VirtAddr>` reserves a range in `0x1000_0000..0x5000_0000`
(for circular buffers and later mmap) without mapping or tracking it, the caller maps it right
away. When the bump cursor is exhausted `find_free_gap` scans the sorted regions linearly for a
gap, without allocating.

### Circular buffer

`CircularBuffer::map_for_user` reserves `2 * data_size`, allocates the frames, maps them with
`map_frames(base, Owned)` and `map_frames(base + data_size, Borrowed)`. The second view does not
own the frames, so teardown needs no deduplication. User supplied flags are reduced to
`PRESENT|USER_ACCESSIBLE|NO_EXECUTE`, plus `WRITABLE` if requested. A zero size, or a size whose
two views do not fit the window, is rejected. Any failure frees the frames and undoes the mapping.

### Windows, event buffers, shared data

Window buffers, the per window event buffer page and the shared data page are tracked as
`Borrowed`, since the kernel owns the frames. `Window.owner_pid` records the process that
mapped them. `destroy_window` unmaps them from the owner and frees the event page, and process
exit destroys the windows of the process (before `PROCESS_MANAGER` is locked). The event buffer of
window `id` is mapped at `PROCESS_EVENT_BUFFER_BASE + id * PAGE_SIZE`, `GetWindowInfo` returns
that address in `event_buffer_vaddr`.

### Teardown

`free_address_space(user_mgr, frame_allocator)` on `terminate_process`:

```rust
for region in self.regions.iter() {
    user_mgr.unmap_user_region(pml4, region, frame_allocator);
}
user_mgr.free_user_tables(pml4, frame_allocator);
user_mgr.free_pml4_frame(pml4, frame_allocator);
```

Cost is O(pages), there are no linear scans over the freed frames. `terminate_process` calls it
directly on the process layout (no clone) and then clears `regions`.

Verified by `test_usermem_reclaim`, `test_usermem_layout` and `test_usermem_windows`, see
"Tests and logs".

## UserMemoryManager helpers

`unmap_user_region`, `free_user_tables` and `free_pml4_frame` in `memory/usermem.rs` implement
the teardown. `get_page_flags` is used for the merge on `PageAlreadyMapped` in
`map_virt_mem_region`. `translate_kernel_heap_virt_to_phys` walks the kernel PML4 (no
`vaddr - HHDM_OFFSET` for the heap, bug BUG-11).

## Global Memory Accessors

Lock order for the memory related locks is `PROCESS_MANAGER`, then `FRAME_ALLOCATOR`. The
compositor lock, when needed, comes before `PROCESS_MANAGER`.

```rust
// Call after boot:
memory::init_memory_globals(frame_allocator, user_mem_manager);

// Use anywhere:
memory::get_frame_allocator() -> MutexGuard<MemoryMapFrameAllocator>
memory::get_user_mem_mgr()    -> &'static UserMemoryManager
```

## Known Issues / TODOs

- `map_virt_mem_region` overlap frame leak fixed: on `PageAlreadyMapped` the spare
  frame is now `deallocate_frame`'d and flags merged. Huge-page intermediate tables
  are still 4 KiB-only; no 2 MiB alloc path.
- There is no unmap syscall or page fault path yet. `unmap_region` and the sorted `regions` vector
  are the base for `munmap`. Killing a process that runs on another core has no TLB shootdown,
  `TerminateProcess` only refuses when the target is currently running on another core and that
  check is not atomic with the teardown.
- ACPI region mapping helper (`map_acpi_regions`) is currently commented out in kmain.
- `IdendtityAcpiHandler` (typo: should be "Identity") — most read/write methods are
  `todo!()` panics, only the mapping is implemented.
- Early `Vec::with_capacity` in `MemoryMapFrameAllocator::init` must stay `Vec::new`
  until after `init_heap` (heap before alloc).

## Tests and logs

- `cargo xtask usertest` (`kernel/src/bin/test_usermem_reclaim.rs`) exercises user heap
  reclamation: boots via `boot_common::bsp_early_init` + `init_shared_state`, waits for
  APs, snapshots `free_frame_count` and the frames in use (`allocated_bump_frames`, which is
  frames handed out minus the free list), creates `memstress` ELF (`user/rustspace/src/bin/memstress.rs`
  4×528384), polls `PROCESS_MANAGER`/`scheduler::is_pid_queued` for `Terminated`, asserts `free` reclaimed,
  the frames in use did not grow, the free list has no duplicate frame, and `cleanup_dead`
  removes PID.
- `kernel/src/bin/test_usermem_layout.rs` drives `ProcessMemoryLayout` directly: heap growths
  coalesce into one region, circular buffer ownership and flag normalization, rejected
  circular requests, `unmap_region` splitting, borrowed frames not being freed, and an exact
  frames-in-use comparison after `free_address_space`.
- `kernel/src/bin/test_usermem_windows.rs` runs the `winstress` ELF
  (`user/rustspace/src/bin/winstress.rs`): two circular buffers with alias checks, many heap
  growths, two windows with distinct working event buffers, window id reuse after destroy with a
  pixel write to the new buffer, then an exit with two windows open. The kernel then checks
  that the windows are gone, event pages released, no frames leaked and no duplicate free. QEMU `-serial stdio -serial file:` captures COM1+COM2.
- `cargo xtask test` and `usertest` save split logs like `scripts/log_splitter.py` to
  `test_logs/<test>/all.txt` + `core_N.txt` + `userspace_pid_N.txt` via `xtask/src/main.rs:save_test_logs`
  (ANSI stripped, `MAX_CORES` from `kernel/src/lib.rs:21`). Also `cargo xtask usertest 2>&1 | cat`
  now prints serial even on `ok` (previously only on fail).
