# User Renderer — Option A Implementation Plan

Shared-memory pixel buffers. Processes draw directly into mapped pages.
Kernel only owns compositing and framebuffer output.

---

## What moves where

### Stays in kernel (kernel/src/graphics/)
- `compositor.rs` — window management, z-ordering, compositing to framebuffer
- `framebuffer.rs` — physical display framebuffer ownership
- `window.rs` — `Window`, `WindowBuffer` structs (kernel allocates the pixel pages)

Everything else in `kernel/src/graphics/` moves to userspace:
- `color.rs`
- `pipeline.rs`
- `renderer.rs`
- `resources.rs`
- `shaders.rs`
- `transform.rs`
- `programs/theophe.rs`

### Moves to userspace (user/rustspace/src/)
All the above become a graphics library inside rustspace.
Suggested layout:
```
user/rustspace/src/
  lib.rs              -- syscalls, allocator, Serial, print!/println!
  gfx/
    mod.rs
    color.rs          -- Rgba8888UNORM, rgba_to_xrgb, xrgb_to_rgba (ported verbatim)
    surface.rs        -- UserSurface: wraps a *mut u32 slice + width/height
                         implements DrawTarget<Color=Rgb888>
    pipeline.rs       -- Vertex2D, Vertex3D, VSOut, PSIn, shader traits, etc.
    renderer.rs       -- RenderContext, rasterizer (ported verbatim)
    resources.rs      -- Texture, ConstantBuffer, RWBuffer, DepthBuffer
    shaders.rs        -- PassThroughVS, TextureSamplePS, BlinnPhong*, etc.
    transform.rs      -- Matrix4x4, F32x4Ext, create_perspective_matrix
  programs/
    theophe.rs        -- Theophe<D: DrawTarget> (ported verbatim, no kernel deps)
```

`UserSurface` replaces `WindowBackBuffer`. It holds:
```rust
pub struct UserSurface {
    pub pixels: *mut u32,   // mapped back buffer from sys_map_window_buffer
    pub width:  u32,
    pub height: u32,
}
```
Implements `DrawTarget<Color=Rgb888>` and exposes `write_pixel`, `clear`, `as_slice_mut`.
`RenderTarget` (in renderer.rs) wraps `&mut UserSurface` instead of `&mut WindowBackBuffer`.

---

## New syscalls needed

Two new syscalls added to `SyscallNumber` enum and `handle_syscall_inner`:

### sys_map_window_buffer (12)
```
arg1 = window_id: u32
returns: virtual address of the mapped back buffer (*mut u32), or u64::MAX on failure
```
Kernel side:
1. Look up `WindowBuffer` for `window_id` in `COMPOSITOR`.
2. Get the physical address of each page of `back_buffer` via HHDM offset.
   `WindowBuffer` allocates via `alloc_zeroed` so pages are contiguous in the
   kernel heap — walk them with `translate_kernel_virt_to_phys` (same approach
   as `translate_user_virt_to_phys` but for kernel addresses).
3. Map those physical pages into the calling process's user page table
   using `map_virt_mem_region` with `PRESENT | WRITABLE | USER_ACCESSIBLE | NO_EXECUTE`.
   Target user virtual address: a fixed per-window slot, e.g.
   `USER_WINDOW_BUFFER_BASE + window_id * MAX_WINDOW_BUFFER_SIZE`
   where `USER_WINDOW_BUFFER_BASE = 0x0000_0001_0000_0000` and
   `MAX_WINDOW_BUFFER_SIZE = 8 MB` (enough for any reasonable window).
4. Return that user virtual address.

Also return `width` and `height` — simplest way: pack them into two additional
return registers, OR add a separate `sys_get_window_size(window_id)` syscall that
returns `(width << 32) | height` as a u64.

### sys_present_window (13)
```
arg1 = window_id: u32
returns: 0 on success
```
Kernel side: calls `window_buffer.present()` (sets `needs_swap = true`).
The compositor will swap back/front on the next compose call.

### sys_get_window_size (14)  [optional convenience]
```
arg1 = window_id: u32
returns: (width as u64) << 32 | (height as u64)
```

Userspace side additions to `lib.rs`:
```rust
pub const SYS_MAP_WINDOW_BUFFER: u64 = 12;
pub const SYS_PRESENT_WINDOW: u64    = 13;
pub const SYS_GET_WINDOW_SIZE: u64   = 14;

pub unsafe fn sys_map_window_buffer(window_id: u32) -> *mut u32;
pub unsafe fn sys_present_window(window_id: u32);
pub unsafe fn sys_get_window_size(window_id: u32) -> (u32, u32);
```

---

## kernel/src/memory/usermem.rs additions

Need a helper to translate a kernel virtual address (in the HHDM-mapped heap)
to a physical address, so we can map those pages into user address spaces:

```rust
pub fn translate_kernel_heap_virt_to_phys(&self, vaddr: u64) -> PhysAddr {
    // kernel heap pages are direct-mapped via HHDM_OFFSET:
    // phys = vaddr - phys_offset  (phys_offset == HHDM_OFFSET)
    PhysAddr::new(vaddr - self.phys_offset)
}
```

Then `sys_map_window_buffer` uses this + `map_virt_mem_region` to map
each page of the pixel buffer into the user page table.
Important: only map pages, do NOT re-allocate. Use a variant of
`map_virt_mem_region` that takes an explicit physical frame instead of
allocating a new one. Add:
```rust
pub fn map_specific_frame(
    &self,
    pml4_table_phys: PhysAddr,
    virt_addr: VirtAddr,
    phys_addr: PhysAddr,
    flags: PageTableFlags,
) -> Result<(), MapToError<Size4KiB>>
```

---

## kernel/src/graphics/window.rs changes

`WindowBuffer` currently allocates pixel data on the kernel heap with `alloc_zeroed`.
The physical layout of these pages must be discoverable for mapping into user space.
Since `alloc_zeroed` gives us a virtual address and kernel heap pages are HHDM-mapped,
`phys = vaddr - HHDM_OFFSET`. No structural changes to `WindowBuffer` needed —
the kernel side is unchanged.

One addition: expose `back_buffer_virt_addr() -> u64` and `pixel_count() -> usize`
on `WindowBuffer` so the syscall handler can compute the page range without
reaching into `UnsafeCell` internals.

---

## Cargo.toml changes for rustspace

Add dependencies:
- `embedded-graphics = "0.8.2"` — for DrawTarget, Theophe
- `micromath = "2.1.0"` — for F32Ext used in shaders/transform

```toml
[dependencies]
embedded-graphics = { version = "0.8.2", default-features = false }
micromath = { version = "2.1.0", default-features = false, features = ["f32"] }
```

Add feature gate for portable_simd since `core::simd` is nightly:
```toml
# in lib.rs
#![feature(portable_simd)]
```

---

## Implementation order

1. **kernel**: add `map_specific_frame` to `usermem.rs`
2. **kernel**: add `back_buffer_virt_addr()` + `pixel_count()` to `WindowBuffer`
3. **kernel**: implement `sys_map_window_buffer` (12), `sys_present_window` (13),
   `sys_get_window_size` (14) in `handle_syscall_inner`
4. **kernel**: add new `SyscallNumber` variants (12, 13, 14)
5. **rustspace**: add `Cargo.toml` deps (embedded-graphics, micromath)
6. **rustspace**: port `color.rs` to `gfx/color.rs` (remove kernel imports)
7. **rustspace**: implement `UserSurface` in `gfx/surface.rs`
8. **rustspace**: port `pipeline.rs`, `renderer.rs`, `resources.rs`,
   `shaders.rs`, `transform.rs` to `gfx/` (swap `WindowBackBuffer` -> `UserSurface`,
   remove `crate::serial_println` -> no-op or `println!`)
9. **rustspace**: port `theophe.rs` to `programs/theophe.rs` (already generic over
   `D: DrawTarget`, no changes needed beyond removing kernel imports)
10. **rustspace**: add syscall wrappers to `lib.rs`
11. **game.rs**: use `sys_map_window_buffer`, wrap in `UserSurface`, draw with Theophe
12. **kernel**: remove `programs/theophe.rs`, remove graphics modules that moved,
    update `main.rs` to use the compositor-only path

---

## What does NOT change

- `compositor.rs` and `framebuffer.rs` stay exactly as-is
- `WindowBuffer` double-buffering logic is untouched
- The compositing loop in `main.rs` stays
- All existing syscalls (create_window, destroy_window, allocate, etc.) unchanged
- The ELF loader, scheduler, memory manager — nothing in process infrastructure changes

---

## Notes / invariants

- The back buffer is mapped WRITABLE into user space. The front buffer is NOT mapped
  — user processes can only write the back buffer. The compositor reads the front
  buffer, which is only accessible from kernel space.
- After `sys_present_window`, the compositor will pointer-swap back and front on
  the next compose tick. At that point the user process's mapped address still
  points to the old front buffer (now the new back buffer). This is correct —
  the user should simply redraw into it each frame.
- `map_specific_frame` must NOT flush the TLB for the kernel's mapping of those pages
  (we don't want to break the kernel's view of them). Use `flush()` only for the
  new user-space mapping.
- The `USER_WINDOW_BUFFER_BASE` range must not overlap the process heap
  (`0x6000_0000+`) or stack (`0x7FFF_FFFF_F000`). `0x0000_0001_0000_0000` (4 GB)
  is safely above the heap and well below the stack.
