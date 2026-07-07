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

LIBC_OBJS = libc/syscall.o

PROGRAMS = programs/first/first programs/test/test programs/ping/ping programs/ping_yield/ping_yield

.PHONY: all rust-programs clean

all: $(PROGRAMS) rust-programs

rust-programs:
	cd rustspace && cargo +nightly build -Z build-std=core -Z json-target-spec --target x86_64-user.json

crt0.o: crt0.c
	$(CC) $(CFLAGS) -c $< -o $@

libc/%.o: libc/%.c
	$(CC) $(CFLAGS) -c $< -o $@

libc.a: $(LIBC_OBJS)
	$(AR) rcs $@ $^

%.o: %.c
	$(CC) $(CFLAGS) -c $< -o $@

programs/first/first: programs/first/first.o crt0.o libc.a
	$(LD) $(LDFLAGS) -o $@ crt0.o programs/first/first.o libc.a

programs/test/test: programs/test/test.o crt0.o libc.a
	$(LD) $(LDFLAGS) -o $@ crt0.o programs/test/test.o libc.a

programs/ping/ping: programs/ping/ping.o crt0.o libc.a
	$(LD) $(LDFLAGS) -o $@ crt0.o programs/ping/ping.o libc.a

programs/ping_yield/ping_yield: programs/ping_yield/ping_yield.o crt0.o libc.a
	$(LD) $(LDFLAGS) -o $@ crt0.o programs/ping_yield/ping_yield.o libc.a

clean:
	rm -f crt0.o libc/*.o libc.a programs/*/*.o $(PROGRAMS)
	cd rustspace && cargo clean
