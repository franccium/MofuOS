# Terminal

Userspace terminal for `theophe` and future apps (`odys`). Inspired by `refterm`, rendered with `embedded_graphics`, ASCII only.

## Philosophy

Keep a large circular double mapped byte store, keep a small index of line starts, and separate ingest from render. The hardest work is copying bytes linearly and walking an index.

This terminal copies that:

- Byte storage is a double-mapped circular buffer (`Scrollback`). Writes and reads are linear `copy_nonoverlapping` slices even across the wrap, no split checks.

- Line storage is a circular index (`LineIndex`) of `first_pos/one_past_last_pos` absolute positions, not copied strings. The index is the only structure that knows where lines start and end.

- Ingest (`write_bytes_internal`) updates the index. Render (`redraw_all`) walks the index and draws only the visible window. No work is done on invisible history.

- Only ASCII `0x20..0x7E` plus `\n \r \t` is supported.

- Rendering itself is `embedded_graphics` `DrawTarget` (`UserSurface` `user/rustspace/src/gfx/surface.rs:12`). The terminal does not know about pixels, only `Text::draw`. Clearing uses `UserSurface::clear(Rgba8888UNORM::BLACK)` (`terminal/mod.rs:264`) with SIMD stores, not `PrimitiveStyle`.

These choices follow `docs/rust_coding_guidelines.md`: performance first, linear access, no `dyn` dispatch, `debug_assert!` for invariants, small declarative state.

## Core Concepts

**Absolute position.** `Scrollback.absolute_filled: u64` is monotonic and never wraps. Every byte ever written gets an absolute position. `LineMeta` stores `first_pos/one_past_last_pos` as absolute position. `relative: usize` is the physical write offset inside `view_size`. `is_in_buffer` (`scrollback.rs:42`) checks `absolute_filled - pos <= view_size`.

**Double alias.** `Scrollback` is built from `CircularBufferInfo` (`user/rustspace/src/lib.rs:134`) created by `sys_create_circular_buffer:472` (`kernel/src/data_structures/circular_buffer.rs:36`). The kernel maps `view_size/PAGE_SIZE` frames twice at `virt_base` and `virt_base+view_size` via `map_specific_frame` (`kernel/src/process/circular_buffer.rs:55`). Any valid range `abs..abs+len` within `view_size` is contiguous at `base + view_size + relative - (absolute_filled - abs)` (`scrollback.rs:99`) even when it straddles the logical wrap. `read_at:91` and `get_writable_slice:51` rely on this to avoid split copies.

**Line wrapping.** `Terminal.max_chars_per_line` is derived from `FontMetrics` and `available_width` (`terminal/mod.rs:70`). `append_chunk_with_wrap:168` fills the current line, finds the last `b' '` within `space` to word-wrap, writes the head, `line_feed`s, and continues with the remainder. Lines longer than `SPLIT_LINE_AT=4096` (`line_index.rs:1`) are force-split (`terminal/mod.rs:187`).

## Components

**Scrollback** `user/rustspace/src/terminal/scrollback.rs:5`
- `base: *mut u8, view_size, total_size, relative, absolute_filled`
- `new(info)`, `empty()`, `write_bytes(&[u8]) ->absolute position, `read_atabsolute position max_len) -> &[u8]`, `remove_last_byte`, `get_writable_slice`, `commit_bytes`
- No allocation after `new`; `write_bytes` is a tight copy loop.

**LineIndex** `user/rustspace/src/terminal/line_index.rs:27`
- `lines: Vec<LineMeta>, current, count, max`
- `LineMeta{first_pos, one_past_last_pos}` `line_index.rs:3` with `len/is_empty`
- Circular `max=4096` (`terminal/mod.rs:28`) – 64 KiB index. `line_feed:90` advances `current=(current+1)%max`, `update_end:86`, `force_line_feed_if_needed:107`, `logical_line_for_index:69` translates logical 0..count to physical slot `(start+idx)%max` where `start` is `(current+1)%max` when full.

**Terminal** `user/rustspace/src/terminal/mod.rs:54`
- `Terminal<D: DrawTarget<Color=Rgb888>> {draw_target, window_id, scrollback, lines, max_chars_per_line, viewport_rows, font_metrics, last_command_buf, input_state, scroll_offset}`
- `FontMetrics:30` `from_font` for `FONT_8X13` (8x13), `get_max_chars_per_line:49`
- `MARGIN_LEFT=4, MARGIN_TOP=16, LINE_SPACING=15` (`mod.rs:17`)
- `CHARACTER_STYLE` white on black (`mod.rs:21`), `TEXT_STYLE` left aligned

**Viewport** `mod.rs:114`
- `viewport_rows = (height - MARGIN_TOP)/LINE_SPACING` clamped to `>=1` via `max` (`mod.rs:79,120`) per style guide (no `if` clamp).
- `recompute_viewport:113` recomputes `max_chars_per_line` and `viewport_rows` and calls `clamp_scroll:129`.
- `clamp_scroll`, `scroll_up/down_lines`, `scroll_up/down_page:134` maintain `scroll_offset` as distance from bottom (`0=bottom`, `max=count-viewport_rows`). `redraw_all:269` computes `visible=min(rows,total)`, `max_scroll=total-visible`, `offset=min(scroll_offset,max_scroll)`, `start=total-visible-offset` and draws `visible` lines via `scrollback.read_at`.

**Input** `user/rustspace/src/terminal/input.rs:1`
- Shared between `theophe` and `odys`. `InputState{command_mode:bool}:38`, `InputAction:15` (`ToggleCommandMode`, `Scroll{Up/Down,Line/Page}`, `Char,Backspace,Enter,ArrowUp/Down/...`), `translate(event, state):58`.
- `KeyEvent` `LControl/RControl Pressed` toggles `command_mode`. While `command_mode`, `Q/E` → `Scroll Page Up/Down`, `A/D` → `Line Up/Down` via both `KeyCode` (`input.rs:72`) and `CharEvent` lowercased `q/e/a/d` (`input.rs:125`). Other typing in command mode returns `Ignore`. Outside command mode, `Backspace/Enter/Char` pass through.
- `Terminal` holds `input_state:65` and delegates `handle_event:553` → `translate` → `apply_action:495` (single static dispatch, no `dyn`). `odys` can reuse `translate` and map `Scroll` to its own list/file-preview offsets without inheriting `Terminal` (not append-only).

## Input Flow

```
InputEvent (EventBuffer 0x0000_0007_0000_0000: lib.rs:918)
  -> input::translate -> InputAction
  -> Terminal::apply_action: Scroll -> scroll_offset, Char -> write_bytes_internal, Enter -> capture_last_command + line_feed + execute_command
  -> needs_redraw=true -> render -> redraw_all -> UserSurface draw
  -> sys_present_window -> compositor swap
```

`Ctrl` toggles command mode (`input.rs:67`). In command mode `Q`=page up, `E`=page down, `A`=line up, `D`=line down (`terminal/mod.rs:134`). Typing while scrolled jumps to bottom (`mod.rs:529`).

## Rendering

`redraw_all:269` clears with `surface.clear` (SIMD `_mm_storeu_si128` in `gfx/surface.rs:62`), then for each visible logical line fetches `bytes = scrollback.read_at(meta.first_pos, meta.len())` and draws `Text::with_text_style(s, Point::new(MARGIN_LEFT, MARGIN_TOP + vis_idx*LINE_SPACING), CHARACTER_STYLE, TEXT_STYLE)` (`mod.rs:292`). No per-cell glyph cache, no `dyn` shader.

## Allocation

Userspace heap is `Arena` `user/rustspace/src/lib.rs:556` – bump with size-class free lists (`BLOCK_SIZES [8..4096]:560`, `SLAB_SIZE 16KiB:558`). `alloc:641` tries `small_heads[idx]` pop else bumps `block_size`; large `>4096` uses `large_head` first-fit with split. `dealloc:775` pushes to `small_heads` or `large_head`. This makes `Vec`/`String` reuse after `drop` (previous `flow` leaked because `dealloc` was no-op). `Terminal` itself avoids hot-path alloc: `flow_demo:333` uses stack `line_buf:[u8;128]` and `msg_buf:[u8;64]`, `Scrollback` and `LineIndex` are preallocated.

## Commands (theophe)

`execute_command:423` dispatches `last_command_buf` trimmed: `clear` → `clear:255` (resets `LineIndex` at current absolute position and `scroll_offset=0`), `flow [n]` → `flow_demo:333` (random slices of `utils/lorem_ipsum.txt:309` via `include_bytes!` + `tsc_read` xorshift), `deb`, `odys` (`sys_create_process:464`).

## Reuse for odys

`odys` `user/rustspace/src/bin/odys.rs:80` is not append-only (`entries:Vec<FileEntry>`, `file_content:String`, selectable list). It should not embed `Terminal`. Instead share `term_core` primitives:

- Keep `odys::Odys` state, replace `file_content:String` with `Scrollback+LineIndex` for preview to get wrapped scroll via same `viewport` math.
- Reuse `terminal/input.rs` – `odys` calls `translate` and maps `Scroll` to `selected_idx/scroll_offset` (already `odys.rs:272` `move_selection`) and `ToggleCommandMode` to its own mode if desired.
- Reuse `FontMetrics`, `MARGIN_*`, `LINE_SPACING`, `clear_screen` via `UserSurface`.

This keeps `Terminal` append-only and `Odys` random-access, both using the same scrollback/viewport/input core without inheritance.

## File Map

- `user/rustspace/src/terminal/mod.rs` – shell terminal, append log
- `user/rustspace/src/terminal/scrollback.rs` – circular double-mapped store
- `user/rustspace/src/terminal/line_index.rs` – circular line index
- `user/rustspace/src/terminal/input.rs` – shared `InputState` + `translate`
- `user/rustspace/src/gfx/surface.rs` – `UserSurface` double-buffered `*mut u32`
- `user/rustspace/utils/lorem_ipsum.txt` – flow demo source
- `kernel/src/data_structures/circular_buffer.rs` – `CircularBuffer::map_for_user` double mapping
- `kernel/src/process/syscall.rs:600` – `CreateCircularBuffer` syscall
