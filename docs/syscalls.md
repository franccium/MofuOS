# MofuOS — Syscall Reference & Ring 3 -> Ring 0 Context Switch

> Canonical reference for all syscalls. Source of truth is `kernel/src/process/syscall.rs:66-108` (`SyscallNumber`) and `user/rustspace/src/lib.rs:14-48` (wrappers). This doc documents the hardware mechanism and every implemented number.
> Last updated: 2026-08-21 — covers File IO (20-28), File Cache (30-35), `CreateCircularBuffer (600)`, `GetWindowInfo (16)`, `GetCpuInfo (970)`.

---
## 1. Hardware Mechanism: SYSCALL / SYSRET

MofuOS uses **64-bit `SYSCALL`/`SYSRET`** (not `int 0x80` or `sysenter`). One `syscall` instruction is ~30 cycles; no IDT dispatch.

### 1.1 MSR Configuration (per AP core, `syscall.rs:init_syscall`)

Called from `cpuinfo.rs:ap_core_from_limine_entry_point` and BSP `kmain` via `init_syscall()`:

```rust
EFER.SCE = 1                 // MSR 0xC0000080, bit 0 — enable SYSCALL
STAR  = (0x10 << 48) | (0x08 << 32)  // MSR 0xC0000081
  // 63:48 = user CS base for SYSRET = 0x10
  // 47:32 = kernel CS base for SYSCALL = 0x08
  // SYSRET then does: CS = (STAR[63:48]+16)|3 = 0x23, SS = (STAR[63:48]+8)|3 = 0x1B
LSTAR = syscall_handler as u64       // MSR 0xC0000082 — Ring0 entry RIP
SFMASK = 1 << 9                      // MSR 0xC0000084 — mask IF on entry (see 1.4)
KERNEL_GS_BASE = &PER_CORE_SYSCALL[core_id] as u64 // MSR 0xC0000102
```

GDT layout `gdt.rs:5-6` (per-core):

| Slot | Sel | Desc |
|------|-----|------|
| 1 | 0x08 | kernel code (DPL0, L=1) |
| 2 | 0x10 | kernel data (DPL0) |
| 3 | 0x18 | user data (DPL3) |
| 4 | 0x20 | user code (DPL3, L=1) |
| 5 | 0x28 | TSS (2 slots) |

### 1.2 CPU actions on `SYSCALL` (Ring3 -> Ring0)

Intel SDM Vol.2 `SYSCALL` (64-bit):

1. `RCX = RIP` (next RIP saved), `R11 = RFLAGS` saved.
2. `RFLAGS &= ~SFMASK` — here clears `IF` (bit 9) so interrupts masked on entry.
3. `CPL = 0`, `CS = STAR[47:32]` (=0x08), `SS = STAR[47:32]+8` (=0x10), `RIP = LSTAR`.
4. **No stack switch** — `RSP` still = user RSP. Kernel must switch manually (see 1.3).
5. `RCX/R11` are clobbered; user must not expect them preserved. `RAX` holds syscall number on entry, return value on exit.

Userspace calling convention `user/rustspace/src/lib.rs:224-244` (`syscall6`):

```
rax = num
rdi = arg1, rsi = arg2, rdx = arg3, r10 = arg4, r8 = arg5, r9 = arg6
# r10 not rcx because SYSCALL clobbers rcx
asm: mov r10, rcx; syscall
ret: rax = return value, rcx/r11/r10 clobbered
```

### 1.3 Kernel entry: `syscall_handler` naked asm (`syscall.rs:154-206`)

Per-core 64 KiB stack `PER_CORE_SYSCALL[core_id]: PerCoreSyscallData` — `stack_top` is first field (`#[repr(C)]`), cache-line `align(64)`, pointed to by `KERNEL_GS_BASE`. Hot path uses `swapgs` + `gs:0`:

```asm
swapgs              // GS: user -> kernel (now GS points to PerCoreSyscallData)
mov r15, rsp        // save user RSP in r15 (callee-saved, survives handler)
mov rsp, gs:0       // load this core's kernel stack top (PER_CORE_SYSCALL[core].stack_top)
swapgs              // restore user GS (done with gs:0, no further GS use)

push r15            // user_rsp
push rcx            // user_rip (CPU saved)
push r11            // rflags (CPU saved)
push rax            // syscall_num  — BUG-08 fix: must not be omitted
push rdi; push rsi; push rdx; push r10; push r8; push r9  // args
push rbx; push rbp; push r12; push r13; push r14           // callee-saved

mov rdi, rsp        // rdi = *mut SyscallFrame
call handle_syscall_inner  // Rust, interrupts re-enabled inside (sti)

pop r14; pop r13; pop r12; pop rbp; pop rbx
pop r9; pop r8; pop r10; pop rdx; pop rsi; pop rdi
add rsp, 8          // skip syscall_num slot — rax already holds return value
pop r11; pop rcx; pop r15
mov rsp, r15        // restore user RSP
sysretq             // RCX->RIP, R11->RFLAGS, CS=0x23 SS=0x1B, CPL=3
```

`SyscallFrame` `syscall.rs:133-151` (`#[repr(C)]`, lowest address first):

```
r14, r13, r12, rbp, rbx          // callee-saved
arg6(r9), arg5(r8), arg4(r10), arg3(rdx), arg2(rsi), arg1(rdi)
syscall_num(rax)
rflags(r11), user_rip(rcx), user_rsp(r15)
```

Pop order skips `syscall_num` with `add rsp,8` so `rax` (return value from `handle_syscall_inner`) is not overwritten — `process.md:403-411` documents this.

### 1.4 Interrupt safety & Intel SYSRET SS bug (BUG-09)

**SFMASK = 1<<9 (IF).** On `SYSCALL` entry `IF` is cleared, so timer (`APIC TSC-Deadline`, 10ms, vector 0x20) cannot fire while handler runs with kernel `SS=0x10` but still on kernel stack. At top of `handle_syscall_inner:211` the kernel does `sti` — long syscalls become preemptible again. Before `return_to_scheduler()` in `Yield`/`Exit` it does `cli` to avoid timer firing during `mov rsp,[slot]; ret` unwind.

**Intel SYSRET SS bug:** Intel `SYSRETQ` sets `SS = STAR[63:48]+8` without ORing `RPL=3`, leaving `SS=0x18` (`RPL=0`) in userspace. `qemu64` follows Intel. User then runs with `SS=0x18`. When timer fires in Ring3 the interrupt frame captures `SS=0x18`; compiler `iretq` back to Ring3 faults `#GP` because `iretq` requires `SS.RPL == CS.RPL (3)`. Fix `interrupts.rs:timer_interrupt_handler`:

- `SFMASK`+`sti` as above so timer never fires with kernel SS.
- If `stack_frame.code_segment.rpl()==Ring3` and work waiting → `return_to_scheduler()` (bypasses `iretq` entirely).
- If Ring3 but nothing waiting → patch `*(stack_frame_ptr+4) = 0x1B` (SS slot at offset +32: `RIP+0, CS+8, RFLAGS+16, RSP+24, SS+32`) before `iretq`.

AMD does OR `RPL=3`; bug is Intel-specific but QEMU `qemu64` is Intel-mode.

### 1.5 Return paths & scheduler integration (`scheduler.rs:run_on_core_loop`)

AP `run_on_core_loop(core_id)` saves kernel `RSP` before `jmp jump_to_userspace`:

```asm
lea rax, [rip+2f]; push rax        // return label address
mov [KERNEL_RSP_ON_CORE[core_id]], rsp  // save (kernel PML4 still active)
mov rcx, cr3  // user PML4 phys — pinned to rcx (not rax!) — BUG-02:4 fix
jmp jump_to_userspace  // builds iretq frame SS/RSP/RFLAGS/CS/RIP, iretq -> Ring3
2:  // return_to_scheduler() ret lands here
mark_core_idle(core_id)
if !terminated { re-enqueue(pid, prio) }
```

`execution.rs:jump_to_userspace(rip,rsp,rflags)` does `cli; mov ds/es/fs/gs=user_data; push SS,RSP,RFLAGS,CS,RIP; iretq` — `rflags` is `0x202` on first entry, saved `frame.rflags` on resume (Yield/preemption).

`return_to_scheduler(): !` (`scheduler.rs:244`):

```rust
mov rsp, KERNEL_RSP_ON_CORE[core_id]; ret  // pops pushed label, resumes loop at 2:
```

Called from `sys_yield` (998), `sys_exit` (999), and `timer_interrupt_handler` preemption. Callee disables interrupts (`cli`) before calling.

### 1.6 Pointer validation

`validate_user_ptr(ptr,len)` `syscall.rs:38-41`: `ptr !=0 && end = ptr+len <= USER_MEM_MAX_ADDRESS (0x0000_7FFF_FFFF_FFFF) && !wraparound`. All syscalls with user pointers (`OpenFile` path, `StatFile` out ptr, `ListDir` buf, `ReadFile`/`WriteFile` buf, `GetWindowInfo`/`GetCpuInfo`/`GetCacheStats`/`CreateCircularBuffer` out ptr) check this first and return `u64::MAX` / `INVALID_FD` on fail. TODO: walk page tables to verify `USER_ACCESSIBLE` per page (currently not done).

---
## 2. Syscall Table (complete, implemented in `syscall.rs:215-1156`)

Return convention: success = documented value, error = `u64::MAX` (or `FileDescriptor::INVALID_FD = u64::MAX`, `INVALID_WINDOW_ID` style). `SyscallError` enum exists but handler returns raw `u64::MAX` for unimplemented/failed.

| Num | Name | Args (regs) | Return | Impl | Notes |
|-----|------|-------------|--------|------|-------|
| 0 | `CreateProcess` | `rdi=path_ptr, rsi=path_len, rdx=name_ptr, r10=name_len` | `0` ok, `MAX` err | **partial** | Parses `ODYS_ELF` (`elf_loader.rs:ODYS_ELF`, `include_bytes!` fallback) and calls `PROCESS_MANAGER.create_process_from_elf(0, &info, "proc1", 5)`. `path`/`name` args are logged but not used to load from FS — FS load is TODO `ISSUE-P8`. |
| 1 | `TerminateProcess` | — | `MAX` | no | stub, `_ => MAX` |
| 2 | `Write` | `rdi=fd(1/2), rsi=buf, rdx=count` | `count` | yes | `fd==1/2` → COM2 `serial2_print!("[pid=N] s")` routed via `log_splitter.py` to `userspace_pid_N.txt`; else `serial_println_core!`. Validates UTF-8 for logging, returns `count` even if invalid. |
| 3 | `Read` | — | `MAX` | no | stub |
| 4 | `GetLine` | — | `MAX` | no | stub |
| 5 | `Allocate` | `rdi=size:usize` | `old_heap_end:u64` or `MAX` | yes | Bump `ProcessMemoryLayout::grow_heap` at `0x6000_0000`, `heap_start=0x6000_0000 heap_end` moves; maps new pages `PRESENT|WRITABLE|USER_ACCESSIBLE|NO_EXECUTE` via `USER_MEMORY_MANAGER`. Returns old `heap_end` as alloc ptr. Used by rustspace `Arena` (`SLAB_SIZE=64KiB`) `user/rustspace/src/lib.rs:553`. |
| 8 | `LoadFile` | — | `MAX` | no | stub |
| 9 | `UnloadFile` | — | `MAX` | no | stub |
| 10 | `CreateWindow` | `rdi=w:u32, rsi=h:u32, rdx=x:i32, r10=y:i32` | `window_id:u32` | yes | `compositor.create_window(w,h,x,y,pid)` → `Window { Arc<WindowBuffer 2*heap alloc page-aligned> + EventBuffer }`, `z=7`. Returns `id` (index in `windows: RwLock<Vec<Window>>`). |
| 11 | `DestroyWindow` | `rdi=window_id:u32` | `0` | yes | `is_visible=false`, recycles id via `free_window_ids`. |
| 12 | `MapWindowBuffer` | `rdi=window_id:u32` | `user_base:u64` or `MAX` | yes | Maps **both** back and front buffers. `USER_WINDOW_BUFFER_BASE=0x0000_0001_0000_0000 + id*8MB` (back), `+8MB` offset for front (`MAX_WINDOW_BUFFER_SIZE=8MB`). Walks kernel heap via `translate_kernel_heap_virt_to_phys` (page-table walk, not `vaddr-HHDM_OFFSET` — BUG-11 fix), `map_specific_frame` with `PRESENT|WRITABLE|USER_ACCESSIBLE|NO_EXECUTE`. Copies `page_count = (w*h*4+0xFFF)/0x1000` pages. User writes XRGB `u32`. |
| 13 | `PresentWindow` | `rdi=window_id:u32` | `0` | yes | `buffer.present()` sets `needs_swap=true`; compositor `try_swap` pointer-swaps back/front (`UnsafeCell<NonNull<u32>>`) on next `compose`. |
| 14 | `GetWindowSize` | `rdi=window_id:u32` | `(width<<32)|height` or `MAX` | yes | Reads `WindowBuffer.width/height`. |
| 15 | `FocusWindow` | `rdi=window_id:u32` | `0` | yes | `max_z+1`, normalize if `>250`. |
| 16 | `GetWindowInfo` | `rdi=window_id:u32, rsi=info_ptr:*mut WindowInfo` | `0` ok, `MAX` err | yes | **New — missing from `process.md:333-359`.** Validates `info_ptr` (`size_of::<WindowInfo>=16`). Fills `WindowInfo { width,height, event_buffer_vaddr }` (`window.rs:33-38`). `event_buffer_vaddr` = `EventBuffer *` (kernel heap) or `0` if `None`. Used by `theophe` to find `EventBuffer`. |
| 20 | `OpenFile` | `rdi=path_ptr, rsi=path_len, rdx=flags:u8` | `fd:usize` or `MAX` | yes | `read_user_string` → `sirius.resolve_path` → `node_id`. `flags=FD_FLAG_READ(0x01)/WRITE(0x02)` stored in `Process.file_descriptors: Vec<FileDescriptor{node_id,flags}>` (`process.rs:89-93`), `offset` removed (was `usize`, now no offset — FS offset passed explicitly per read/write). `fd = index` in Vec. |
| 21 | `CloseFile` | `rdi=fd:usize` | `0` ok, `MAX` err | yes | Swap-remove `Vec` (`proc.file_descriptors.size -=1`, move last to `fd` if not last) — **fd indices not stable after close** (documented `filesystem.md:365`). |
| 22 | `ReadFile` | `rdi=fd, rsi=offset:usize, rdx=buf_ptr, r10=count` | `bytes_read:u64` or `MAX` | yes | Validates `buf_ptr`. Checks `FD_FLAG_READ`. `sirius.driver.read_file(node_id, offset, &mut [u8])` via `FilesystemDriver` (through `CachedDriver` if `use_cached_fs`). No auto-advance of per-fd offset — caller passes `offset` explicitly. |
| 23 | `WriteFile` | `rdi=fd, rsi=offset, rdx=buf_ptr, r10=count` | `bytes_written` or `MAX` | yes | Validates, checks `FD_FLAG_WRITE`, `sirius.driver.write_file(node_id, offset, &[u8])`. Writeback if cached (see 30-35): patches in-mem, marks `is_dirty`, disk not touched until flush. |
| 24 | `StatFile` | `rdi=path_ptr, rsi=path_len, rdx=stat_ptr:*mut StatFlat` | `0` ok, `MAX` err | yes | Validates `stat_ptr` (16+size). `sirius.resolve_path` → `FileNode {name,file_type,size,created/modified}` → copies to `StatFlat { name[16], name_len, is_dir, size, created_time, modified_time }` (`sirius.rs:StatFlat`). |
| 25 | `ListDir` | `rdi=path_ptr, rsi=path_len, rdx=out_ptr:*mut DirEntryFlat, r10=out_len` | `entry_count:u64` or `MAX` | yes | Validates `out_ptr`. `sirius.list_directory(path)` → for `i<min(entries.len, out_len/size_of::<DirEntryFlat>())` fills `DirEntryFlat { name[16], name_len, is_dir, _pad[6], size, created, modified }`. `_pad` keeps `size` 8-aligned. |
| 26 | `CreateFile` | `rdi=path_ptr, rsi=path_len` | `0` ok, `MAX` err | yes | `sirius.create_file(path)` → FAT32 `find_free_slot_in_directory` + `expand_directory` if needed, 8.3 validate `stem<=8 ext<=3`. |
| 27 | `CreateDir` | `rdi=path_ptr, rsi=path_len` | `0` ok, `MAX` err | yes | `sirius.create_directory(path)` |
| 28 | `Delete` | `rdi=path_ptr, rsi=path_len` | `0` ok, `MAX` err | yes | `sirius.delete(path)` — file or empty dir; invalidates cache first if `use_cached_fs`. |
| 30 | `PinFile` | `rdi=path_ptr, rsi=path_len` | `0` ok, `MAX` err | yes (cfg `use_cached_fs`) | `sirius.pin_file(path)` → load into cache `Resident` (never evict). Returns `MAX` if `!use_cached_fs`. |
| 31 | `UnpinFile` | `rdi=path_ptr, rsi=path_len` | `0`/`MAX` | yes (cfg) | `sirius.unpin_file(path)` → importance `Normal`. |
| 32 | `ReserveCache` | `rdi=path_ptr, rsi=path_len, rdx=importance:u8 0..6` | `0`/`MAX` | yes (cfg) | `sirius.reserve_cache(path, CacheImportance {0:Minimal,1:Low,2:Normal,3:High,4:VeryHigh,5:Critical,6:Resident})`. Directory-level hint; currently broken due to `directory_children` not populated — see `filesystem.md:233-240` & `action_items.md:A0-3`. |
| 33 | `EvictDirectory` | `rdi=path_ptr, rsi=path_len` | `freed_bytes:u64` or `MAX` | yes (cfg) | `sirius.evict_directory(path)` → drop all cached children of dir node. Same bug as 32 — no effect until A0-3 fixed. |
| 34 | `GetCacheStats` | `rdi=stats_ptr:*mut CacheStatsFlat` | `0` or `MAX` | yes (cfg) | Validates ptr, `sirius.cache_stats() -> CacheStats {total_files,total_bytes,max_bytes,dirty_files}` → `CacheStatsFlat {total_files,total_bytes,max_bytes,dirty_files:u64}`. |
| 35 | `FlushFileCache` | — | `0` or `MAX` | yes (cfg) | Flushes **all open fds of calling process**: collects `fd.node_id` for `pid` then `sirius.flush_nodes(&node_ids)` (batch). Global `flush_all_file_caches()` (`syscall.rs:1216`) flushes all dirty regardless of pid. Writeback policy: writes dirty→clean, `evict_one` flushes before evict. |
| 600 | `CreateCircularBuffer` | `rdi=size_bytes:usize, rsi=page_flags:u32, rdx=info_ptr:*mut CircularBufferInfo` | `0` ok, `MAX` err | yes | **New — missing from `process.md`.** `CircularBuffer::map_for_user` (`circular_buffer.rs:36-79`): `data_size=align_up(size,PAGE_SIZE)`, `total_virt_size=2*data_size`. Allocates `data_size/PAGE_SIZE` frames, maps each frame **twice**: at `virt_view_base + i*PAGE` and `virt_second_view_base = virt_view_base + data_size + i*PAGE` with `page_flags` (`PRESENT|WRITABLE|USER_ACCESSIBLE|NO_EXECUTE` typically). Returns `CircularBufferInfo { virtual_base, view_size=data_size, total_virtual_size }` via `*info_ptr` (no validation currently — TODO `validate_user_ptr`). Enables contiguous wrap-around read/write without modulo branch; `total_virt_size` is contiguous `2*data_size` alias. Uses `ProcessMemoryLayout::allocate_virtual_range` (`PROCESS_USER_VADDR_ALLOC_START=0x1000_0000, MAX=0x5000_0000`). |
| 970 | `GetCpuInfo` | `rdi=buf_ptr:*mut CpuInfoFlat, rsi=buf_len:usize` | `0` ok, `2` err | yes | **New — missing from `process.md`.** Validates `buf_ptr`. Reads `get_cpu_info_for_core(get_current_core_id())`, `TSC_FREQUENCY_HZ`, `BOOT_TSC`, `TOTAL_CORE_COUNT`. Fills `CpuInfoFlat {vendor,family,model,stepping,display_family,display_model,cache_line_size,apic_id,features:TSC|APIC|... bits, tsc_frequency_hz, boot_tsc, max_cpuid_leaf, max_extended_cpuid_leaf, core_count:u8}` (`cpuinfo.rs:49-65`, `syscall.rs:1129-1152`). Rust wrapper `sys_get_cpu_info(&mut CpuInfoFlat)->bool` checks `ret==0`. |
| 997 | `GetPID` | — | `pid:u64` | yes | `scheduler::get_current_process_for_core(core_id)` — `CURRENT_PROCESS_ON_CORE[core_id]`. No args. Rust alias `SYS_ECHO`/`SYS_GET_PID` both 997. |
| 998 | `Yield` | — | `0` (never returns directly) | yes | Saves `frame.user_rip/rsp/rflags` into `process.execution_context`, `cli`, `return_to_scheduler()` → scheduler re-enqueues `pid` and runs next. On resume `jump_to_userspace(rip,rsp,rflags)` sysrets to after `syscall` instruction. Cooperative. See `ISSUE-P9` tight-loop note. |
| 999 | `Exit` | `rdi=exit_code:i32` | `!` | yes | `PROCESS_MANAGER.terminate_process(pid, code, false)` (orphans children to arche `0`), `cli`, `return_to_scheduler()` → scheduler sees `Terminated`, logs, does not re-enqueue. Noreturn. No `wait()` yet (`ISSUE-P3`). |
| 996 | `GetProcessInfo` | — | `MAX` | no | stub `_ => MAX` |
| — | `Read (3), GetLine (4), LoadFile (8), UnloadFile (9), TerminateProcess (1)` | — | `MAX` | no | stubs, return `MAX`. |

Unlisted numbers → `match _ => u64::MAX` `syscall.rs:1154`.

Wire types that must match kernel <-> `user/rustspace/src/lib.rs`:

- `DirEntryFlat` 32B: `name[16], name_len:u8, is_dir:u8, _pad[6], size:u64, created:u32, modified:u32` (`filesystem.md:84`, `rustspace:97-105`). `FS_NAME_LEN=16` must be identical — no `static_assert` yet (`action_items.md:A1-5`).
- `StatFlat` 32B: `name[16], name_len, is_dir, size, created, modified` (`rustspace:143-152`). Note `StatFlat` has no `_pad` (smaller) vs `DirEntryFlat` — intentional.
- `WindowInfo` 16B: `width:u32, height:u32, event_buffer_vaddr:u64` (`window.rs:33-38`, `rustspace:839-844`).
- `CpuInfoFlat` 32B `align(16)`: `vendor,family,model,stepping,display_family:u16,display_model,cache_line_size,apic_id,features:u32, tsc_frequency_hz:u64, boot_tsc:u64, max_cpuid_leaf:u32, max_extended_cpuid_leaf:u32, core_count:u8` (`cpuinfo.rs:49-65`, `rustspace:183-198`).
- `CircularBufferInfo` 24B: `virtual_base:u64, view_size:u64, total_virtual_size:u64` (`circular_buffer.rs:29-33`, `rustspace:134-139`).
- `CacheStatsFlat` 32B: `total_files,total_bytes,max_bytes,dirty_files:u64` (`syscall.rs:1159-1165`, `rustspace:860-863`).
- `EventBuffer` 4KiB `align(4096)`: `write_idx,read_idx,event_count:AtomicU32 + events[MAX_EVENT_COUNT]` (`rustspace:728-733`), per-window `EventBuffer` mapped via `Window.event_buffer` (kernel heap phys → `WindowInfo.event_buffer_vaddr`).

---
## 3. Context Switch Details (userspace ↔ kernel ↔ scheduler)

### Userspace -> Kernel -> Userspace roundtrip (no preemption)

1. User `syscall` → CPU saves `RCX/R11`, clears `IF`, jumps to `LSTAR`.
2. Handler saves user `RSP/RIP/RFLAGS` + args + callee-saved, `mov rdi,rsp; call handle_syscall_inner; sti` inside.
3. Handler restores callee-saved, `mov rsp,r15; sysretq` → user `RIP=RCX, RFLAGS=R11`.

For `Yield`/`Exit` the handler never reaches `sysretq` — it calls `return_to_scheduler()` which `ret`s to `run_on_core_loop` label `2:`.

### Preemption (timer IRQ, Ring3)

`interrupts.rs:timer_interrupt_handler` (TSC-Deadline, 10ms):

- Increments `SYSTEM_TICKS`, re-arms deadline.
- If `stack_frame.code_segment.rpl()==Ring0` → just EOI + return (no preempt).
- If Ring3: saves `instruction_pointer/stack_pointer/cpu_flags` from `InterruptStackFrame` into `process.execution_context` (via `PROCESS_MANAGER`), `interrupt_over()` (LAPIC EOI), then `return_to_scheduler()` → scheduler picks next `PID` (priority 0-7 FIFO within level `scheduler.rs:188-193`). Resumed process re-enters via `jump_to_userspace(rip,rsp,rflags)` with saved `rflags`.

This is why `SFMASK` masks `IF` and `handle_syscall_inner` does `sti` — timer can preempt long syscalls; otherwise tight `sys_write` loop (ISSUE-P9) starves.

---
## 4. Userspace Wrappers

- **Rust:** `user/rustspace/src/lib.rs:223-547` — `syscall6` (`r10` dance), typed wrappers `sys_write, sys_allocate, sys_create_window, sys_map_window_buffer, sys_present_window, sys_open_file, sys_read_file, sys_write_file, sys_stat_file, sys_list_dir, sys_create_file/dir/delete, sys_get_cpu_info, sys_create_circular_buffer, sys_get_window_info, sys_pin_file` etc., `Arena` global allocator (64KiB slabs via `sys_allocate`), `EventReader` (`0x7000_0000`).
- **C:** `user/libc/syscall.h` + `syscall.c` + `crt0.c` — `syscallN` helpers, linked via `user/Makefile` (`clang --target=x86_64-unknown-elf -ffreestanding -mno-red-zone` + `ld.lld -T linker.ld`).

Build notes: `process.md:Userspace Test Program` still references old `user/programs/test/test.c`; primary Rust userspace is now `user/rustspace/src/bin/{theophe,fs_test}.rs` via `user/rustspace/x86_64-user.json` (`code-model=small`).

---
## 5. Gaps & TODOs (link to `action_items.md`)

- `CreateProcess` hardcodes `ODYS_ELF` — wire to Sirius FS (`ISSUE-P8`, `action_items.md:A2-6`).
- `FileDescriptor` no per-fd offset (removed) — offset is per-call arg; update `process.md:361-372` docstring.
- `CreateCircularBuffer` missing `validate_user_ptr(info_ptr)` — add.
- `directory_children` never populated — cache hints 32/33 dead (`A0-3`).
- No `wait()`/`get_process_info(996)` — exit code not propagated (`ISSUE-P3`).
- `validate_user_ptr` should walk page tables for `USER_ACCESSIBLE` (`A1-5`).

