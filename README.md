# MofuOS

## Running

### Prerequisites

- QEMU
- llvm tools (`llvm-ar`, `ld.lld`, `objcopy`, `clang`)
- `xorriso`, `mtools` (`mmd`/`mcopy`), `parted`, `mkfs.fat`, `cc`/`ld`
- `rustc nightly` (`rust-toolchain.toml` pins nightly + `rust-src` `llvm-tools-preview`)

for Debian systems:

`sudo apt install -y qemu-system-x86 llvm xorriso mtools parted dosfstools ovmf`

### Instructions

This project uses `cargo xtask` (short alias `cargo x`) for all build/run tasks.
Run `cargo xtask --help` to see available tasks. `make` is kept as a deprecated shim
that delegates to `cargo xtask`.

```bash
cargo xtask build        # build kernel + user programs
cargo xtask iso          # build bootable ISO (target/template-x86_64.iso)
cargo xtask hdd          # build bootable HDD (target/template-x86_64.hdd)
cargo xtask run          # build ISO + ATA disk and launch QEMU (sockets + log_splitter.py)
cargo xtask run-nologs   # QEMU with -serial stdio
cargo xtask run-fast     # QEMU with virtio-vga-gl + gtk
cargo xtask run-fs       # ISO + FAT32 test disk
cargo xtask ata-disk     # create storage/ata_disk.img
cargo xtask clean        # cargo clean
cargo xtask fmt          # format all Rust code
cargo xtask clippy       # lint all Rust code
```

Compile the kernel and generate an ISO image: `cargo xtask iso` (legacy: `make all`)

Build the kernel and the ISO image and run using `qemu`: `cargo xtask run` (legacy: `make run`)

OVMF is auto-discovered from `/usr/share/OVMF` etc and copied to `target/ovmf/`.
Legacy `ovmf/` directory at repo root still works but is deprecated.
Limine is managed at `target/limine/` (cloned from `v10.x-binary` on demand).

I recommend setting up `qemu-kvm` for hardware acceleration

