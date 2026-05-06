; ap_real_to_long.S - Real mode entry point for AP cores
.section .text
.code16
.global ap_trampoline_start
ap_trampoline_start:
    cli
    cld
    
    ; Set up segment registers
    xor %ax, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    
    ; Load GDT pointer (you need a special GDT for AP startup)
    lgdt (gdtr)
    
    ; Enable protected mode
    mov %cr0, %eax
    or $1, %al
    mov %eax, %cr0
    
    ; Far jump to 32-bit code
    ljmp $0x08, $ap_protected_mode
    
.code32
ap_protected_mode:
    ; Set up segment registers for protected mode
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    
    ; Enable PAE (Physical Address Extension)
    mov %cr4, %eax
    or $0x20, %eax
    mov %eax, %cr4
    
    ; Load CR3 with kernel page table
    mov $kernel_page_table, %eax
    mov %eax, %cr3
    
    ; Enable long mode
    mov $0xC0000080, %ecx
    rdmsr
    or $0x100, %eax
    wrmsr
    
    ; Enable paging
    mov %cr0, %eax
    or $0x80000000, %eax
    mov %eax, %cr0
    
    ; Far jump to 64-bit code
    ljmp $0x08, $ap_long_mode
    
.code64
ap_long_mode:
    ; Set up 64-bit GDT
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    
    ; Clear RFLAGS
    pushq $0
    popfq
    
    ; Load 64-bit IDT
    lidt (idtr64)
    
    ; Jump to Rust entry point
    mov $ap_core_entry_point, %rax
    jmp %rax

; GDT for AP startup (at known location)
.align 8
gdtr:
    .word gdt_end - gdt - 1
    .quad gdt + 0xFFFF800000000000  ; Add HHDM offset

gdt:
    .quad 0x0000000000000000  ; Null descriptor
    .quad 0x00AF9A000000FFFF  ; 64-bit code segment
    .quad 0x00CF92000000FFFF  ; Data segment
gdt_end:

idtr64:
    .word 0xFFFF  ; IDT limit
    .quad kernel_idt_base  ; IDT base address with HHDM offset

.global ap_trampoline_end
ap_trampoline_end: