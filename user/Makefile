CC = clang
LD = ld.lld
AR = llvm-ar

TARGET = x86_64-unknown-elf

CFLAGS = --target=$(TARGET) \
         -ffreestanding \
         -nostdlib \
         -nostdinc \
         -fno-builtin \
         -fno-stack-protector \
         -mno-red-zone \
         -m64 \
         -Wall \
         -Wextra \
         -I. \
         -g

LDFLAGS = -T linker.ld \
          -nostdlib \
          -static

# Build libc
LIBC_OBJS = libc/syscall.o

crt0.o: crt0.c
	$(CC) $(CFLAGS) -c $< -o $@

libc/%.o: libc/%.c
	$(CC) $(CFLAGS) -c $< -o $@

libc.a: $(LIBC_OBJS)
	$(AR) rcs $@ $^

# Build user programs
PROGRAMS = programs/first/first

all: $(PROGRAMS)

%.o: %.c
	$(CC) $(CFLAGS) -c $< -o $@

programs/first/first: programs/first/first.o crt0.o libc.a
	$(LD) $(LDFLAGS) -o $@ crt0.o programs/first/first.o libc.a

%.o: %.c
	$(CC) $(CFLAGS) -c $< -o $@

clean:
	rm -f crt0.o libc/*.o libc.a programs/*/*.o $(PROGRAMS)

.PHONY: all clean