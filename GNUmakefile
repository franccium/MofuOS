# MofuOS — GNUmakefile shim
# Deprecated: use `cargo xtask` / `cargo x` directly.
# This file delegates to xtask for backwards compatibility.
# See `cargo xtask --help` for tasks.

MAKEFLAGS += -rR
.SUFFICES:

# Map legacy make targets to cargo xtask
.PHONY: all all-hdd run run-hdd fat32-image ata-disk kernel user-programs clean clean-fat32
.PHONY: run-x86_64 run-x86_64-ata run-nologs run-fast-x86_64 run-hdd-x86_64 run-fs run-fs-x86_64 run-bios run-hdd-bios

all:
	@echo "[make] deprecated — delegating to: cargo xtask iso"
	cargo xtask iso

all-hdd:
	@echo "[make] deprecated — delegating to: cargo xtask hdd"
	cargo xtask hdd

run:
	@echo "[make] deprecated — delegating to: cargo xtask run"
	cargo xtask run

run-hdd:
	cargo xtask run-hdd

run-x86_64:
	cargo xtask run

run-x86_64-ata:
	cargo xtask run

run-nologs:
	cargo xtask run-nologs

run-fast-x86_64:
	cargo xtask run-fast

run-hdd-x86_64:
	cargo xtask run-hdd

run-fs:
	cargo xtask run-fs

run-fs-x86_64:
	cargo xtask run-fs

run-bios:
	cargo xtask run-bios

run-hdd-bios:
	cargo xtask run-bios

fat32-image:
	cargo xtask fat32-image

ata-disk:
	cargo xtask ata-disk

kernel:
	cargo xtask build

user-programs:
	cargo xtask build

clean:
	cargo xtask clean

clean-fat32:
	rm -f test_disk_image.fat32.img target/test_disk_image.fat32.img

# Fallthrough for any other xtask task
%:
	cargo xtask $@

# Legacy artefacts that previously provided limine/ovmf via make
limine/limine:
	@echo "[make] limine is now managed by xtask at target/limine — run: cargo xtask iso"

edk2-ovmf:
	@echo "[make] OVMF is now managed by xtask at target/ovmf — it auto-discovers /usr/share/OVMF"

template-x86_64.iso:
	cargo xtask iso

template-x86_64.hdd:
	cargo xtask hdd

test_disk_image.fat32.img:
	cargo xtask fat32-image
