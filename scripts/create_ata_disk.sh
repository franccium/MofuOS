#!/usr/bin/env bash
# Usage: ./scripts/create_ata_disk.sh [output_file] [size_mb]
set -e

OUTPUT="${1:-ata_disk.img}"
SIZE_MB="${2:-256}"

echo "Creating ${SIZE_MB}MB FAT32 disk image: ${OUTPUT}"

dd if=/dev/zero of="${OUTPUT}" bs=1M count="${SIZE_MB}" status=none
mkfs.fat -F 32 -n "MOFUOS" "${OUTPUT}"

# Copy any files from os_disk_fat32/ if it exists
if [ -d "os_disk_fat32" ]; then
    for f in os_disk_fat32/*; do
        [ -f "$f" ] && mcopy -i "${OUTPUT}" "$f" "::/$(basename $f)" && echo "  Copied $(basename $f)"
    done
fi

echo "Done: ${OUTPUT} ($(du -sh "${OUTPUT}" | cut -f1))"
