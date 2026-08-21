assuming we have some particular instruction address to look at

cd /home/bawercx/repos/MofuOS && objdump -d --no-show-raw-insn kernel/kernel
  2>/dev/null | awk '/^ffffffff80040ee0/,0' | grep -B 20 "jmp.*jump_to_userspace"
ffffffff80041cee: mov    0x3c8(%rsp),%rax
ffffffff80041cf6: mov    %rax,0x398(%rsp)
ffffffff80041cfe: mov    0x3d0(%rsp),%rax
ffffffff80041d06: mov    %rax,0x3a0(%rsp)
ffffffff80041d0e: mov    0x3d8(%rsp),%rax
...