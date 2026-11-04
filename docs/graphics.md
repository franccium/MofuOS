# MofuOS — Graphics Subsystem

## Files

```
kernel/src/graphics/
  mod.rs         — module declarations + FRAMEBUFFER_BYTES_PER_PIXEL = 4
  color.rs       — Rgba8888UNORM, Rgba8888F, rgba_to_xrgb, xrgb_to_rgba
  framebuffer.rs — FrameBufferTarget (wraps Limine fb), init/get_framebuffer
  window.rs      — Window, WindowBuffer (double-buffered), WindowBackBuffer, Rect
  compositor.rs  — Compositor (z-sorted blit to framebuffer)
  pipeline.rs    — Vertex2D, Vertex3D, PipelineState, VSOut, PSIn, shader traits
  renderer.rs    — RenderContext, rasterizer, triangle drawing
  resources.rs   — Texture, ConstantBuffer, RWBuffer
  shaders.rs     — PassThroughVS, TextureSamplePS (built-in shaders)
  transform.rs   — matrix math (view/projection/model for 3D pipeline)
  text.rs        — empty file (text rendering not yet implemented)
  programs/theophe.rs — Theophe: software text terminal renderer
```

## Pixel Format

All internal pixel buffers (window back/front buffers, framebuffer) use **XRGB8888**
packed as a `u32`:

```
Bit 31..24: ignored (X)
Bit 23..16: R
Bit 15..8:  G
Bit 7..0:   B
```

`rgba_to_xrgb(color: Rgba8888UNORM) -> u32` is the canonical conversion.
`FRAMEBUFFER_BYTES_PER_PIXEL = 4` (always).

The Limine framebuffer reports its own bit masks (red_mask_size, red_mask_shift, etc.)
via the Framebuffer struct. The current code writes XRGB directly without checking
those masks — this works for the common case (BGRX / XRGB) in QEMU. If the hardware
has a different layout, `write_pixel` on `FrameBufferTarget` would need to use the
mask fields.

## Framebuffer (FrameBufferTarget)

File: `graphics/framebuffer.rs`

Wraps the Limine framebuffer response. Owns a `NonNull<()>` pointer to the linear
framebuffer memory. Fields mirror Limine's Framebuffer struct.

Key methods:
- `write_pixel(x, y, Rgb888)` — bounds-checked, writes XRGB u32
- `fill_rect(x, y, w, h, Rgb888)` — fills a rectangle
- `copy_row(src: *const u32, dst_y, dst_x, width)` — raw row copy
- Implements `embedded_graphics::DrawTarget<Color = Rgb888>` — can use the
  embedded-graphics primitive library (circles, rectangles, etc.) directly.

Global singleton:
```rust
static FRAMEBUFFER_TARGET: Once<Mutex<FrameBufferTarget>>
pub fn init_framebuffer(fb: &limine::framebuffer::Framebuffer)
pub fn get_framebuffer() -> MutexGuard<'static, FrameBufferTarget>
```

The framebuffer is initialized once in `kmain` and accessed via `get_framebuffer()`.

## Window System (window.rs)

### Window

```rust
pub struct Window {
    pub id: WindowID,       // u32; INVALID_WINDOW_ID = u32::MAX
    pub x: i32, pub y: i32,
    pub z_index: u8,
    pub is_visible: bool,
    pub buffer: Arc<WindowBuffer>,
}
```

### WindowBuffer

Double-buffered. Two heap-allocated pixel buffers (back and front), each
`width * height * 4` bytes. Allocated page-aligned.

```rust
pub struct WindowBuffer {
    pub width: u32, pub height: u32,
    pub x: i32, pub y: i32,
    back_buffer: UnsafeCell<NonNull<u32>>,
    front_buffer: UnsafeCell<NonNull<u32>>,
    needs_swap: AtomicBool,
    swap_count: AtomicU32,
}
```

- Processes write to the back buffer via `back_buffer_mut() -> WindowBackBuffer`.
- When content is ready, call `buffer.present()` → sets `needs_swap = true`.
- Compositor calls `buffer.try_swap()` which pointer-swaps back and front atomically
  if `needs_swap` is set.
- After swap, compositor reads from front buffer (`front_buffer_ptr()`).

`WindowBackBuffer` wraps a `&WindowBuffer` and exposes:
- `write_pixel(x, y, Rgba8888UNORM)`
- `write_pixel_unchecked` (no bounds check)
- `clear(Rgba8888UNORM)`
- `as_slice_mut() -> &mut [u32]` — direct pixel slice access
- Implements `embedded_graphics::DrawTarget<Color = Rgb888>`

`WindowPresentBuffer` exposes read-only access to the front buffer.

`Rect` helper struct with `contains`, `intersects`, `get_intersection_rect`,
`get_union_rect` methods.

## Compositor

File: `graphics/compositor.rs`

```rust
pub struct Compositor {
    framebuffer_width: u32, framebuffer_height: u32,
    next_window_id: AtomicU32,
    currently_focused_window: Mutex<WindowID>,
    free_window_ids: Mutex<Vec<WindowID>>,
    windows: RwLock<Vec<Window>>,
}
```

### create_window(width, height, x, y) -> (WindowID, Arc<WindowBuffer>)

Allocates a `WindowBuffer` (Arc), creates a Window at the given position with
z_index=0, pushes into `windows` Vec, sets as currently focused.

### compose(&self, framebuffer: &mut FrameBufferTarget)

Compositing pass:
1. Read-lock `windows`.
2. Collect visible windows, sort by z_index (ascending → back to front).
3. For each window: call `try_swap()` (swaps if pending), then blit visible region
   to the framebuffer.
4. Clipping: clamp source and destination rects to framebuffer bounds.
5. Copy row by row using `core::ptr::copy_nonoverlapping` (XRGB u32 words).

TODO: alpha blending is not implemented. No dirty region tracking.

### focus_window(window_id)

Sets z_index of the window to `max_z_index + 1`. If `max_z_index > 250`
(NORMALIZE_Z_INDEX_THRESHOLD), all visible windows are re-normalized starting from 0
to prevent overflow.

### destroy_window

Marks window `is_visible = false`, recycles ID into `free_window_ids`.

## Software Rendering Pipeline

Files: `graphics/pipeline.rs`, `graphics/renderer.rs`, `graphics/resources.rs`,
       `graphics/shaders.rs`, `graphics/transform.rs`

A fixed-function-style software rasterizer with pluggable vertex and pixel shaders.

### Vertex Types

```rust
pub struct Vertex2D {
    pub xyuv: f32x4,  // [x, y, u, v] — SIMD
}

pub struct Vertex3D {
    pub pos: f32x4,   // [x, y, z, w]
    pub uv: f32x4,    // [u, v, 0, 0]
    pub norm: f32x4,  // [nx, ny, nz, 0]
}
```

Both use `core::simd::f32x4` (portable SIMD).

### PipelineState

```rust
pub struct PipelineState {
    pub vs: Box<dyn VertexShader>,
    pub ps: Box<dyn PixelShader>,
    pub vertex_layout: VertexLayout,
    pub rasterizer_state: RasterizerState,
    pub blend_state: BlendState,
    pub render_mode: RenderMode,  // XY or XYZ
    pub depth_enabled: bool,
    pub depth_write: bool,
    pub depth_func: DepthFunc,
}
```

### Shader Traits

```rust
pub trait VertexShader: Send + Sync {
    fn run(&self, input: &VSIn, output: &mut VSOut, constants: &[ConstantBuffer]);
}
pub trait PixelShader: Send + Sync {
    fn run(&self, input: &mut PSIn);
}
```

`VSOut` carries clip-space position (`f32x4`) and up to 8 interpolated attributes
(split as `attributes: f32x4` + `extra: f32x4`).

`PSIn` gives the pixel shader:
- Interpolated attributes and extra
- Screen x/y
- Mutable slice over the render target (`&mut [u32]`)
- Slice of bound textures and constant buffers

### Resources

```rust
pub struct Texture { width: u32, height: u32, data: Vec<u32> }
pub struct ConstantBuffer { data: Vec<u8> }
pub struct RWBuffer { data: Vec<u8> }
```

Textures are RGBA (u32 per texel, `to_u32_rgba` from `Rgba8888UNORM`).

### RenderContext

```rust
pub struct RenderContext {
    textures: Vec<Texture>,
    cbuffers: Vec<ConstantBuffer>,
}
```

- `bind_texture(tex) -> slot_index`
- `bind_cbuffer(buf) -> slot_index`
- `begin_frame(back_buffer) -> RenderTarget`
- `clear(target, color)`

### Built-in Shaders (shaders.rs)

- `PassThroughVS`: passes vertex position and UV through unchanged
- `TextureSamplePS`: samples from texture slot 0 using bilinear interpolation (via the `micromath` crate for fast f32 ops)

### Transform (transform.rs)

Matrix math for 3D rendering:
- `Mat4x4` — row-major 4x4 f32 matrix
- `perspective_matrix(fov_y, aspect, near, far)`
- `view_matrix(eye, target, up)`
- `model_matrix(translation, rotation_y, scale)`
- Matrix multiply, vector-matrix multiply

Used by 3D demo in `main.rs` / `test_graphics.rs`.

## Theophe (Text Terminal)

File: `kernel/src/programs/theophe.rs`

A software text terminal that renders into a `WindowBackBuffer`. No real font, uses a
custom bitmap glyph rasterizer. Has `write_line`, `write_str`, `render` methods.

Used in `main.rs` to display CPU info in the first window.

## Graphics Usage in main.rs

Current flow in `main()`:
1. Get `FrameBufferTarget` from global.
2. `test_graphics::draw_shapes(fb)` — draws embedded-graphics primitives directly on fb.
3. Create `Compositor` (created twice in current code — second one overwrites the first;
   appears to be leftover/dead code with the first compositor block).
4. Create two windows via compositor.
5. `Theophe::new(window_buffer.back_buffer_mut())` → render CPU info text.
6. `compositor.focus_window(0)`, `compositor.compose(fb)`.
7. Render 3D test (textured cube) into window3's back buffer via `RenderContext`.
8. Enter `loop { if RENDER_SHADERS { ... } }`.

`RENDER_SHADERS: bool = false` — the render loop body is currently disabled. No frame
updates happen after initial composition.

## Known Issues / TODOs

- `text.rs` is empty — bitmap font renderer not implemented. Theophe (userspace) uses embedded-graphics instead.
- Alpha blending not implemented in compositor.
- No dirty region tracking in compositor (full blit every compose call).
- Double compositor creation in `main.rs` — first one is wasted (dead code block using local `Compositor::new`).
- Framebuffer pixel format assumed XRGB; mask fields from Limine not checked.
- `RENDER_SHADERS = false` means the 3D render loop is disabled.
- Compositor does not own the framebuffer (noted in TODO comments).

## Userspace Window Rendering (sys_map_window_buffer)

Userspace processes render into windows via shared memory:

1. `sys_create_window(w, h, x, y)` → kernel creates `WindowBuffer` on heap, returns `window_id`
2. `sys_map_window_buffer(window_id)` → kernel walks its own page table to find the physical
   frames of the back buffer, maps them USER_ACCESSIBLE into the process's address space at
   `USER_WINDOW_BUFFER_BASE + window_id * 8MB = 0x0000_0001_0000_0000 + id * 8MB`.
   Returns that user virtual address.
3. `sys_get_window_size(window_id)` → returns `(width << 32) | height`
4. Process writes pixels directly to the mapped address (XRGB8888 u32 per pixel)
5. `sys_present_window(window_id)` → calls `buffer.present()` (sets `needs_swap = true`)
6. Kernel compositor loop calls `compose()` → `try_swap()` pointer-swaps back/front,
   blits front buffer to framebuffer

IMPORTANT — physical address translation for window buffers:
The `WindowBuffer` pixel data is allocated with `alloc_zeroed` from the kernel heap at
`0xFFFF_8080_0000_0000`. These pages are NOT HHDM-mapped — they were individually
allocated by the frame allocator and mapped by `init_heap`. To find their physical
addresses, walk the kernel page table (`translate_user_virt_to_phys` with
`kernel_page_table_phys`). Do NOT use `vaddr - HHDM_OFFSET` for heap addresses.

Double-buffering: `WindowBuffer` has two pixel buffers (back and front). The user maps
and writes to the back buffer. `try_swap` pointer-swaps them atomically. After the swap,
the kernel `back_buffer` pointer changes but the user's mapped VA still points to the
original back buffer pages. The userspace side must manage both buffers and track which
is currently back. See `user/rustspace/src/bin/theophe.rs` for the reference impl.
