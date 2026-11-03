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

- The syscall entry is `syscall_handler` (naked asm) in `kernel/src/process/syscall.rs`. User rsp is stashed in a per-core scratch field (`PerCoreSyscallData.user_rsp_scratch`, after `_stack` so `test_guard_pages` layout checks still hold). The per-core syscall stack top is 8 mod 16
- `jump_to_userspace` (`kernel/src/process/execution.rs`) restores only rip/rsp/rflags. GPRs saved into `ExecutionContext` on yield are not restored yet
- The kernel is built for `x86_64-unknown-none` (soft-float). The kernel binary contains no XMM instructions, so syscalls and interrupts do not clobber user SSE state. Only process switches save it (`FpuState`). Check with `objdump -d target/x86_64-unknown-none/debug/kernel | grep -c xmm`
- ELF segments of user programs share pages (`.text`/`.rodata`/`.bss` are not page aligned by `user/rustspace/linker.ld`). `map_virt_mem_region` handles `PageAlreadyMapped` by merging flags, and teardown (`unmap_user_region`) skips pages that are already absent so a shared page is freed once
- The `sys_create_process` userspace wrapper passes `&[char]` (UTF-32) where the kernel expects UTF-8, so process names show up as `o   ` in logs

## Disk image

- `storage/ata_disk.img` is FAT32 created by `scripts/create_ata_disk.sh` from `disk_templates/fat32_os_disk_template_default` with mtools. Inspect it with `mdir -i storage/ata_disk.img ::/`, `mtype -i storage/ata_disk.img ::/test.txt`, `minfo -i storage/ata_disk.img ::`
- Root dir offset: `(reserved_sectors + fat_count * fat_len) * sector_size` for cluster 2 (with 1 sector per cluster), then dump with `xxd -s <offset>`
