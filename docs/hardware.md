# MofuOS — Hardware Abstraction: GDT/IDT/TSS, APIC, SMP, Serial

## Files

```
kernel/src/gdt.rs         — per-core GDT, TSS, selector accessors
kernel/src/interrupts.rs  — IDT, LAPIC, IOAPIC, TSC, PIT calibration, timer
kernel/src/util/
  apic.rs    — APICOffset enum (MMIO register offsets)
  msr.rs     — msr_read / msr_write wrappers
  cpuinfo.rs — CpuInfo, CpuFeatureFlags, per-core init, AP trampoline entry, SMP
kernel/src/asm/
  ap_trampoline.S   — real-mode → 64-bit transition for AP cores
  mod.rs            — include_bytes! for the trampoline binary blob
kernel/src/io/
  serial.rs  — COM1 (kernel logs), COM2 (userspace output), macros
```

## GDT / TSS (gdt.rs)

One GDT + TSS per CPU core. Max cores: `MAX_CORES = 16`.

### Static Allocations

All in `.bss` (zero-initialized at load, permanent lifetime):
```rust
static mut RSP0_STACKS:   [KernelStack; MAX_CORES]  // 16 * 32 KiB = 512 KiB
static mut IST0_STACKS:   [KernelStack; MAX_CORES]  // 16 * 32 KiB = 512 KiB
static mut PER_CORE_GDT:  [Gdt;        MAX_CORES]
static mut PER_CORE_TSS:  [UnsafeCell<TaskStateSegment>; MAX_CORES]
```

`PER_CORE_STACK_SIZE = 8 * 4096 = 32 KiB` per stack.

### GDT Segment Layout (per core)

| Slot | Selector | Type                         |
|------|----------|------------------------------|
| 0    | null     | null descriptor              |
| 1    | 0x08     | kernel_code_segment (ring 0) |
| 2    | 0x10     | kernel_data_segment (ring 0) |
| 3    | 0x18     | user_data_segment (ring 3)   |
| 4    | 0x20     | user_code_segment (ring 3)   |
| 5    | 0x28+    | tss_segment (64-bit TSS, 2 slots) |

STAR MSR userspace: CS = 0x23 (0x20 | 3), SS = 0x1B (0x18 | 3).

### TSS Configuration (per core)

- `privilege_stack_table[0]` = top of `RSP0_STACKS[core_id]`
  Used on any ring-3 → ring-0 transition (interrupts, exceptions, SYSCALL via INT).
  Without this, hardware tries to switch to RSP=0 → immediate triple fault.
- `interrupt_stack_table[DOUBLE_FAULT_IST_INDEX]` = top of `IST0_STACKS[core_id]`
  Used by the double-fault handler. Without a dedicated IST stack, a double fault on
  a corrupt RSP causes a triple fault.

`DOUBLE_FAULT_IST_INDEX = 0`

### init_core_gdt(core_id: u8)

1. Build `Gdt::new(core_id)` — sets up RSP0 and IST0 in the per-core TSS.
2. Stores in `PER_CORE_GDT[core_id]`.
3. `gdt.table.load()` — LGDT.
4. Reload CS via far return (`retfq` idiom):
   `push selector; lea rip_label; push rip; retfq`
5. Set DS/ES/SS to kernel data selector.
6. `load_tss(tss_selector)` — LTR.

Called on BSP (core 0) in `kmain`, and on each AP core in their boot entry point.

### Selector Accessors

```rust
pub fn get_kernel_code_selector() -> SegmentSelector
pub fn get_kernel_data_selector() -> SegmentSelector
pub fn get_user_code_selector()   -> SegmentSelector
pub fn get_user_data_selector()   -> SegmentSelector
pub fn get_tss_selector()         -> SegmentSelector
```

All look up `PER_CORE_GDT[get_current_core_id()]`.

## IDT / Interrupts (interrupts.rs)

`lazy_static! static ref IDT: InterruptDescriptorTable`

Handlers registered: page fault, double fault, general protection fault, timer
(APIC timer), keyboard, spurious interrupt, and others. Double fault uses IST
index 0 (DOUBLE_FAULT_IST_INDEX).

`pub fn load_idt()` — calls `IDT.load()`. Called on BSP in `kmain` only; APs
share the same IDT (it's a static ref, the address is the same on all cores).

### TSC and System Time

```rust
static TSC_FREQUENCY_HZ: AtomicU64  // measured at boot, shared across cores
static BOOT_TSC: AtomicU64          // TSC value at boot epoch
static SYSTEM_TICKS: AtomicU64      // incremented by timer interrupt
```

`TIMER_TICK_INTERVAL_MS = 10ms` → `TIMER_TICK_FREQ_HZ = 100 Hz`
`TICK_DURATION_NS = 10_000_000 ns`

`init_tsc_globals()` — measures TSC frequency via PIT channel 2 (one-shot mode,
~55ms window), records boot TSC. Must be called before APs start.

`tsc_timestamp_us()` — used by `serial_println_core!` to prepend timestamps.

`system_uptime_ns()` — returns nanoseconds since boot using TSC and measured frequency.

### APIC Timer

Timer mode: **TSC-Deadline** (`MSR_IA32_TSC_DEADLINE = 0x6E0`).
Each core programs its own LAPIC timer to fire at `TSC + ticks_per_interval`.

`pub const TIMER_TICK_INTERVAL_MS: u64 = 10`

Timer interrupt vector: `0x20` (typical; check IDT setup in interrupts.rs).

On each tick:
- Increment `SYSTEM_TICKS`.
- Call `SCHEDULER.lock().on_timer_tick(core_id)`.
- Re-arm TSC-Deadline.

### LAPIC

Virtual base: `LAPIC_VIRT_BASE = 0xFFFF_FFFF_0000_0000`
Physical address: read from APIC_BASE MSR (0x1B).

`map_local_apic_for_current_core` — maps the LAPIC MMIO page into the kernel page
table at `LAPIC_VIRT_BASE + core offset`.

`init_lapic_for_current_core(core_id)` — enables LAPIC (SVR register bit 8), sets up
the timer.

LAPIC register access helpers in `util/apic.rs`:
```rust
pub unsafe fn lapic_read(offset: APICOffset) -> u32
pub unsafe fn lapic_write(offset: APICOffset, value: u32)
```

`APICOffset` is a comprehensive enum of all LAPIC register offsets (IDr, Eoi, Svr,
LvtT, Ticr, Tccr, Tdcr, Icr1, Icr2, etc.).

### IOAPIC

Physical base determined from ACPI MADT.
Virtual mapped at `IOAPIC_VIRT_BASE = 0xFFFF_FFFF_FF00_0000`.

Used for routing hardware IRQs (keyboard on IRQ1 → vector 0x21 typically).

### ACPI

`init_acpi(rsdp_phys, hhdm_offset, mapper, frame_alloc)`:
1. Parse ACPI tables using the `acpi` crate with `IdendtityAcpiHandler`.
2. Find MADT for LAPIC/IOAPIC addresses.
3. Map IOAPIC MMIO.
4. Set up interrupt source overrides (ISOs) from MADT.

## SMP — Symmetric Multiprocessing

Currently configured for 2 cores in QEMU (`-smp cores=2`).
MAX_CORES = 16 (compile-time upper bound).

### AP Boot Flow

AP cores start in real mode. The boot sequence is:

1. `build.rs` assembles `src/asm/ap_trampoline.S` → links with `ap_trampoline.ld`
   → `objcopy -O binary` → `ap_trampoline.bin`.
2. Binary blob is embedded via `env!("AP_TRAMPOLINE_BIN")` in `asm/mod.rs`.
3. At boot, BSP writes the trampoline blob to physical address `0x8000`.
4. BSP identity-maps `0x8000` and `0x9000` (via `setup_ap_trampoline_mapping`).
5. `cpus[1].bootstrap(ap_core_from_limine_entry_point, 0x12345678)` — Limine sends
   an IPI SIPI to the AP, pointing it at the trampoline.

AP trampoline (`ap_trampoline.S`) transitions:
- Real mode → protected mode → long mode (64-bit)
- Sets up minimal GDT, CR0/CR4/EFER bits, loads CR3 from BSP
- Jumps to `ap_core_from_limine_entry_point`

### ap_core_from_limine_entry_point (cpuinfo.rs)

Called by each AP with a pointer to the Limine `MpInfo`:
1. Read core ID from `MpInfo.lapic_id` → map to sequential core index.
2. `init_core_gdt(core_id)` — AP gets its own GDT/TSS.
3. Map LAPIC for this core.
4. `init_lapic_for_current_core(core_id)`.
5. `init_syscall()` — write SYSCALL MSRs.
6. Enable interrupts.
7. `run_on_core_loop(core_id)` — enter scheduler, never return.

### Core ID

`get_current_core_id() -> u8` reads the APIC ID from the per-core LAPIC register
(IDr offset), then translates it to a 0-based sequential index using the CPU info
table built during `init_cpu_infos`.

`init_current_core()` stores the current core's info in a per-core structure accessed
by `get_current_core_id()`.

### CpuInfo (cpuinfo.rs)

```rust
pub struct CpuInfo {
    pub features: CpuFeatureFlags,
    pub cache_line_size: u8,
    pub apic_id: u8,
    pub family: u8, pub model: u8, pub stepping: u8,
    pub vendor: CpuVendor,  // Intel / Amd / Unknown
}
```

`CpuFeatureFlags`: APIC, X2APIC, TSC, TSC_DEADLINE, SSE, SSE2, SSE3, SSE4_1,
SSE4_2, AVX, AES, RDRAND, HYPERVISOR (among others).

`CPU_INFO_PER_CORE: Once<Vec<CpuInfo>>` — initialized by `init_cpu_infos(&cpus)`.

`get_cpu_info()` returns the BSP's info. `get_cpu_info_for_core(core_id)` returns
per-core info.

## Serial (io/serial.rs)

Uses `uart_16550` crate (Uart16550Tty with PioBackend).

| Port    | Address | Use                                          |
|---------|---------|----------------------------------------------|
| COM1    | 0x3F8   | Kernel debug log (serial_print!, serial_println!, serial_println_core!) |
| COM2    | 0x2F8   | Userspace stdout/stderr (sys_write fd=1/fd=2 routes here) |

```rust
lazy_static! {
    pub static ref SERIAL1: Mutex<Uart16550Tty<PioBackend>>
    pub static ref SERIAL2: Mutex<Uart16550Tty<PioBackend>>
}
```

`_print(args)` and `_print2(args)` disable interrupts while holding the lock to
prevent deadlock from serial writes inside interrupt handlers.

### Log Macros

```rust
serial_print!(...) / serial_println!(...)
    // Write to COM1; no prefix

serial_println_core!(...)
    // Write to COM1 with prefix: [Core N | Xus]
    // Uses get_current_core_id() and tsc_timestamp_us()

serial2_print!(...)
    // Write to COM2; used by sys_write syscall handler for userspace output
```

### MSR Helpers (util/msr.rs)

```rust
pub unsafe fn msr_read(reg: u32) -> u64   // RDMSR
pub unsafe fn msr_write(reg: u32, val: u64) // WRMSR
```

Used for: EFER (0xC0000080), STAR (0xC0000081), LSTAR (0xC0000082),
SFMASK (0xC0000084), APIC_BASE (0x1B), IA32_TSC_DEADLINE (0x6E0).

## ISA Debug Exit Device

Port 0xF4 (4-byte I/O). Write a u32:
- 0x10 → QEMU exits with code 33 (success)
- 0x11 → QEMU exits with code 35 (failure)

Configured in QEMU as: `-device isa-debug-exit,iobase=0xf4,iosize=0x04`

Used by the panic handler and `exit_qemu()` in main.rs.

## Known Issues / TODOs

- IDT loaded only on BSP; APs share the IDT pointer but do not call `load_idt()`
  explicitly — this works because the IDT is at a fixed virtual address accessible
  from all cores via the shared kernel page table.
- ACPI region mapping (`map_acpi_regions`) is commented out in kmain — ACPI tables
  are accessed through the HHDM offset instead.
- `IdendtityAcpiHandler` has most IO methods as `todo!()` — only memory mapping works.
- X2APIC is explicitly disabled (`MP_FLAG_NO_X2APIC = 0x0`) in the Limine MP request;
  XAPIC (MMIO) mode only.
- AP trampoline identity mapping (`setup_ap_trampoline_mapping`) is called but its
  usage path may not be connected in the current kmain (check if called before
  Limine's bootstrap).
