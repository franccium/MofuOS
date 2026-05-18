.section .text
.code16
.global ap_trampoline_start
ap_trampoline_start:
    cli
    cld
    
    # Diagnostic: we started
    movw $0xDEAD, %ax
    movw %ax, 0x8FF0
    
    xor %ax, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    movw $0xBEEF, 0x8FF2   # Segments set up
    
    # Load GDT pointer
    lgdtl (gdtr)
    movw $0xCAFE, 0x8FF4   # GDT loaded
    
    # Enable protected mode
    movl %cr0, %eax
    orl $1, %eax
    movl %eax, %cr0
    movw $0xC001, 0x8FF6   # Protected mode enabled
    
    # Far jump to 32-bit code using 32-bit code segment (0x08)
    ljmp $0x08, $ap_protected_mode
    
.code32
ap_protected_mode:
    movw $0xD0D0, 0x8FF8   # In 32-bit mode

    
    # Set up data segments with 32-bit data segment (0x10)
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    movw $0xD0D1, 0x8FF9   # Segments set
    
    # Enable PAE
    movl %cr4, %eax
    orl $0x20, %eax
    movl %eax, %cr4
    movw $0xDA1E, 0x8FFA   # PAE enabled
    
    movl $0x9000, %edi
    movl %edi, 0x8FF4
    movl (%edi), %eax
    movl %eax, 0x8FFA    # Write CR3 value before loading
    movl %eax, %cr3

    # Load CR3
    movl $0x9000, %edi
    movl (%edi), %eax
    movl %eax, %cr3
    movw $0xDB0E, 0x8FFC   # CR3 loaded
    
    # Enable long mode in EFER
    movl $0xC0000080, %ecx
    rdmsr
    orl $0x100, %eax        # Set LME (Long Mode Enable)
    wrmsr
    movw $0xDC0E, 0x8FFE   # Long mode enabled


    
    # Enable paging (this activates long mode since LME is set)
    movl %cr0, %eax
    orl $0x80000000, %eax   # Set PG (Paging)
    movl %eax, %cr0
    movw $0xDD0E, 0x8FF0   # Paging enabled!


    
    # Now we're in compatibility mode (32-bit with paging)
    # Far jump to 64-bit code using 64-bit code segment (0x18)
    ljmp $0x18, $ap_long_mode
    
.code64
ap_long_mode:
    # Now in full 64-bit mode
    movw $0x10, %ax         # Still using data segment 0x10
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss

    xor %rdi, %rdi

    
    # We're in 64-bit mode!
    #movl $0x6464B007, 0x8F00

wait_for_entry:
    mov $0x9010, %edi
    mov (%rdi), %rax
    test %rax, %rax                # Check if entry point is non-zero
    jz wait_for_entry              # Wait for BSP to set it
    
    # Also verify stack is set up
    mov $0x9008, %edi
    mov (%rdi), %rsp
    test %rsp, %rsp
    jz wait_for_entry              # Wait for valid stack
    
    
    # Set up stack
    mov $0x9008, %edi
    mov (%rdi), %rsp

    #movq %rsp, 0x8FF0
    # try to write into stack
    movq $0xBEEF, (%rsp)

    # Clear RFLAGS
    pushq $0
    popfq
    #jmp .
    
    # Signal we're ready to jump
    # MAGIC_OFFSET == 0x8FF0
    movl $0x41505354, 0x8FF0
    
    # Jump to Rust entry
    xor %rdi, %rdi
    mov $0x9010, %edi
    mov (%rdi), %rax

    # Signal the jump target
    movq %rax, 0x8FF8

    jmp .

    jmp *%rax
    
    # Should never get here
    hlt

.align 16
gdt:
    .quad 0x0000000000000000  # Null descriptor (0x00)
    .quad 0x00CF9A000000FFFF  # 32-bit code segment (0x08) - base=0, limit=4GB, DPL=0
    .quad 0x00CF92000000FFFF  # Data segment (0x10) - base=0, limit=4GB, DPL=0
    .quad 0x00AF9A000000FFFF  # 64-bit code segment (0x18) - base=0, limit=4GB, DPL=0, 64-bit
gdt_end:

.align 4
gdtr:
    .word gdt_end - gdt - 1
    .long gdt

.global ap_trampoline_end
ap_trampoline_end: