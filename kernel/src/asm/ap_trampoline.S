# ap_real_to_long.S - Real mode entry point for AP cores
# This code runs at physical address 0x8000
# It is linked with ap_trampoline.ld to address 0x8000, so all symbol references are correct

.section .text
.code16
.global ap_trampoline_start
ap_trampoline_start:
    cli
    cld
    
    # Set up segment registers
    xor %ax, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss

    # loop here to debug
    jmp .
    
    # Load GDT pointer - gdtr is defined at end of this file
    lgdt (gdtr)
    
    # Enable protected mode
    mov %cr0, %eax
    or $1, %al
    mov %eax, %cr0
    
    # Far jump to 32-bit code
    ljmp $0x08, $ap_protected_mode
    
.code32
ap_protected_mode:
    # Set up segment registers for protected mode
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    
    # Enable PAE (Physical Address Extension)
    mov %cr4, %eax
    or $0x20, %eax
    mov %eax, %cr4
    
    # Load CR3 - the Rust code writes the kernel's CR3 to address 0x9000
    mov $0x9000, %edi
    mov (%edi), %eax
    mov %eax, %cr3
    
    # Enable long mode
    mov $0xC0000080, %ecx
    rdmsr
    or $0x100, %eax
    wrmsr
    
    # Enable paging
    mov %cr0, %eax
    or $0x80000000, %eax
    mov %eax, %cr0
    
    # Far jump to 64-bit code
    ljmp $0x08, $ap_long_mode
    
.code64
ap_long_mode:
    # Set up 64-bit segment registers
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    
    # Clear RFLAGS
    pushq $0
    popfq
    
    # Load the entry point address from memory
    # The Rust code writes the kernel entry point address to 0x9010
    mov $0x9010, %edi
    mov (%rdi), %rax
    
    # Jump to the Rust entry point
    jmp *%rax

# GDT for AP startup
# Placed at end of trampoline so it's within reach
.align 8
gdtr:
    .word gdt_end - gdt - 1
    .long gdt  # 32-bit GDT address (valid since we're at 0x8000)

gdt:
    .quad 0x0000000000000000  # Null descriptor
    .quad 0x00AF9A000000FFFF  # 64-bit code segment
    .quad 0x00CF92000000FFFF  # Data segment
gdt_end:

.global ap_trampoline_end
ap_trampoline_end: