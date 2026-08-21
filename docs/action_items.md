# MofuOS — Action Items: Detailed Issue Analysis and Remediation Plan

> Source: audit of `docs/bugs.md`, `docs/issues.md`, `docs/memory.md`, `docs/process.md`, `docs/hardware.md`, `docs/graphics.md`, `docs/filesystem/filesystem.md`, `docs/conventions.md` plus direct source inspection.
> Last updated: 2026-08-21

This document converts every known bug/issue/TODO into an **actionable item** with: root cause on x86_64, concrete failure mode in QEMU, severity, fix sketch, and verification.

Priority definitions:
- **P0 Blocker** — will OOM, triple-fault, or silently corrupt data under normal use. Fix before any new feature.
- **P1 High** — correctness or portability violation; manifests on real hardware or under sustained load.
- **P2 Medium** — performance, scalability, or developer-velocity debt.
- **P3 Low** — cosmetic / tech-debt / thesis-polish.

---

## P0 — Blockers (fix next)

### A0-1: No physical frame reclamation — monotonic OOM (ISSUE-M1 + BUG-05)

**Files:** `kernel/src/memory/memory.rs: MemoryMapFrameAllocator`, `kernel/src/memory/usermem.rs: map_virt_mem_region`, `kernel/src/process/process.rs: Process::create_with_elf`, `kernel/src/process/process_mem.rs`, `kernel/src/process/syscall.rs: sys_exit/sys_allocate`

**Root cause:**
`MemoryMapFrameAllocator` is a pure bump allocator over Limine `MEMMAP_USABLE` entries. No `deallocate_frame`, no free list. `map_virt_mem_region` allocates a frame *before* `map_to`; on `Err(PageAlreadyMapped)` the frame is leaked (`bugs.md: BUG-05`). `ProcessMemoryLayout::new` allocates a fresh PML4 frame per process plus one frame per 4 KiB page of every PT_LOAD segment and stack. None are freed on `terminate_process`. User heap `grow_heap` similarly never shrinks.

On x86_64 every user page table level (PML4, PDPT, PD, PT) also consumes frames via the `FrameAllocator` passed to `map_to` — those intermediate tables are also never reclaimed.

**Failure mode in QEMU:**
With `-m 2G`, ~500 MB is usable after Limine + kernel image + heap (16 MB at `0xFFFF_8080_0000_0000`). One `ping` ELF (~50 pages) costs ~60 frames. 10k create/exit cycles exhaust RAM; allocator returns `None`, `create_with_elf` panics or returns `MapToError` which currently propagates as kernel panic → `exit_qemu(Failed)` port `0xF4`. Even without churn, `BUG-05` leaks 1–2 frames per multi-segment ELF (common when `.text` and `.rodata` share a page boundary) — low per process but unbounded over time.

**Fix sketch:**
1. Add `fn deallocate_frame(&mut self, frame: PhysFrame)` to `MemoryMapFrameAllocator`. Simplest: intrusive free list of `PhysFrame` — push freed frames onto `Vec<PhysFrame>` or linked list via HHDM alias (`frame.start_address() + hhdm_offset` as `*mut FreeNode`). `allocate_frame` pops free list first, else bumps.
2. Fix `BUG-05` leak: allocate lazily — call `map_to` with a closure that allocates, or on `PageAlreadyMapped` push `phys_frame` back via `deallocate_frame`.
3. On `terminate_process(cascade)`: walk `ProcessMemoryLayout.mapped_regions` + stack range + PML4, unmap each page (`unmap` + `deallocate_frame` for data frame + `tlb flush`), then walk and free intermediate tables. Requires tracking which tables were allocated — easiest is to enumerate present entries recursively from the process PML4 and free leaf frames + tables.
4. Add `Drop` or explicit `free_address_space(pml4_phys)` that walks 256 user PML4 entries.

**Verification:**
- Unit test with `MockDisk` + in-memory frame allocator: loop `create_with_elf` → `terminate_process` 1000x, assert `free_frame_count` returns to baseline.
- QEMU log: print `FRAME_ALLOCATOR.lock().free_count()` periodically via `serial_println_core!`.

**Effort:** 2–3 days. Prerequisite for any real multi-process or FS workload.

---

### A0-3: File cache `directory_children` never populated — `reserve_cache`/`evict_directory` are dead (filesystem.md:234 Known Issue)

**Files:** `kernel/src/filesystem/file_cache.rs: FileCache`, `kernel/src/filesystem/sirius.rs: CachedDriver`

**Root cause:**
Cache eviction is LRU-weighted by `CacheImportance`. `reserve_cache(path, importance)` stores hint under directory `node_id`. `get_effective_importance(file_node_id)` incorrectly looks up file's own `node_id` in `directory_hints` (which holds directory IDs). `register_file_in_directory` is never called, so `directory_children: HashMap<dir_id, Vec<file_id>>` stays empty, `evict_directory(path)` is a no-op. File importance always falls back to `Normal`.

The `FileNodeHandle` packing `bits 55-32 = parent_cluster` (`filesystem.md:245`) already carries parent identity — decode it on `load_file_into_cache` and call `register_file_in_directory(parent_id, file_id)`.

**Failure mode:**
On a cached workload (pin 100 files), eviction picks victim by `(MAX - priority)*age` but all have same priority — pure LRU, not workload-aware. `reserve_cache("/bin", Critical)` has zero effect; `evict_directory("/tmp")` after `create_file` burst does not free anything → cache fills to `FS_CACHE_SIZE=64MB` then thrashes, evicting hot files.

**Fix sketch:**
```rust
// in FileCache::load_file(node_id)
let (_, parent_cluster, _) = decode_node_id(node_id);
let parent_id = encode_node_id_for_cluster(parent_cluster); // or store parent_id explicitly
self.register_file_in_directory(parent_id, node_id);
```
Fix `get_effective_importance` to lookup `parent_id`, not `file_id`. Add `debug_assert!(directory_children.contains_key(...))` in tests.

**Verification:** `fs_test` add suite: `reserve_cache("/a", Critical)`, create 10 files under `/a` and 10 under `/b`, fill cache, assert `/a` files survive eviction.

**Effort:** 2 hours.

---

## P1 — High (correctness / hardware portability)

### A1-1: FAT32 mirror FAT not updated — spec violation, fsck failure (ISSUE-F6)

**File:** `kernel/src/filesystem/fat32/mod.rs: write_fat_entry`

**Root cause:** `BootSector.num_fats == 2` but `write_fat_entry` only writes to `fat_start_sector`. FAT2 is stale. Linux `fsck.fat -v` reports `FATs differ`. On power loss after FAT1 write but before data, FAT2 replay would resurrect old chain → cross-linked clusters.

**Fix:** After `new_entry = (existing & 0xF000_0000) | (value & 0x0FFF_FFFF)`, write to both `fat_start + entry_offset` and `fat_start + fat_size_32*bytes_per_sector + entry_offset` if `num_fats >=2`. Batch if multiple entries in same sector.

**Verification:** `cargo xtask run` → inside `fs_test` write file, `qemu-img` dump, `fsck.fat -n ata_disk.img` zero errors; hex-compare FAT1/FAT2 sectors.

---

### A1-2: Framebuffer hardcodes XRGB — wrong colors on real hardware (BUG-06, ISSUE-G?)

**File:** `kernel/src/graphics/framebuffer.rs: write_pixel`

**Root cause:** Ignores Limine `red_mask_shift/size` etc. QEMU presents `XRGB 8:8:8` (`R shift 16`). Real UEFI GOP can be `BGRX` (`R shift 0, B shift 16`) or `BGR565`. Hardcoded `(r<<16)|(g<<8)|b` swaps channels.

**Fix:** At `init_framebuffer(fb: &Framebuffer)`, store `red_shift = fb.red_mask_shift` etc. In `write_pixel`:
```rust
let color = ((r as u32) << red_shift) | ((g as u32) << green_shift) | ((b as u32) << blue_shift);
```
Handle `_mask_size !=8` by scaling (e.g., 5/6-bit). Add `debug_assert!(red_size==8)` path for now.

**Verification:** Boot with `qemu -vga std` vs `-vga qxl` and compare; unit test with mock `Framebuffer` BGRX.

---

### A1-3: `IdentityAcpiHandler` IO stubs `todo!()` — panics on any AML/PCI path (BUG-07 / ISSUE-C2)

**File:** `kernel/src/memory/memory.rs: IdentityAcpiHandler`

**Root cause:** Only `map_physical_region` implemented. `acpi` crate's table parse path currently avoids IO, but any `AcpiHandler::read_u32` for MADT X2APIC or future `AcpiTable` will `panic → exit_qemu(Failed)` with no diagnostic on serial.

**Fix (choose one):**
- Implement via HHDM: `unsafe { ((paddr as u64 + hhdm_offset) as *const u32).read_volatile() }` for mem, via `port read` for `read_io_*` (0xCF8/0xCFC PCI).
- Or rename to `TableParseOnlyAcpiHandler`, document `#[cfg]` that full AML is unsupported, and make stubs return `0` / no-op with `serial_println!("acpi IO stub called")` instead of `todo!()` so it degrades gracefully.

**Effort:** 3 hours for HHDM path.

---

### A1-4: Tight syscall loop starves preemption (ISSUE-P9)

**File:** `kernel/src/process/syscall.rs: handle_syscall_inner`, `kernel/src/interrupts.rs: timer_interrupt_handler`

**Root cause:** `SFMASK=1<<9` masks `IF` on `SYSCALL` entry (Intel `SYSRET` SS fix requires it, `bugs.md: BUG-09`). Interrupts are `sti` at top of `handle_syscall_inner`, so handler is preemptible, but tight user loop `syscall; sysret; syscall` has only ~10 instructions in Ring3 between. With `TIMER_TICK_INTERVAL_MS=10ms` (100 Hz), probability of timer firing in that window is tiny → one `sys_write`-heavy process hogs core 1 until completion, `ping` vs `ping` interleaving fails.

**Fix sketch:**
- Keep `SFMASK=IF` (required for SS safety) + `sti` early is already correct. Add explicit preemption check on syscall exit: before `sysretq`, test `SCHEDULER.lock().ready_count_on_core(core_id) >1` and if true call `return_to_scheduler` path (save `user_rip` to resume at `sysret` target). This makes every syscall a voluntary preemption point without changing tick rate.
- Alternative: `sys_yield` (998) already exists; make `user/rustspace` `sys_write` wrapper call `yield` every N writes in stress tests.
- Do NOT lower tick below 1ms (adds overhead, x86_64 `TSC_DEADLINE` re-arm cost).

**Verification:** `PING_ELF` tight write loop + second `ping_yield` process, assert interleaved `userspace_pid_*.txt` timestamps within 20ms.

---

### A1-5: Wire type fragility + incomplete user-pointer validation

**Files:** `kernel/src/process/syscall.rs: DirEntryFlat, StatFlat`, `user/rustspace/src/lib.rs: FS_NAME_LEN`, `kernel/src/process/syscall.rs: validate_user_ptr`

**Root cause:**
- `FS_NAME_LEN` (rustspace) vs `DIR_ENTRY_NAME_LEN` (kernel) must be identical; divergence silently breaks `#[repr(C)]` layout. No `static_assert!`.
- `validate_user_ptr(ptr,len)` checks `null`, `>=0x0000_8000_0000_0000`, wrap-around, but not that pages are mapped `USER_ACCESSIBLE` or that `len` does not cross an unmapped guard. `sys_read_file`/`sys_write_file` then `copy_nonoverlapping` via HHDM translation which `#PF` in kernel mode → double fault.

**Fix:**
- Add `const_assert!(core::mem::size_of::<DirEntryFlat>() == 32)` (or actual) in both crates, and cross-crate version check via build script or shared `common` crate.
- Extend validation: `validate_user_slice(ptr,len, writable: bool)` that walks page tables via `translate_user_virt_to_phys` per page (like ELF loader does) and checks `USER_ACCESSIBLE` + `WRITABLE` if needed. Return `EFAULT` (`u64::MAX` currently) on fail.

---

### A1-6: Single-core userspace bottleneck (ISSUE-P5 + ISSUE-H3)

**Files:** `kernel/src/process/core_pool.rs: available_cores &= !1`, `xtask/src/main.rs: run_iso QEMUFLAGS -smp cores=3`, `kernel/src/lib.rs: MAX_CORES=16`

**Root cause:** Core 0 (BSP) runs `main()` compositor loop + `hlt`; all userspace pinned to core 1. With `cores=2`, only one runnable process at a time → scheduler's 8-level priority + `Dequeue` is untested for concurrency. `MAX_CORES=16` bitmap (`u64`) supports it, but `init_cpu_infos`, `PER_CORE_*` arrays, `log_splitter.py: MAX_CORES=4` vs `lib.rs:16` mismatch (AGENTS.md warns keep in sync).

**Fix:**
- Test matrix: `QEMUFLAGS="-smp 4" cargo xtask run` (requires `log_splitter.py:22 MAX_CORES=16`). Verify `CORE_POOL` assigns least-loaded across 3 APs, `SCHEDULER.per_core.len()==core_count`.
- Long term: allow BSP to also run userspace when compositor idle, or dedicate core 0 to `SCHEDULER` + `Sirius` lock holder to avoid `SCHEDULER` contention on all cores.

---

### A1-7: Heap translation bug class — guard against `vaddr - HHDM_OFFSET` misuse (BUG-11 pattern)

**File:** `kernel/src/memory/usermem.rs: translate_kernel_heap_virt_to_phys` (already fixed), but pattern recurs.

**Root cause:** Kernel heap at `0xFFFF_8080_0000_0000` is not HHDM identity-mapped; it is demand-mapped via `FrameAllocator`. Any new code that does `phys = virt - HHDM_OFFSET` for heap/buffer addresses will reintroduce `BUG-11` (user writes to garbage phys `~512GB`). `graphics/window.rs: WindowBuffer` back buffers are also heap-allocated.

**Fix:**
- Delete helper `translate_kernel_heap_virt_to_phys` entirely; force all translations through `translate_user_virt_to_phys(kernel_pml4_phys, vaddr)` which walks page tables correctly.
- Add `#[deny(clippy::manual_sub)]` or comment lint: grep CI for `HHDM_OFFSET` subtraction.

---

## P2 — Medium (performance, scalability, correctness hardening)

### A2-1: Paging and address-space hygiene

- **PML4 copy 256-511** correctness: verify `USER_MEMORY_MANAGER.kernel_page_table_phys` is `Cr3::read().0` at boot after `init_offset_page_table` and after LAPIC mapping. If LAPIC mapped after `USER_MEMORY_MANAGER` init, new address spaces miss it → AP LAPIC access `#PF`. Current `boot.rs: init_offset_page_table → map LAPIC → init_user_mem_mgr` order is correct; add `debug_assert!(pml4[lapic_pml4_idx].is_present())` in `allocate_new_address_space`.
- **2 MiB huge pages**: `translate_user_virt_to_phys` already handles 2 MiB, but `map_virt_mem_region` never allocates them. Keep 4 KiB for correctness, evaluate huge pages only after frame allocator is stable.
- **TLB shootdown:** `flush()` is local only. On `smp>2`, `unmap` on one core needs IPI shootdown to others sharing same `PML4`. Not needed now (per-process PML4 private), but kernel global unmap (future `kfree`) will need it.

### A2-2: Scheduler and locking

- **Lock ordering** `conventions.md: CORE_POOL → PROCESS_MANAGER → SCHEDULER → FRAME_ALLOCATOR → SERIAL` is documented but not enforced. Add `lock_order` debug asserts or `try_lock` with ordering check.
- **Never hold `PROCESS_MANAGER` across `jump_to_userspace`** (`conventions.md: Unsafe Rules 3-4`) — current `run_on_core_loop` correctly `drop`s guard before `jmp`. Keep that pattern; add `#[must_not_hold]` comment on `PROCESS_MANAGER` lock.
- **`SCHEDULER.on_timer_tick` empty** `ISSUE-P1` — repurpose for time-slice accounting (`process_ticks[pid]++`) when preemption is syscall-driven as in A1-4.
- **`Dequeue<PID>` O(n) scans** `process/process.rs: get_process` linear — fine at <100 processes, but switch to `HashMap<PID, index>` or `Vec<Option<Process>>` indexed by PID if scaling beyond.

### A2-3: Filesystem performance and correctness

- **`write_direntry` rewrites whole directory chain** per 32B entry — `fat32/mod.rs: write_direntry`. Batch updates: hold directory cluster buffer, patch in place, write once.
- **`find_free_cluster` linear FAT scan** — use FSInfo sector (`free_cluster_count`, `next_free_cluster` hints) at `BPB.fs_info_sector`. Update on alloc/free.
- **Timestamps not updated** on `write_file/create` — `fat_time_to_unix_timestamp` exists but never written. Update `DirEntry.modified_time` on each write.
- **`sirius.rs: init_filesystem_ata` always 64 MB cache** — make `cache_size` tunable via Limine cmdline or `BootInfo` for QEMU `-m` small.

### A2-4: Graphics

- **Double `Compositor::new` in `main.rs` `ISSUE-G5`** — delete first block; first window leaked. Verify second compositor owns both windows expected by `focus_window`.
- **Dirty region tracking `ISSUE-G3`** — add `Rect dirty: Option<Rect>` per `Window`, `compose()` only blits dirty union. Use `get_intersection_rect` already in `window.rs`.
- **Alpha blending `ISSUE-G2`** — `compositor.compose` currently `copy_nonoverlapping`. Add `blend: (src_a * src_rgb + (255-src_a)*dst_rgb)/255` when `Rgba8888UNORM.a != 255`. Benchmark — software blend is expensive; gate via `BlendState`.
- **Compositor ownership of `FrameBufferTarget` `ISSUE-G6`** — change `compose(&mut FrameBuffer)` to `Compositor { fb: Mutex<FrameBufferTarget> }` + `fn compose(&self)` for autonomous background thread.
- **Input routing `ISSUE-G7`** — route `keyboard` IDT handler scancodes to focused `WindowID` via `currently_focused_window` + event queue per process (future `sys_read`).

### A2-5: SMP and boot

- **AP trampoline `0x8000` `ISSUE-H2`** — if Limine `bootstrap` is canonical, remove custom `setup_ap_trampoline_mapping` and `ap_trampoline.S` binary blob to reduce attack surface and boot complexity. Keep only if native IPI boot is planned.
- **X2APIC `ISSUE-H1`** — when `CPUID(1).ECX[21]` set, enable via `MSR APIC_BASE` bit 10 + use `MSR 0x800-0x8FF` instead of MMIO. Faster, no `LAPIC_VIRT_BASE` alias.
- **4+ core testing:** fix `scripts/log_splitter.py:22 MAX_CORES` vs `lib.rs:13 MAX_CORES` mismatch; test `MAX_CORES=16` with `QEMUFLAGS="-smp 16"`; watch `CORE_POOL.available_cores: u64` bitmap limit `const_assert!(MAX_CORES<=64)`.

### A2-6: Userspace and ELF

- **Baked `TEST_ELF` `ISSUE-P8`** — connect `Sirius` + `elf_loader::ElfLoadInfo::from_elf_data` to `sys_create_process(path)`. Need `sys_create_process` (0) to accept `path_ptr/len`, read file via `Sirius::read_file`, then `Process::create_with_elf`. This unblocks multi-binary without recompile.
- **`create_process` dead path `ISSUE-P7`** — either delete or implement correctly with `entry_point/stack_top/pml4` from ELF, not zeros.
- **Rust userspace `user/rustspace`**: `Arena` bump via `sys_allocate(5)` is correct; add `free` as no-op with leak tracking. `embedded-graphics` via `UserSurface` over mapped back buffer is solid.
- **Window double-buffer userspace contract** `graphics.md:310` — user maps back buffer at `USER_WINDOW_BUFFER_BASE + id*8MB`. After `try_swap` kernel swaps pointers but user VA still points to old back. `theophe.rs` handles it; document that second `sys_map_window_buffer` after `present` is not needed — both buffers are shared? Clarify.

---

## P3 — Low / Tech Debt

### A3-1: Code quality and CI

- **Remove `#![allow(warnings,unused)]` `ISSUE-C1`**: Replace with crate-level `#[allow(dead_code)]` per module + `#[deny(unused_must_use)]` for `Result`. Run `cargo +nightly clippy --target x86_64-unknown-none -Z build-std` in CI.
- **No `no_std` test harness `ISSUE-C3`**: Promote `kernel/src/tests_exp/` to `#[cfg(test)]` harness that runs under `qemu -serial stdio -device isa-debug-exit` and asserts `QemuExitCode::Success 0x10`. Add `cargo xtask clippy` target.
- **Formatting:** `user/rustspace` already `cargo fmt` in Makefile; add `kernel` fmt check.
- **Build deps:** `build.rs` `cc/ld/objcopy` — pin `llvm-tools-preview` via `rust-toolchain.toml` already, but check `llvm-ar/ld.lld` host version drift.

### A3-2: Documentation sync

- `docs/*` is source of truth, but `notes/agents/*` duplicates it and `start_session.md` still points to `notes/agents/overview.md`. Consolidate: make `docs/` canonical, `notes/agents/` symlink or redirect stub. Otherwise drift (already: `MAX_CORES 4 vs 16`, `cores=2 vs 3`).
- `reading_asm_with_addresses.md` objdump recipes are gold — keep and add `llvm-objdump --disassemble --no-show-raw-insn` alias.

### A3-3: Minor FS and HW polish

- `FAT32 8.3 only ISSUE-F5` + `set_filename` stem<=8 ext<=3 check — keep for thesis, add `InvalidFilename` propagation to `sys_create_file` already does.
- `FSInfo` hints unused, `write_fat_entry` mirror `ISSUE-F6` already P1.
- `text.rs` empty `ISSUE-G1` — `Theophe` covers it; decide to delete file or implement `bitmap_font` primitive.
- `RENDER_SHADERS=false ISSUE-G4` — gate 3D pipeline behind `lib.rs: RUN_SHADERS` const, document perf cost.

---


## Verification Checklist (per item)

- [ ] QEMU `cargo xtask run` boots, `logs/core_*.txt` shows `[Core N | Xus]` prefixes, no `#PF`/`#GP`
- [ ] `QEMUFLAGS="-smp 4" cargo xtask run` with 4 `ping` processes interleaves
- [ ] `fs_test` 6 suites pass on `ata_disk.img`; `fsck.fat -n` clean
- [ ] `cargo +nightly clippy` zero warnings (after A3-1)
- [ ] `objdump` of `run_on_core_loop` shows `mov %rcx,%cr3` not `mov %rax,%cr3` (BUG-02 regression guard)
- [ ] Frame free count stable over 1000 process cycles (A0-1)

