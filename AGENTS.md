# AGENTS.md — MofuOS

## Build — Don't Use `cargo` at Workspace Root
This is a bare-metal `x86_64` OS (Limine boot, custom target, `no_std`). The root `Cargo.toml` workspace (`members = ["kernel"]`) and `src/main.rs` (commented-out `ovmf_prebuilt` harness) are **not** the build entry. All builds go through `GNUmakefile`.

```bash
make all              # clone/build limine (v10.x-binary), build kernel + userspace, create template-x86_64.iso
make all-hdd          # same but HDD image (template-x86_64.hdd) + test_disk_image.fat32.img
make -C kernel        # kernel only (nightly cargo + AP trampoline via cc/ld/objcopy)
make -C user          # C userspace only (clang + ld.lld); rust userspace via cargo in user/rustspace
make clean            # kernel cargo clean + rm iso/hdd/fat32/ata_disk.img
```

`kernel/GNUmakefile` runs: `RUSTFLAGS="-C link-arg=-Tlinker-x86_64.ld -C relocation-model=static" cargo build --target x86_64-unknown-none --profile dev|release`
`user/Makefile` runs: `clang --target=x86_64-unknown-elf -ffreestanding -mno-red-zone` + `ld.lld -T linker.ld`; Rust userspace at `user/rustspace` runs `cargo fmt && cargo +nightly build -Z build-std=core,alloc -Z json-target-spec --target x86_64-user.json`.

**Toolchain:** `rust-toolchain.toml` pins `nightly` with `rustfmt clippy rust-src llvm-tools-preview` + `x86_64-unknown-uefi`. Host needs `QEMU`, `llvm-tools` (`llvm-ar`, `ld.lld`, `objcopy`), `xorriso`, `mtools` (`mmd`/`mcopy`), `parted`, `mkfs.fat`, `cc`/`ld`. Debian: `sudo apt install -y qemu-system-x86 llvm-14-tools ovmf xorriso mtools parted dosfstools`.

## Run — OVMF Required
```bash
# First time (OVMF not tracked in git):
mkdir -p ovmf && cp /usr/share/OVMF/OVMF_VARS_4M.fd ovmf/ovmf-vars-x86_64.fd \
               && cp /usr/share/OVMF/OVMF_CODE_4M.fd ovmf/ovmf-code-x86_64.fd
# or: make edk2-ovmf  (curls edk2-ovmf-nightly tarball)

make run              # QEMU q35 + kvm + 3 cores (SMP) + ATA disk + log_splitter.py; preferred dev path
make run-nologs       # same but -serial stdio, no socket/log splitting
make run-fast-x86_64  # virtio-vga-gl + gtk + gl, minimal serial
make run-fs           # adds test_disk_image.fat32.img as second drive
QEMUFLAGS="-m 4G" make run   # override -m 2G default (KARCH=x86_64, QEMUFLAGS appended)
```

QEMU details: `-M q35 -accel kvm -cpu host,+tsc-deadline,+apic -device isa-debug-exit,iobase=0xf4,iosize=0x04 -monitor telnet:127.0.0.1:1234,server,nowait`. Exit codes `kernel/src/main.rs:47-51`: `0x10` success, `0x11` failed. Remove `-no-reboot` if stuck on black screen (see `notes.md`).

## Logging — Unix Sockets, Not Stdio
`make run` / `run-x86_64` spawns `scripts/log_splitter.py` before QEMU:
- `COM1` (`/tmp/mofuos_com1.sock`) = kernel `serial_println_core!` → `logs/<YYYY-MM-DD_HH-MM-SS>/all.txt` + `core_N.txt` (routed by `^\[Core\s+(\d+)`, `MAX_CORES=4` — keep `kernel/src/lib.rs:13` and `scripts/log_splitter.py:22` in sync)
- `COM2` (`/tmp/mofuos_com2.sock`) = userspace → `all.txt` + `userspace_pid_<pid>.txt` (routed by `[pid=N]` tag)
- Also prints to stdout. `logs/` is gitignored. Single-socket/stdin fallback supported.

## Architecture — Where Things Live
```
linker-x86_64.ld / x86_64-kernel.json   # kernel ELF layout: ENTRY(kmain), 0xffffffff80000000, lld-elf, large code-model, -mmx, disable-redzone
.cargo/config.toml                      # default target x86_64-unknown-none + build-std=[core,alloc] + linker flag
kernel/src/main.rs                      # BSP entry, SMP wait (AP_CORES_READY), compositor loop (hlt)
kernel/src/lib.rs                       # HHDM_OFFSET=0xFFFF_8000_0000_0000, MAX_CORES=4, feature flags RUN_THEOPHE etc.
kernel/src/{boot,interrupts,gdt,memory,graphics,filesystem,process,programs,tests_exp}/
kernel/src/asm/ap_trampoline.S + ap_trampoline.ld  # built by kernel/build.rs via cc/ld/objcopy → $OUT_DIR/ap_trampoline.bin (env AP_TRAMPOLINE_BIN)
kernel/src/process_start.rs             # create_userspace_processes() called after AP cores ready
user/crt0.c + libc/ + linker.ld        # C runtime: _start → main → sys_exit
user/programs/{first,test,ping,ping_yield}/  # C programs linked with crt0.o + libc.a
user/rustspace/x86_64-user.json + src/bin/{rust_first,window_app,theophe,fs_test}.rs  # Rust userspace (small code-model)
disk_templates/fat32_os_disk_template_default/  # staged into storage/ata_disk.img via scripts/create_ata_disk.sh
limine.conf                             # Limine protocol, kernel_path boot():/boot/kernel
storage/ata_disk.img                    # generated FAT32 ATA image (qemu ide-hd, content from disk_templates)
```

Workspace illusion: `cargo test` / `cargo clippy` at root is mostly useless (bare-metal, `#![no_std]` `#![no_main]` `#![feature(abi_x86_interrupt, allocator_api, portable_simd)]` + `#![allow(warnings,unused)]`). Verify kernel changes by `make all` or `make -C kernel`; test suites live in `kernel/src/tests_exp/` and run inside QEMU (e.g., `test_ata_filesystem()` called from `kernel/src/main.rs:81`).

## Conventions & Gotchas
- **Generated/ignored:** `limine/`, `ovmf/`, `target/`, `iso_root/`, `*.iso`/`*.hdd`, `storage/ata_disk.img`, `logs/`, `user/crt0.o`/`libc.a` — don't edit or commit.
- **ATA disk:** `scripts/create_ata_disk.sh -i disk_templates/fat32_os_disk_template_default -o ata_disk.img -s 64` writes to `storage/`. Makefile target `ata-disk` and `test_disk_image.fat32.img` dependencies — if `create_fat32_image.sh` missing (referenced in `GNUmakefile:267`), disk image won't build.
- **Kernel `Cargo.toml` features:** `default = ["use_cached_fs"]` — disable with `--no-default-features` to test uncached FS.
- **Formatting:** `user/rustspace` Makefile runs `cargo fmt` before build; kernel has no fmt CI — run `cargo +nightly fmt` manually.
- **No CI / no pre-commit hooks** — verify locally with `make all` (ISO boots in QEMU = success).
- **SMP:** `MAX_CORES=4` (1 BSP + 3 APs in QEMU `-smp cores=3`). Changing it requires updating `kernel/src/lib.rs`, `scripts/log_splitter.py`, and QEMU flags together.
