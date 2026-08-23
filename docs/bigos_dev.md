# BigOS -> MofuOS Feature Port — Detailed Audit & Transition Plan

> Source: BigOS `/home/bawercx/repos/cloned/bigos`, MofuOS `/home/bawercx/repos/MofuOS` audit 2026-08-23. Canonical docs `docs/initial_overview.md`, `docs/issues.md`, `docs/action_items.md`, `docs/syscalls.md`.

---

## 1. Executive Decision: Port Direction

`bigos` = clean bootstrap, `MofuOS` = thesis OS with full SMP/process/syscall/storage/userspace/shell but debt (`docs/issues.md`, `docs/action_items.md`). Correct direction: `MofuOS -> bigos`. `bigos` already has better `xtask` (`xtask/src/main.rs:1-893` tests discovery, `xorriso` iso), barrel `cargo check`. Do not `git merge`; cherry-pick subsystems.

Constraint: `bigos` `UP` single-core, `MofuOS` `SMP 4` (`kernel/src/lib.rs:13` `MAX_CORES=16` but `xtask` `cores=4`, `boot_common.rs:233` `cpus[i].bootstrap`). Port must keep `bigos` boot while adding SMP — biggest risk.

---

## 2. BigOS Audit — `/home/bawercx/repos/cloned/bigos`

### 2.1 Build/Boot — OK, but UP-only

* Good: `Cargo.toml:1-19` workspace `members=[kernel,kernel-macros]`, `kernel-macros/src/lib.rs:1-32` `#[test_case]` via `linkme`, `kernel/linker-x86_64.ld:1-63` `0xffffffff80000000`, `xtask` 900 LOC `build_kernel -Z build-std` + `build_user auto-discover user/programs/*/`. `bootloader/limine.conf:1-10` `protocol:limine`.
* Bad/Stub: `kernel/src/boot.rs:16-65` requests `Hhdm,Framebuffer,Rsdp,EfiMemmap,Memmap,Paging X86_64_4LVL, MpRequest` but `MpRequest` never consumed. No `AP trampoline` (`kernel/build.rs:1-8` only `-Tlinker -no-pie`), no `MAX_CORES`, no `AP_CORES_READY`. `kmain boot.rs:67-140` does `hhdm_offset` dynamic, no `HHDM_OFFSET` const validation, no `HEAP 0xFFFF_8080_0000_0000 16M` guard install, single `gdt::init()` BSP only. UP hard limit.

### 2.2 Memory — Skeleton, leaks `free`

* `memory/paging.rs:40-111` `MemoryMapFrameAllocator` bump-only, no `deallocate_frame`, no `free_list`. `memory/allocator.rs:1-245` `FixedSizeBlockAllocator` `[8..4096]` + `linked_list_allocator::Heap` 16M heap correct, but `dealloc 192` align=`block_size` over-align.
* `memory/usermem.rs:1-174` `UserMemoryManager {kernel_pt_phys,phys_offset}` `allocate_new_address_space:27-55` copy upper 256 PML4 correct, but `translate_user_virt_to_phys:57-114` ignores huge at `PDPT`, `map_virt_mem_region:116-151` no collision check, `create_main_stack 0x7FFF_FFFF_F000` ok. `ProcessMemoryLayout new:32-47` no shrink unmap.
* No `stack_guard.rs`, no `unmap_guard_page`, no per-core `GuardedKernelStack` / `GuardedSyscallSlot` (`MofuOS kernel/src/gdt.rs:14-39` 36KiB + 72KiB).
* Bugs: `paging.rs:112` `Handler::read_u8 todo!()` panics on ACPI AML walk. `process/task.rs:213` BSS `as_mut_ptr().write_bytes` writes to user `VirtAddr` without `HHDM` translate — wrong phys.

### 2.3 GDT/IDT/APIC/Timer — BSP only, half-correct

* `gdt.rs:1-121` `TSS.interrupt_stack_table[0] 4M` double fault `IST0`, `GDT` 6 entries `0x08/0x10/0x1B/0x23` + TSS. `interrupts.rs:431-468` `lazy_static IDT` `Timer 32/Keyboard 33`, no `int 0x80`. Single `GDT/TSS`, no `PER_CORE_GDT[T]`, no `PER_CORE_TSS`.
* `interrupts.rs:103-430` `LAPIC_ADDRESS Mutex<*mut u32>` no `Volatile`, `map_apic_mem 4K NO_CACHE` ok, `init_acpi` `acpi::AcpiTables` extracts `local_apic + io_apics[0]` but ignores `ISOs` (`356-413` commented), `disable_pic 82-87` writes only `0xA1 0xFF` slave, master `0x21` still live. `init_timer 47-279` `TSC_MOCK 2.4G / TIMER_TICK 1ms 2000Hz` + `TSC_DEADLINE 0x6E0` + periodic fallback `divider 16 LVTT 1<<17` correct, but `232` `rdtsc` without `cpuid` serialize.
* `io/serial.rs:1-41` `SERIAL1 0x3F8` ok.

### 2.4 Process/Scheduler — Skeletal / Stubbed `process/process_manager.rs:67-110`

* `process/task.rs:1-261` `ExecutionContext {16 GPRs + rip/rflags/cr3}`, `Process {pid,parent,priority 0-7,state,children:Vec, fds:Vec<FileDescriptor>, resources, exit_code, execution_context, memory_layout}` — field layout mirrors MofuOS but `create_with_elf:153-260` ignores `PF_RWX` (`PageTableFlags` hardcoded `P|W|U`), BSS bug above, `Merged` wrong `elf_loader.rs:51 new_end - curr_end` vs `new_end - curr_vaddr`.
* `process_manager.rs:1-180` `create_process` commented out, `processes Vec` never pushed, `init_arche` empty, `SCHEDULER` `scheduler.rs:1-1 //TODO` empty. `get_process` linear scan O(n). `terminate_process:112-154` marks `INVALID_PID` but no `free_address_space`, no `remove_pid` scheduler.
* `process/execution.rs:1-72` `jump_to_userspace` `push SS,RSP,0x202,CS,RIP iretq` — no `swapgs`, no `TSS.RSP0` update, no `CORE_POOL`, `execute_process_direct` `Cr3::write(Ph2Empty)` `+0` rflags.
* No preemption, no `run_on_core_loop`, no `KERNEL_RSP_ON_CORE`, no `return_to_scheduler`.

### 2.5 Syscalls — Single global stack, unvalidated

* `process/syscall.rs:1-372` `SyscallNumber 0 CreateProcess..999 Exit` `syscall6 r10/r8/r9`. `init_syscall:337-372` `STAR (0x1B<<48|0x23<<32|0x08<<16|0x10) LSTAR syscall_handler FMASK 0`. `syscall_handler 235-293` naked `mov rsp,[STACK_TOP]` pushes `r15,rcx,r11,rax,rdi,rsi,rdx,r10,r8,r9,rbp,rbx,r12-14` -> `handle_syscall_inner` -> `sysretq`. Bug: `SYSCALL_STACK 64K align 4096:197-204` single static `STACK_TOP AtomicU64` — SMP-unsafe, re-entrant clobber. MofuOS `SYSCALL_SLOTS[16] 72K guard+data align64`.
* `SyscallFrame 214-233` ok, but `handle_syscall 145-194` only `0,1,2,3,4,5,999` — `2 Write` loops `from_raw_parts -> serial_print` no fd check, no `validate_user_ptr`, `997 arg1+4` dummy. `user/libc/syscall.h:1-74` `syscall6` correct, `crt0.c:1-18` `_start` ok, `user/programs/first/first.c:1-9` single test program — vs MofuOS `ping,ping_yield,rustspace window_app,theophe,fs_test`.

### 2.6 Storage — Mock only, FAT single-cluster

* `io/disk.rs:1-199` `DiskDevice {read/write_sectors}` `MockDiskDevice Vec<u8>` `DiskManager Box<dyn> DISK Once` `SECTOR 512`. No `io/ata.rs` `AtaPioDriver` LBA28 polling `READ 0x20 WRITE 0x30 FLUSH 0xE7 IDENTIFY 0xEC` `0x1F0/0x1F6/0x3F6` 256-word loops.
* `filesystem/sirius.rs:1-291` `Sirius<Box<dyn FilesystemDriver>> SIRIUS Once`, `find_node/list/read/write/create/delete` via `split_path` ok.
* `filesystem/fat32/mod.rs:1-1105` `BootSector, DirectoryEntry 32B 8.3, FileNodeHandle ((attrs&7F)<<56|(parent&FFFFFF)<<32|(cluster&FFFFFF)) 41-50`. `Fat32Driver::new 145-190` `fat_start,reserved,num_fats,fat_size,data_start,cluster_size,max_cluster` `total_32||total_16` — `diskTemplates` 32M case. Bugs: `write_fat_entry` RMW only FAT1 not FAT2 (`MofuOS A1-1`), `find_free_cluster 329` linear FAT scan no FSInfo, `read_file 692-800` only copies `cluster_size - offset%cluster_size` single cluster truncate, `write_file 802 NotSupported` — `MofuOS 723,831` cluster chain alloc + `read-modify-write`. `read/write_direntry` rewrite whole chain per 32B expense.
* `fat32/test_data.rs:1-269` `create_fat32_image 64K 2FATs root 2 HELLO.TXT 3 LORE 4 NESTDIR 5` — same as MofuOS.

### 2.7 Userspace/Shell — No shell

* `user/crt0.c linker.ld 0x400000` ok. Only `first`. No `rustspace x86_64-user.json small` (`MofuOS user/rustspace/src/lib.rs:225 syscall6` + `Arena 64K slabs` + `CircularBuffer 600`). Theophe = `kernel/src/programs/theophe.rs:1-511` kernel console `ls/cat/mkdir/demo help` `event poll + compositor.compose`, not a process. No userspace shell, no `sh`, no pipe, no argv.
* Graphics `framebuffer.rs 1-141 / window.rs 1-344 double-buffer UnsafeCell / compositor.rs 1-202 z sort copy_nonoverlapping` mirrors MofuOS but `compositor loop main.rs:75-87` polls not `hlt`.

### 2.8 Testing — Best part

* `kernel-macros` + `xtask/tests` `kernel/src/bin/test_*.rs` `Hhdm+Memmap init_with_heap run_all_tests` `QemuExitCode 0x10/0x11 Port 0xF4` `test_basic_boot,elf_loader,filesystem,heap,page_mapping` — better than MofuOS `main.rs:81 test_ata_filesystem` gated.

---

## 3. MofuOS Current State — What to Steal

| Subsystem | File:Line | Status |
|---|---|---|
| SMP | `kernel/src/boot_common.rs:233 bootstrap`, `asm/ap_trampoline.S:1-168` `0x8000/0x9000`, `util/cpuinfo.rs:635 ap_core_from_limine_entry_point`, `gdt.rs:144 PER_CORE_GDT/TSS, GuardedKernelStack 32K+4K guard` | Correct but `setup_ap_trampoline_mapping 56` dead, Limine MP used not trampoline, `MAX_CORES 16 vs log_splitter.py 4 vs QEMU -smp 2` `H3/A1-6`, `PREEMPTION_ENABLED false 59` not enabled, X2APIC forced off `MP_FLAG_NO_X2APIC H1` |
| Scheduler | `process/scheduler.rs:210-510` `CoreScheduler core_id ready_queues[8] Dequeue current`, `run_on_core_loop 358` `mov rsp scheduler_top-0x1000 lea rax 2f push rax mov [KERNEL_RSP] Cr3 jmp`, `return_to_scheduler 41 mov rsp ret` | Correct race: push `process+thread_group` before `enqueue_on_core` `process_manager.rs:181-296`. `on_timer_tick 255` empty, preemption via `interrupts.rs:1085` `if in_userspace && has_waiting save rip/rsp/rflags -> return_to_scheduler` handles `SS 0x1B` Intel `SYSRET SS=0x18 RPL0 BUG-09 SFMASK 1<<9` |
| Syscalls | `process/syscall.rs:125 SYSCALL_SLOTS GuardedSyscallSlot 4096 guard + PerCoreSyscallData 64K align64`, `221 naked swapgs mov rsp gs:0 swapgs` `198 SyscallFrame` | Correct per-core GS `KERNEL_GS_BASE 0xC0000102`, `EFER SCE, STAR 0x10<<48\|0x08<<32 LSTAR SFMASK 1<<9`. Implemented `2 Write->serial2 [pid]`, `5 Allocate bump 0x60000000`, `10-16 Window(USER_WINDOW_BUFFER_BASE 0x100000000+id*8M back+front via translate_kernel_heap walk BUG-11 fix)`, `20-28 FS, 30-35 Cache, 600 Circular 2*data_size double-map, 970 CpuInfo, 997 GetPID 998 Yield 999 Exit`. Weak `validate_user_ptr 38 ptr!=0 && end<=0x800000000000` no page walk `A1-5`, `FS_NAME_LEN 16` fragility |
| Memory | `memory/usermem.rs:33 allocate_new_address_space copy 256..512 kernel PML4`, `73 translate walk PRESENT HUGE 0x1FFFFF`, `170 map_virt_mem_region P\|W\|U merge flags OR+NX`, `254 map_specific_frame`, `311 create_main_stack 0x7FFFFFFFFFF000`, `333 unmap_all + 479 reclaim_empty + 585 free_pml4 dedup` | Advanced. `memory/memory.rs:284 allocate_frame free_list first else bump`, `allocator.rs:67 32M heap 0xFFFF_8080_0000_0000 FixedSizeBlock [8..4096]+linked_list`. `stack_guard.rs:12 unmap_guard_page split 2M huge -> PT` |
| FS | `filesystem/fat32/mod.rs:226 read_fat_entry &0x0FFFFFFF RWM new&0x0F000000`, `fs 8.3 to_fat32_name 78 stem<=8 ext<=3`, `file_cache.rs:135 arena 16M`, `sirius.rs:102 Sirius<CachedDriver<Fat32Driver>> vs Fat32Driver cfg use_cached_fs` | `write_file` chain alloc `723`, `MockDisk+ AtaPioDriver LBA28 polling flush E7`, `init_filesystem_ata 64M`. Debt `A0-3 directory_children empty register_file_in_directory never called -> reserve/evict no-op`, `F6 FAT2 stale`, `F5 LFN not supported`, `cache dead importance file_id not parent` |
| Userspace | `user/rustspace/src/lib.rs:225 syscall6 r10 dance`, `src/bin/theophe.rs:1-136 UserSurface CircularBuffer 64K EventReader 0x700000000`, `src/gfx pipeline/renderer`, `user/crt0.c linker.ld 0x400000`, `rustspace Cargo slit +nightly -Z build-std -Z json-target-spec` | True shell window `MapWindowBuffer Present Focus`, `Arena bump+free 16K slab grow via sys_allocate`. C `ping/ping_yield/test` vs Rust `rust_first/window_app/odys/memstress/fs_test` |
| Graphics | `graphics/compositor.rs compose sort z try_swap clip copy_nonoverlapping`, `window.rs WindowBuffer back/front needs_swap Atomic` | No alpha, no damage, `heap not HHDM` walk fix `11` |

Known MofuOS gaps to NOT port blindly: `validate_user_ptr` page-walk missing, `FileCache A0-3`, `FAT mirror`, `allocate_virtual_range None`, `PREEMPTION false`, `TRAMPOLINE dead`, `core0 reserved waste P5`, `FrameAllocator deallocate` tests.

---

## 4. Gap Matrix — What BigOS Must Change

| # | BigOS File | Issue | Fix = MofuOS Source File:Line |
|---|---|---|---|
| M1 | `boot.rs:67 kmain` | No guard pages, no per-core GDT | `MofuOS boot_common.rs:71-240 + gdt.rs:110 install_guard_pages + stack_guard.rs:12` |
| M2 | `memory/*` | Bump-only allocator | `memory/memory.rs:229 deallocate_frame + free_frame_count + reclaim_empty` |
| M3 | `memory/usermem.rs` | No `map_specific_frame`, no `reclaim_empty` | `usermem.rs:254,479` |
| S1 | `gdt.rs` | Single GDT/TSS | `gdt.rs:144 PER_CORE_GDT/TSS + GuardedKernelStack/SyscallSlot` copy, align 4096 |
| S2 | SMP missing | Add `MAX_CORES 4 (keep in sync xtask/log_splitter)`, `asm/ap_trampoline.S + build.rs cc/ld/objcopy AP_TRAMPOLINE_BIN`, `setup_ap_trampoline_mapping` or Limine MP `util/cpuinfo.rs:635` | Choose Limine MP (already requested) — keep `boot.rs MpRequest` path |
| P1 | `process_manager.rs:67` stub | Implement `create_process_from_elf` least-loaded core pick | `process/process_manager.rs:181-296` enqueue order critical |
| P2 | `scheduler.rs:1` empty | `CoreScheduler 8 queues Dequeue + run_on_core_loop + KERNEL_RSP_ON_CORE` | `process/scheduler.rs:61-510` |
| P3 | `syscall.rs:197` global stack | Per-core `SYSCALL_SLOTS + KERNEL_GS_BASE + swapgs` | `process/syscall.rs:125-272` |
| P4 | `syscall handler` no validation | `validate_user_ptr + walk USER_ACCESSIBLE`, `SFMASK 1<<9` `sti` early | `syscall.rs:38,221` |
| F1 | `fat32 read_file` single cluster | Loop chain `offset/cluster_size` copy | `fat32/mod.rs:723` |
| F2 | `write_file NotSupported` | Implement chain alloc `expand` | `fat32/mod.rs:831` |
| F3 | `MockDisk only` | Add `io/ata.rs 325` `AtaPioDriver` + `DiskManager` + `storage/ata_disk.img 64M create_ata_disk.sh` + QEMU `-device piix3-ide -drive ata0` | |
| F4 | No cache | `FilesystemDriver cfg use_cached_fs` `file_cache.rs CachedDriver` optional | `filesystem/file_cache.rs:135` fix `A0-3` first if porting |
| U1 | `user/programs` 1 C program | `user/rustspace x86_64-user.json small + Arena + CircularBuffer 600` + window syscalls `10-16` | `user/rustspace/src/lib.rs:223-547` |
| U2 | No shell | Move `kernel/programs/theophe` -> `user/rustspace/src/bin/theophe.rs` + `terminal/*` | `docs/userspace/terminal.md` Option A gfx->user |
| U3 | No shell spawn | `sys_create_process 0 path ptr` wired to `Sirius` not baked `ODYS_ELF` `ISSUE-P8` | |

Incorrect/Oversimplified list (must fix before port): BSS `VAddr write` `task.rs:213`, ELF merge size bug `51`, `heap shrink leak`, `todo!()` ACPI reads, `PIC 0x21` not disabled, `read_file` truncate, `DiskDevice` `Box<dyn>` dispatch vs MofuOS static monomorphized, `Vector resize` `layout new_capacity dealloc old_capacity leak`.

---

## 5. Transition Plan — Phased, Dependency-Ordered

### Phase 0 — Align Build/Test (1 day, no risk)

1. Keep `bigos xtask` superior; merge `MofuOS .cargo/config.toml` `link-arg -Tlinker-x86_64.ld relocation static` + `x86_64-kernel.json large -mmx -mno-red-zone` verification.
2. Copy `kernel/GNUmakefile` `AP trampoline via cc/ld/objcopy` recipe to `bigos kernel/build.rs` behind feature `smp`.
3. Add `cargo xtask run` `log_splitter.py` dual-socket `COM1 /tmp/mofuos_com1.sock COM2` `MAX_CORES` constant single source `kernel/src/lib.rs:13`.
4. Verify `cargo xtask iso` boots to Theophe `ls` — gate.

### Phase 1 — Memory & Guards (2-3 days, foundation)

1. Port `memory/memory.rs:40 deallocate_frame`, `setup_ap_trampoline_mapping` (even if dead), `memory/usermem.rs:254 map_specific_frame, 311 create_main_stack USER_STACK_TOP, 333 unmap_all, 479 reclaim_empty`, `memory/stack_guard.rs:12 unmap_guard_page` splitting 2M.
2. Port `gdt.rs:14 GuardedKernelStack 36K + GuardedSyscallSlot 72K + PER_CORE_GDT/TSS` but keep `core 0` only initially (`init_core_gdt(0)`). Fix `allocator.rs` Align bug.
3. Fix BigOS bugs: `task.rs:213 HHDM translate for BSS`, `elf_loader merge size`, `paging Handler todo->IdentityAcpiHandler`.
4. Tests: `test_page_mapping, test_heap, test_stack_overflow` must pass `cargo xtask run --test`.

### Phase 2 — SMP (3-5 days, highest risk)

1. Decision: Keep Limine MP (`boot.rs MpRequest`) — do not revive bare trampoline unless need `0x8000` identity map tests. Add `boot_common.rs:71 bsp_early_init` steps `9-14`: `init_core_gdt(0) load_idt init_offset_page_table init_heap install_guard implying CR3 -> init_acpi parse MADT -> CORE_POOL.init_with_core_count -> SCHEDULER.init_with_core_count -> init_current_core(0) -> for i 1..n cpus[i].bootstrap(ap_entry,0x12345678)`.
2. Add `kernel/src/util/cpuinfo.rs` `CpuInfo + init_cpu_infos + ap_core_from_limine_entry_point` (GDT+LAPIC+syscall+idt+run_loop).
3. Add `util/apic.rs lapic_read/write`, `util/msr.rs`, fix `disable_pic` `0x21+0xA1 0xFF`, `map_apic_mem 0xFFFF_FFFF_0000_0000`.
4. QEMU `xtask qemu_smp_arg cores=MAX_CORES` `MAX_CORES 4` sync `boot.rs + gdt + scheduler + log_splitter`. Test: `serial_println_core! [Core N]` appears for `1..3`, `AP_CORES_READY 3`.

### Phase 3 — Scheduler + Process Manager (4-6 days)

1. Port `process/kernel_thread.rs:18 + scheduler.rs:61 CoreScheduler 8 Dequeue + blocked + current + Scheduler per_core + CURRENT_PROCESS_ON_CORE + KERNEL_RSP_ON_CORE + SCHEDULER_STACKS`, `core_pool.rs:15 available bitmask`.
2. Implement `run_on_core_loop 358` `scheduler_stack_top-0x1000 mov rsp, disable get_next hlt, set_current, Cr3 write, enable, asm lea push mov [slot] jmp` + `return_to_scheduler 41`.
3. Replace `process_manager.rs` stub: `create_process_from_elf 181` pick `least-loaded AP 1..total` via `SCHEDULER.lock ready_count`, `Process::create_with_elf` (now fixed flags `PF_R/W/X -> PTE U/W/NX`), push vectors before `enqueue_on_core` with `disable()` around `SCHEDULER lock` (alloc inside `Dequeue`). `terminate_process 298 free_address_space reclaim + release_core + remove_pid + orphan->arche`, `cleanup_dead`.
4. Port `process/process_mem.rs:57 allocate_heap + 127 allocate_virtual_range` (fix `find_free_gap` via `allocated_ranges` gap scan, not `None`).
5. Tests: `test_elf_loader` 2 `ping` instances `PID1/2 on core 1` as `MofuOS process_start.rs:1 create_userspace_processes`.

### Phase 4 — Syscalls (3-4 days)

1. Replace `syscall.rs` with per-core `125 GuardedSyscallSlot align4096 PerCoreSyscallData align64 stack_top+_stack 64K`, `install_syscall_guard_pages 168`, `init_syscall 1339 STAR/LSTAR/SFMASK 1<<9 KERNEL_GS_BASE`, naked `221 swapgs gs:0`.
2. Keep `SyscallNumber` but sync to MofuOS table `0,1,2,5,10-16,20-28,30-35,600,970,997-999` (`docs/syscalls.md` canonical). Implement dispatch `handle_syscall_inner 275` `sti` early, `validate_user_ptr 38` -> upgrade to walk `USER_ACCESSIBLE+WRITABLE` later `A1-5`, `SFMASK` preemption fix `A1-4`.
3. Add `Yield 998 save rip/rsp/rflags -> return_to_scheduler`, `Exit 999 Cr3 kernel terminate`, `GetPID 997 CURRENT_PROCESS_ON_CORE`, `Allocate 5 allocate_heap`, `Write 2 COM2 serial2 [pid=N]` dual-socket.
4. Tests: `first` `syscall6 997` + `sys_write` + `sys_exit 123` QEMU exit `0x10`.

### Phase 5 — Storage (4-7 days)

1. Keep `MockDiskDevice` for tests. Add `io/ata.rs:53 AtaPioDriver LBA28 polling` `DiskDevice` `DISK Once Mutex DiskManager` `SECTOR 512` `storage/ata_disk.img 64M` `scripts/create_ata_disk.sh -s 64 + disk_templates/fat32_os_disk_template_default`.
2. Port `filesystem/fat32/mod.rs:59 node_id pack, 226 read_fat_entry mask 0x0FFF_FFFF RWM, to_fat32_name 8.3, cluster_to_sector, find_free_cluster linear->FSInfo later`, fix `read_file 723` loop multi-cluster, implement `write_file 831` alloc+zero `expand_directory`, `write_direntry 575` RMW optimization later.
3. Port `filesystem/sirius.rs:102 + filesystem/file_cache.rs:135` only after fixing `A0-3` `register_file_in_directory` call sites + `get_effective_importance parent_id` lookup, else keep `use_cached_fs` disabled `default=[]` (`Kernel Cargo feature`). MofuOS `FS_CACHE_SIZE 16M`.
4. Wire `init_filesystem_ata / init_filesystem_ata_with_cache` called lazily not `kmain` (keep `test_ata_filesystem` gate). QEMU `-device piix3-ide bus=ide.0 -drive ata0`.

### Phase 6 — Userspace & Shell (5-8 days)

1. Keep `user/crt0.c linker.ld 0x400000`. Create `user/rustspace` workspace `Cargo fmt + nightly build -Z build-std=core,alloc -Z json-target-spec --target x86_64-user.json` `small code-model` (`MofuOS user/rustspace/Makefile 1-8`).
2. Port `user/rustspace/src/lib.rs:224 syscall6 r10 dance + wrappers sys_write/read, sys_allocate Arena 64K slabs preallocate/grow, sys_create_window 10, sys_map_window_buffer 12 USER_WINDOW_BUFFER_BASE 0x100000000+id*8M walk BUG-11, sys_present 13, sys_get_window_info 16, sys_create_circular 600 double-map 2*data_size, sys_get_cpu_info 970, sys_yield/exit`.
3. Port `user/rustspace/src/gfx pipeline/renderer/resources/shaders/transform` as library `gfx/` replacing `kernel/src/graphics/pipeline` keep kernel `compositor+framebuffer+window` only (`docs/userspace/user_renderer.md` Option A).
4. Port shells: `kernel/programs/theophe.rs -> user/rustspace/src/bin/theophe.rs + terminal/*` (`Window 600x400 + Scrollback Circular 64K + EventReader 0x700000000 -> InputEvent Char/Key/Mouse`) and `odys`. `compositor::get_input_event_buffer` shared `EventBuffer 4K 340 events Atomic write/read` `events/event_buffer.rs:1-144`.
5. Update `process/elf_loader.rs` `ODYS_ELF` baked->`Sirius.resolve_path` load `CreateProcess 0 path_ptr` `ISSUE-P8`, add `sys_open/read/write/stat/list/create/delete 20-28` wiring.

### Phase 7 — Hardening & Parities

* Fix MofuOS debts before final: `A1-5 validate_user_ptr walk`, `A1-1 FAT2 mirror`, `A1-4 PREEMPTION true`, `A0-1 find_free_gap`, `H3 MAX_CORES sync`, `GOT-06 XRGB hardcoded`, `C2 ACPI handler todos`.
* Switch `PREEMPTION_ENABLED true` after `timer_interrupt_handler 1085` `in_userspace && has_waiting` path stable.

---

## 6. What NOT to Port / Keep from BigOS

* Keep `xtask 893` test discovery + `find_tool llvm` + `ensure_ovmf 7 paths`; MofuOS `GNUmakefile` shim deprecated.
* Keep `kernel-macros #[test_case]` `linkme` distributed_slice — MofuOS lacks, more ergonomic than `tests_exp` `boot_common test_main`.
* Keep `allocator.rs FixedSizeBlockAllocator` — MofuOS same but BigOS debug macros richer.

## 7. Risks & Mitigations

* SMP hang black screen: `QEMU -no-reboot` traps `0xF4`; if stuck remove flag per `docs/initial_overview.md`. Test single core first `QEMUFLAGS="-smp 1"`.
* Lock order deadlock: `CORE_POOL -> PROCESS_MANAGER -> SCHEDULER -> FRAME_ALLOCATOR/USER_MEMORY -> SERIAL` (`docs/conventions.md`). Never hold `SCHEDULER` while `PROCESS_MANAGER`.
* HHDM vs heap walk: `BUG-11` — `map_window_buffer` must walk heap via `translate_user_virt_to_phys`, not `vaddr - HHDM_OFFSET`. Keep `UserMemoryManager::translate_kernel_heap_virt_to_phys` removed helper reminder.
* GS/Syscall stack corruption: `BUG-01/02` single stack; verify `asm per_core` + `KERNEL_GS_BASE = &SYSCALL_SLOTS[core].data (+0x1000 guard)`.

## 8. Clarifying Questions Before Build

1. `MAX_CORES` target? `4` (QEMU `3` AP matches `MofuOS` current `make run -smp 3`, `log_splitter`) or `2` keep BigOS UP habit? Changes `per_core` arrays + QEMU.
2. Keep `MockDisk` or immediate `ATA` `64M` image `storage/ata_disk.img` QEMU `ide-hd`? Impacts `F3` scope.
3. `use_cached_fs` default ON/OFF? BigOS currently no cache; MofuOS `default=["use_cached_fs"]` but `A0-3` dead.
4. Preserve `bigos data_structures::Vec` custom or migrate to `alloc::vec::Vec` for `Process.children`? Custom `resize new_capacity` leak bug needs fix either way.
5. Shell choice: port `theophe` only or `odys` too? Both in `user/rustspace src/bin`.

## 9. Verification Checklist (per phase gate `cargo xtask iso + run 10s`)

* `test_basic_boot` `test_elf_loader` `test_heap` `test_page_mapping` `test_filesystem` all `QemuExitCode Success 0x10` `Port 0xF4`.
* `serial_println_core!` shows `Core 0/1/2` + `AP_CORES_READY`.
* `ping/ping_yield` `sys_yield` loops without `starvation` `PREEMPTION true`.
* `ls /` `cat /HELLO.TXT` via `Sirius` `ata` persists across `cargo xtask run-fs`.
* `window_app` `sys_map_window_buffer` XRGB visible, `compositor.compose` swaps, `Theophe` userspace window focus input.
