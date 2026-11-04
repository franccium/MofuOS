# Knowledge gaps

Facts about MofuOS that are not obvious from a first read and cost time when missing

## Userspace

- Userspace programs are embedded into the kernel with `include_bytes!` in `kernel/src/process/elf_loader.rs` (from `user/rustspace/target/x86_64-user/debug/*`). A userspace change needs a kernel rebuild (`cargo xtask build`/`iso`) to take effect
- odys is not started at boot. theophe spawns it when `odys` is typed into its terminal (`user/rustspace/src/terminal/mod.rs`, `CreateProcess` always loads `ODYS_ELF`)
- The userspace heap is the `Arena` global allocator in `user/rustspace/src/lib.rs`: size classes 8..4096 with intrusive free lists, plus a large-block free list. Free-list nodes are written into freed blocks, so every node type must fit in the smallest block it can land in. `Option<*mut T>` is 16 bytes (no niche), `Option<NonNull<T>>` is 8 bytes
- `FONT_8X13` (embedded-graphics ascii) renders any non-printable or non-ASCII byte as `?`. `?` in the UI usually means corrupted bytes, not an encoding problem
- `rustspace::println!` sends one `sys_write` per format piece

## Logs

- Every run writes `logs/<timestamp>/`: `all.txt`, `core_N.txt` (kernel, per core) and `userspace_pid_N.txt`
- Kernel `sys_write` prints `[pid=N] <chunk>` per chunk on COM2 and the log splitter strips the prefix, leaving a space. Extra spaces around format args in `userspace_pid_*.txt` (`id= 1`, `0x 1007d0000`) are a log artifact, not a formatting bug
- Many modules define a local `serial_println_core!` / `serial_println_verbose!` wrapper that is disabled (e.g. `kernel/src/filesystem/sirius.rs`). A missing log line does not mean the code path did not run

## Kernel

- The syscall entry is `syscall_handler` (naked asm) in `kernel/src/process/syscall.rs`. User rsp is stashed in a per-core scratch field (`PerCoreSyscallData.user_rsp_scratch`, safe because IF is off until the stack switch is done). The handler runs on the kernel stack of the calling task (`gdt::KERNEL_TASK_STACKS`), whose top is 16 aligned, so the 16 pushes leave rsp aligned for the call. There is no per-core syscall stack
- A task never resumes through a special path. Its user registers sit in a frame on its kernel stack (`SyscallFrame` or `TrapFrame`) and come back with the normal `sysretq` or `iretq`, its kernel registers are saved by `switch_context`. `test_user_regprobe` checks that every register survives a syscall and a yield
- After a `sysretq` an interrupt in ring 3 pushes SS=0x18 (RPL 0) under KVM with `qemu64`, and `iretq` with it faults. `trap::interrupt_return_path` rewrites SS for every return to ring 3
- A process is torn down only by itself, on its own kernel stack (`task::exit_current`). `TerminateProcess` on another process is a request (`task::request_kill`), so the target is still alive when the syscall returns. Poll its `ProcessState` or its slot (`task::find_slot_by_pid`)
- `get_current_core_id()` reads the LAPIC id register, which is a VM exit under KVM, and the LAPIC address is an atomic pointer. It used to be a `spin::Mutex` that the timer interrupt also took, which deadlocked a core when a tick landed inside the lock (the old `test_user_fpu_switch` timeout)
- Every serial byte costs about 87 us under QEMU (emulated baud rate). Per-process and per-container logs are behind `PROCESS_DEBUG_PRINT`, `USER_MEMORY_DEBUG_PRINT`, `DATA_STRUCTURE_DEBUG_PRINT` and `SCHED_DEBUG_PRINT`, all off, because a 200 round spawn and kill loop cannot run with them on
- `target/ovmf/ovmf-vars-x86_64.fd` is writable and a test that is killed by the timeout can leave it corrupt, after which QEMU boots nothing and the serial log stays empty. Copy `/usr/share/OVMF/OVMF_VARS_4M.fd` over it
- `rdtscp` is not offered by the `qemu64` cpu model of the tests, user programs must use `rdtsc`
- The kernel is built for `x86_64-unknown-none` (soft-float). The kernel binary contains no XMM instructions, so syscalls and interrupts do not clobber user SSE state. Only `core_run_scheduling(core_id)` saves it (`FpuState` in the task slot) and the scheduler loop restores it right before the switch to the task. Check with `objdump -d target/x86_64-unknown-none/debug/kernel | grep -c xmm`
- ELF segments of user programs share pages (`.text`/`.rodata`/`.bss` are not page aligned by `user/rustspace/linker.ld`). `map_virt_mem_region` handles `PageAlreadyMapped` by merging flags, and teardown (`unmap_user_region`) skips pages that are already absent so a shared page is freed once
- The `sys_create_process` userspace wrapper passes `&[char]` (UTF-32) where the kernel expects UTF-8, so process names show up as `o   ` in logs

## Disk image

- `storage/ata_disk.img` is FAT32 created by `scripts/create_ata_disk.sh` from `disk_templates/fat32_os_disk_template_default` with mtools. Inspect it with `mdir -i storage/ata_disk.img ::/`, `mtype -i storage/ata_disk.img ::/test.txt`, `minfo -i storage/ata_disk.img ::`
- Root dir offset: `(reserved_sectors + fat_count * fat_len) * sector_size` for cluster 2 (with 1 sector per cluster), then dump with `xxd -s <offset>`
