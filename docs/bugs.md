# MofuOS — Confirmed Bugs

These are concrete code defects found by reading the source. Not speculation.
Each entry has the file, the exact problem, and the consequence.

---

## BUG-01: Single global syscall stack — SMP corruption [RESOLVED]

Fixed in `kernel/src/process/syscall.rs`.

Each core now has its own 64 KiB stack inside `PER_CORE_SYSCALL: [PerCoreSyscallData; MAX_CORES]`.
`init_syscall()` writes the per-core slot address into `KERNEL_GS_BASE` MSR (0xC0000102).
The naked asm prologue uses `swapgs` + `mov rsp, gs:0` to load the core-local stack top
in two instructions with no lock and no MMIO. `init_syscall_stack()` was removed.

See `notes/agents/process.md` — Syscall Stack section for full details.

---

## BUG-02: sys_exit halts core, never returns to scheduler [RESOLVED]

Fixed in `kernel/src/process/scheduler.rs`, `kernel/src/process/syscall.rs`,
and `kernel/src/gdt.rs`.

Four sub-problems found and fixed across two sessions:

### Sub-problem 1: sys_exit used hlt loop

sys_exit (syscall 999) had no way to return to the scheduler — it just halted.
Fix: added `return_to_scheduler()` which restores the saved kernel RSP and
executes `ret`, landing back at the return label in `run_on_core_loop`.

### Sub-problem 2: slot write after CR3 switch (protection fault)

The initial asm block attempted `mov [slot], rsp` after `mov cr3, {user_cr3}`.
`KERNEL_RSP_ON_CORE` is a kernel static not mapped in the user PML4. Writing to
it with the user page table active caused a PROTECTION_VIOLATION page fault.
Fix: order of operations in the asm block must be:
  1. `lea rax, [rip + 2f]` — compute return label address
  2. `push rax`             — push return address (adjusts RSP)
  3. `mov [slot], rsp`      — save RSP (kernel PML4 still active)
  4. `mov cr3, {cr3}`       — switch to user page table
  5. `jmp jump_to_userspace`

### Sub-problem 3: Limine AP boot stack too small

`push rax` in the scheduler loop faulted because the AP core was still on the
small Limine bootstrap stack. Fix: `run_on_core_loop` immediately switches RSP
to a dedicated per-core `SCHEDULER_STACKS` entry (32 KiB, `.bss`) at the top
of the function via `gdt::get_scheduler_stack_top(core_id)`.

### Sub-problem 4: register alias in inline asm caused CR3 = label address (GPF)

The original asm block used `cr3 = in(reg) cr3` which the compiler allocated to
RAX. Then `lea rax, [rip + 2f]` overwrote RAX with the return label's address.
The CPU then executed `mov rax, cr3` — writing a kernel text address into CR3
(not a valid 4KB-aligned physical PML4 address). This triggered a General
Protection Fault (error code 0) immediately after the push, before iretq.

Confirmed by disassembly:
```asm
lea  0x9(%rip), %rax    ; label address -> rax
push %rax
mov  %rax, %cr3         ; BUG: cr3 = label address, not the PML4 phys addr
jmp  jump_to_userspace
```

Fix: pin each operand to an explicit register so the compiler cannot alias them:
- `in("r8")  kernel_rsp_slot` — slot pointer
- `in("rcx") cr3`             — PML4 physical address
- `in("rdi") rip`             — userspace entry point
- `in("rsi") rsp`             — userspace stack pointer
- `lateout("rax") _`          — rax used only for the lea, declared clobbered

Correct generated asm:
```asm
lea  0xc(%rip), %rax    ; return label address
push %rax               ; push onto scheduler stack
mov  %rsp, (%r8)        ; save RSP into kernel_rsp_slot
mov  %rcx, %cr3         ; switch to user PML4
jmp  jump_to_userspace  ; enter userspace via iretq
; label "2:" — return_to_scheduler() ret lands here
```

Note: RBX is reserved by LLVM for `x86_64-unknown-none` and cannot be used as
an inline asm operand. R8 is safe.

### Final mechanism

- `gdt.rs`: `SCHEDULER_STACKS: [KernelStack; MAX_CORES]` + `get_scheduler_stack_top(core_id) -> u64`
- `scheduler.rs`:
  - `run_on_core_loop` switches to scheduler stack immediately on entry.
  - Before userspace: one asm block with all five steps in order (see above).
  - `KERNEL_RSP_ON_CORE: static mut [u64; MAX_CORES]` stores saved RSP per core.
  - `return_to_scheduler()`: `mov rsp, KERNEL_RSP_ON_CORE[core_id]; ret`
- `syscall.rs`: sys_exit (999) calls `scheduler::return_to_scheduler()`.


## BUG-05: Physical frame leaked on `PageAlreadyMapped` in `map_virt_mem_region`

File: `kernel/src/memory/usermem.rs`

```rust
let phys_frame = frame_allocator.allocate_frame()...;
unsafe {
    match user_page_mapper.map_to(page, phys_frame, user_flags, frame_alloc) {
        Ok(flush) => { flush.flush(); }
        Err(MapToError::PageAlreadyMapped(_existing_frame)) => {
            // merges flags — but phys_frame allocated above is never freed
            ...
        }
    }
}
```

A physical frame is allocated before `map_to` is attempted. If `map_to` returns
`PageAlreadyMapped`, the allocated frame is discarded without being returned to
the allocator. Since `MemoryMapFrameAllocator` has no `deallocate_frame`, the
frame is permanently lost.

This happens for every ELF segment boundary page that is shared between two
PT_LOAD segments (which is common in multi-segment ELF binaries).

Fix: Implement `deallocate_frame` on the allocator

Severity: LOW at current scale (a few pages per process), but worsens with many
processes and will cause OOM on a constrained system.

---

## BUG-06: Framebuffer pixel format not validated against Limine mask fields

File: `kernel/src/graphics/framebuffer.rs`, `write_pixel` method

```rust
let color: u32 =
    ((color.r() as u32) << 16) | ((color.g() as u32) << 8) | (color.b() as u32);
unsafe { px_ptr.write(color) };
```

The Limine Framebuffer struct provides `red_mask_size`, `red_mask_shift`,
`green_mask_size`, `green_mask_shift`, `blue_mask_size`, `blue_mask_shift`.
The code ignores these and hardcodes XRGB (R at bit 16, G at bit 8, B at bit 0).

This works in QEMU (which presents XRGB), but would produce wrong colors or a
blank screen on hardware with a different framebuffer layout (e.g., BGRX, BGR565).

Fix: Use the mask fields to compose pixel values:
```rust
let color: u32 =
    ((r as u32) << fb.red_mask_shift) |
    ((g as u32) << fb.green_mask_shift) |
    ((b as u32) << fb.blue_mask_shift);
```

Severity: LOW for QEMU-only use. HIGH if ever run on real hardware.

---

## BUG-07: `IdentityAcpiHandler` IO methods all panic with `todo!()`

File: `kernel/src/memory/memory.rs`

```rust
fn read_u8(&self, __address: usize) -> u8 { todo!() }
fn write_u32(&self, __address: usize, _value: u32) { todo!() }
// ... (all IO methods)
```

The ACPI handler used to parse tables only implements `map_physical_region`. All
`read_u8/u16/u32/u64`, `write_*`, `read_io_*`, `write_io_*`, and `read_pci_*`
methods panic. If any ACPI table parsing code path (e.g., AML execution) tries to
call these, the kernel panics.

Currently this does not trigger because ACPI table enumeration (MADT parsing for
interrupt topology) does not go through these IO methods in the `acpi` crate's
table-parse path. But any deeper ACPI use (power management, AML, PCI enumeration)
would panic immediately.

Fix: Implement the IO methods properly, or document explicitly that this handler
is read-only/table-parse-only and must not be used for full ACPI.

Severity: LOW for current use. HIGH if ACPI scope is expanded.

---

---

## BUG-08: SyscallFrame missing push rax — all syscalls dispatched to _ => u64::MAX [RESOLVED]

File: `kernel/src/process/syscall.rs` — `syscall_handler` naked asm

The syscall handler pushed 14 registers but never pushed RAX (the syscall
number). `SyscallFrame.syscall_num` at offset +0x58 from the frame base read
from an uninitialized stack slot. The `transmute` to `SyscallNumber` produced
garbage, hitting `_ => u64::MAX` on every call. All syscalls silently returned
u64::MAX. Processes appeared to run (no crash) but produced no output and
sys_exit looped forever.

Fix: add `push rax` after `push rdi` (arg1). On the pop side, `add rsp, 8`
skips the slot because RAX already holds the return value from
`handle_syscall_inner` and must not be overwritten.

---

## BUG-09: Intel SYSRETQ leaves SS=0x18 (RPL=0) — timer iretq GPFs [RESOLVED]

Files: `kernel/src/interrupts.rs`, `kernel/src/process/syscall.rs`

Intel SYSRETQ sets CS with RPL=3 but sets SS WITHOUT ORing RPL=3, leaving
SS=0x18 (kernel data descriptor, RPL=0). The qemu64 CPU model follows Intel
behavior. After any syscall, the user process runs with SS=0x18. When the
timer fired in Ring3, the interrupt frame captured SS=0x18. The compiler-
generated iretq back to Ring3 GPFs because iretq requires SS RPL == CS RPL.

This only manifested because PREEMPTION_ENABLED was false, so the timer
handler fell through to the compiler iretq instead of calling
return_to_scheduler() which would bypass it.

Fix:
- SFMASK = 1<<9 (mask IF on SYSCALL entry) — timer cannot fire while handler
  runs with kernel SS. Re-enable interrupts at start of handle_syscall_inner.
  cli before return_to_scheduler() in sys_yield and sys_exit.
- PREEMPTION_ENABLED = true.
- When in Ring3 with processes waiting: preempt via return_to_scheduler()
  (bypasses compiler iretq entirely).
- When in Ring3 with nothing waiting: patch SS slot in interrupt frame to 0x1b
  before iretq. Frame layout: RIP(+0), CS(+8), RFLAGS(+16), RSP(+24), SS(+32).
  `(addr_of!(*stack_frame) as *mut u64).add(4).write_volatile(0x1b)`

---

## BUG-10: sys_map_window_buffer missing USER_ACCESSIBLE — page fault on every pixel write [RESOLVED]

File: `kernel/src/process/syscall.rs` — `SyscallNumber::MapWindowBuffer`

Window back buffer pages were mapped with PRESENT | WRITABLE | NO_EXECUTE
but without USER_ACCESSIBLE. Pages without this flag are kernel-only.
Every pixel write from Ring3 caused a silent page fault (handler printed
nothing because sys_write was also broken by BUG-08 at the time).

Fix: add PageTableFlags::USER_ACCESSIBLE to the mapping flags.
Note: map_specific_frame in usermem.rs also OR-s USER_ACCESSIBLE in
automatically, so the explicit flag is now redundant but harmless.

---

## BUG-11: translate_kernel_heap_virt_to_phys used vaddr - HHDM_OFFSET — mapped garbage physical pages [RESOLVED]

File: `kernel/src/memory/usermem.rs`

`translate_kernel_heap_virt_to_phys` computed `phys = vaddr - HHDM_OFFSET`.
This is only correct for memory that is HHDM-mapped (physical RAM directly
mapped at HHDM_OFFSET). The kernel heap at 0xFFFF_8080_0000_0000 is NOT
HHDM-mapped: init_heap allocates individual physical frames from the bump
allocator and maps them via map_to. The frames are at low physical addresses
(< 2GB). vaddr - HHDM_OFFSET produces addresses in the ~512GB range, far
outside the machine's RAM.

sys_map_window_buffer used this to find the physical pages of the window back
buffer, then mapped those wrong addresses into the user page table. User pixel
writes went to garbage physical memory; the compositor's buffer (at the real
frames) stayed zero — window appeared permanently black.

Fix: walk the kernel page table using translate_user_virt_to_phys with
self.kernel_page_table_phys as the PML4 root. This correctly resolves any
kernel virtual address to its physical frame regardless of mapping method.
