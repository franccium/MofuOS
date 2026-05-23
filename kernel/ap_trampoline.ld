/* Linker script for AP trampoline code
 * This script links the AP trampoline at its actual physical address (0x8000)
 * to avoid relocation overflow issues when embedding in a kernel at high addresses
 */

OUTPUT_FORMAT(elf64-x86-64)
ENTRY(ap_trampoline_start)

SECTIONS
{
    /* Place at actual physical address where AP code runs */
    . = 0x8000;

    .text : {
        *(.text .text.*)
    }

    .data : {
        *(.data .data.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    /* Discard sections we don't need */
    /DISCARD/ : {
        *(.note.*)
        *(.eh_frame*)
    }
}
