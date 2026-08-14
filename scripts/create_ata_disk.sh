#!/usr/bin/env bash
# Usage: ./scripts/create_ata_disk.sh -i [input_template] -o [output_file] -s [size_mb]
set -e

INPUT_TEMPLATE=""
OUTPUT_FILENAME="ata_disk.img"
SIZE_MB=256

while getopts "i:o:s:h" opt; do
    case $opt in
        i)
            INPUT_TEMPLATE="$OPTARG"
            ;;
        o)
            OUTPUT_FILENAME="$OPTARG"
            ;;
        s)
            SIZE_MB="$OPTARG"
            ;;
        h)
            echo "Usage: $0 -i <input_template_dir> -o <output_filename> -s <size_mb>"
            echo "  -i  Input template directory (required)"
            echo "  -o  Output filename (default: ata_disk.img)"
            echo "  -s  Size in MB (default: 256)"
            echo "  -h  Show this help message"
            exit 0
            ;;
        \?)
            echo "Invalid option: -$OPTARG" >&2
            exit 1
            ;;
        :)
            echo "Option -$OPTARG requires an argument." >&2
            exit 1
            ;;
    esac
done

if [ -z "$INPUT_TEMPLATE" ]; then
    echo "Error: Input template directory is required. Use -i to specify." >&2
    exit 1
fi

if [ ! -d "$INPUT_TEMPLATE" ]; then
    echo "Error: Input template directory '$INPUT_TEMPLATE' does not exist." >&2
    exit 1
fi

OUT_DIR="storage"
mkdir -p "${OUT_DIR}"
OUTPUT="${OUT_DIR}/${OUTPUT_FILENAME}"

echo "Creating ${SIZE_MB}MB FAT32 disk image: ${OUTPUT}"
echo "Using template directory: ${INPUT_TEMPLATE}"

dd if=/dev/zero of="${OUTPUT}" bs=1M count="${SIZE_MB}" status=none
mkfs.fat -F 32 -n "MOFUOS" "${OUTPUT}"

if [ -d "${INPUT_TEMPLATE}" ]; then
    file_count=0
    for f in "${INPUT_TEMPLATE}"/*; do
        if [ -f "$f" ]; then
            mcopy -i "${OUTPUT}" "$f" "::/$(basename "$f")"
            echo "  Copied $(basename "$f")"
            file_count=$((file_count + 1))
        fi
    done
    
    if [ $file_count -eq 0 ]; then
        echo "  No files found in template directory"
    else
        echo "  Copied $file_count file(s) total"
    fi
fi

echo "Done: ${OUTPUT} ($(du -sh "${OUTPUT}" | cut -f1))"

exit 0