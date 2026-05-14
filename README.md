# MofuOS

## Running

### Prerequisites

- QEMU

- llvm tools

- rustc 1.96.0-nightly

### Instructions

for Debian systems:

`sudo apt install -y qemu-system-x86 llvm-14-tools ovmf`

```
mkdir ovmf
cp /usr/share/OVMF/OVMF_VARS_4M.fd ovmf/ovmf-vars-x86_64.fd
cp /usr/share/OVMF/OVMF_CODE_4M.fd ovmf/ovmf-code-x86_64.fd 
```

Compile the kernel and generate an ISO image: `make all`


Build the kernel and the ISO image and run using `qemu`: `make run`

I recommend setting up `qemu-kvm` for hardware acceleration

