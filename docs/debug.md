# Debug methodology

## Principles

- Get runtime evidence before theorizing. Static reading can find real bugs that are not the bug you are looking for (the r15 syscall clobber was real but did not fix the odys symptom)
- Bisect the data path: find the last point where the data is correct and the first point where it is wrong. Log the same value at each layer (disk -> kernel driver -> syscall copy-out -> userspace buffer -> heap object -> render)
- Read exact bytes, not rendered text. Use `xxd` on log lines and `{:?}` / `.as_bytes()` in debug prints. A single wrong byte (e.g. 0x20 or 0x38 where `/` should be) points straight at memory corruption
- Log pointers next to values (`{:p}`). Corrupted value + heap address + allocator size class is usually enough to find an allocator or overflow bug
- After a fix, rerun the same repro and check the same evidence. Do not assume a plausible fix worked
- Remove temporary debug prints before finishing. `cargo xtask build` runs `cargo fmt`, which splits long print lines across several lines, so remove them by matching the whole statement

## Checklist for "corrupted strings/data in userspace"

1. Verify the source data (disk image with mtools/xxd, kernel-side log of the value)
2. Verify the kernel copy-out (log what the syscall wrote and the user address)
3. Verify the user side right after the syscall (print raw buffer bytes)
4. If the data is fine after the syscall but wrong later, it is heap/stack corruption in the process: check the userspace allocator (`Arena` in `user/rustspace/src/lib.rs`), free-list node sizes vs block sizes, and overlapping mappings
5. Check the syscall ABI: registers the user `asm!` treats as preserved must be preserved by `syscall_handler`
6. Check the build is fresh: compare binary mtimes, ELF program headers in the boot log vs `readelf -lW`

## Running and driving QEMU headless

Build with `cargo xtask iso`, then run QEMU with serial to files and the monitor on telnet. Inject keys with `sendkey` and grab the screen with `screendump`. Use `snapshot=on` so the disk image is not modified.

```bash
cd /home/bawercx/repos/MofuOS
cp target/ovmf/ovmf-vars-x86_64.fd "$OUT/vars.fd"
timeout 75 qemu-system-x86_64 -M q35 -accel kvm -smp cores=4,threads=1 -cpu host,+tsc-deadline,+apic \
 -drive if=pflash,unit=0,format=raw,file=target/ovmf/ovmf-code-x86_64.fd,readonly=on \
 -drive if=pflash,unit=1,format=raw,file="$OUT/vars.fd" \
 -cdrom target/template-x86_64.iso -device piix3-ide,id=ide -device ide-hd,drive=ata0,bus=ide.0,unit=0 \
 -drive file=storage/ata_disk.img,format=raw,id=ata0,if=none,snapshot=on \
 -device isa-debug-exit,iobase=0xf4,iosize=0x04 -no-reboot -display none \
 -serial file:"$OUT/com1.log" -serial file:"$OUT/com2.log" \
 -monitor telnet:127.0.0.1:1234,server,nowait -m 2G &
sleep 12
python3 -c "
import socket, time
s = socket.create_connection(('127.0.0.1', 1234)); time.sleep(1)
for k in ['o', 'd', 'y', 's', 'ret']:
    s.send(('sendkey ' + k + '\n').encode()); time.sleep(0.3)
time.sleep(5)
for k in ['down', 'right']:
    s.send(('sendkey ' + k + '\n').encode()); time.sleep(2)
s.send(b'screendump $OUT/shot.ppm\n'); time.sleep(2)
"
```

- COM1 = kernel log, COM2 = userspace output (`[pid=N]` prefixed)
- Key delivery is timing dependent. If keys are lost, rerun or increase the sleeps
- The screendump is a PPM. Convert it to PNG with a small zlib script if PIL/ImageMagick are missing, crop to the window region, then view the image to check rendering

## Useful commands

- `readelf -lW user/rustspace/target/x86_64-user/debug/<prog>`: segment layout, compare with the `Mapping ELF segment` lines in the boot log
- `objdump -d --no-show-raw-insn -M intel target/x86_64-unknown-none/debug/kernel | awk '/<syscall_handler>:/{f=1} f{print} f&&/sysret/{exit}'`: check the generated syscall stub
- `grep -a` on logs (they contain escape codes), `xxd` on single lines to see exact bytes
