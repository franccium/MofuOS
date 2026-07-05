#include"../../libc/syscall.h"

/* ── minimal helpers (no libc) ───────────────────────────────────────────── */

static size_t strlen(const char *s) {
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

/* Write a NUL-terminated string to fd 1 (serial). */
static void print(const char *s) {
    sys_write(1, s, strlen(s));
}

/* Write a single decimal number followed by a newline. */
static void print_num(long n) {
    char buf[32];
    int  neg = 0;
    int  i   = 30;

    buf[31] = '\n';

    if (n < 0) { neg = 1; n = -n; }
    if (n == 0) { buf[i--] = '0'; }
    else {
        while (n > 0) {
            buf[i--] = '0' + (n % 10);
            n /= 10;
        }
    }
    if (neg) buf[i--] = '-';
    i++;  /* point at first valid character */
    sys_write(1, buf + i, 31 - i + 1 + 1); /* +1 for '\n', +1 for off-by-one */
}

/* ── test framework ───────────────────────────────────────────────────────── */

static int tests_run    = 0;
static int tests_passed = 0;
static int tests_failed = 0;

static void test_pass(const char *name) {
    tests_run++;
    tests_passed++;
    print("[PASS]");
    print(name);
    print("\n");
}

static void test_fail(const char *name, long expected, long got) {
    tests_run++;
    tests_failed++;
    print("[FAIL]");
    print(name);
    print(" -- expected");
    print_num(expected);
    print("       got");
    print_num(got);
}

#define EXPECT_EQ(name, expected, expr) \
    do { \
        long _got = (long)(expr); \
        if (_got == (long)(expected)) test_pass(name); \
        else test_fail(name, (long)(expected), _got); \
    } while (0)

#define EXPECT_NE(name, unexpected, expr) \
    do { \
        long _got = (long)(expr); \
        if (_got != (long)(unexpected)) test_pass(name); \
        else { \
            tests_run++; tests_failed++; \
            print("[FAIL]"); print(name); \
            print(" -- got unexpected value"); print_num(_got); \
        } \
    } while (0)

/* ── individual test suites ──────────────────────────────────────────────── */

/*
 * Suite 1 — sys_write (syscall 2)
 *
 * We can't capture serial output from userspace, so we verify that the
 * syscall returns the byte count we passed and doesn't crash.
 */
static void suite_write(void) {
    print("\n[CTests] sys_write\n");

    /* Writing an empty buffer should return 0 bytes. */
    EXPECT_EQ("write 0 bytes returns 0",
              0,
              sys_write(1,"", 0));

    /* Writing a known string should return exactly its length. */
    const char *msg ="hello from MofuOS\n";
    size_t      len = strlen(msg);
    EXPECT_EQ("write N bytes returns N",
              (long)len,
              sys_write(1, msg, len));

    /* A second write should also return its exact length. */
    const char *msg2 ="second write\n";
    EXPECT_EQ("second write returns correct length",
              (long)strlen(msg2),
              sys_write(1, msg2, strlen(msg2)));
}

/*
 * Suite 2 — syscall 997  (echo: returns arg1)
 *
 * lets us verify that argument passing through the syscall ABI works 
 * correctly for several values.
 */
static void suite_echo_math(void) {
    print("\n[CTests] syscall 997 (arg)\n");

    EXPECT_EQ("0 == 0", 0,  syscall6(997, 0,  0,0,0,0,0));
    EXPECT_EQ("1 == 1", 1,  syscall6(997, 1,  0,0,0,0,0));
    EXPECT_EQ("-1 == -1", -1, syscall6(997, -1, 0,0,0,0,0));
    EXPECT_EQ("u64_max == u64_max", 0xFFFFFFFFFFFFFFFF, syscall6(997, 0xFFFFFFFFFFFFFFFF, 0,0,0,0,0));
    EXPECT_EQ("124 == 124", 124,  syscall6(997, 124, 0,0,0,0,0));
}

/*
 * Suite 3 — unimplemented syscalls
 *
 * Calling a syscall that isn't handled yet should return u64::MAX
 * (0xFFFFFFFFFFFFFFFF == -1 as a signed long). This verifies the kernel
 * doesn't crash on unknown syscall numbers and returns a sentinel value.
 */
static void suite_unimplemented(void) {
    print("\n[CTests] unimplemented syscalls return -1\n");

    /* Pick a handful of numbers that have no handler yet. */
    EXPECT_EQ("syscall 3  (read)    -> -1",   -1L, syscall6(3,  0,0,0,0,0,0));
    EXPECT_EQ("syscall 5  (alloc)   -> -1",   -1L, syscall6(5,  0,0,0,0,0,0));
    EXPECT_EQ("syscall 42 (unknown) -> -1",   -1L, syscall6(42, 0,0,0,0,0,0));
    EXPECT_EQ("syscall 998(unknown) -> -1",   -1L, syscall6(998,0,0,0,0,0,0));
}

/*
 * Suite 4 — string operations (pure userspace, no syscalls)
 *
 * These exercise our tiny inline helpers so we know that the C runtime
 * environment (BSS zeroing, stack, basic arithmetic) is working before we
 * trust any syscall-based result.
 */
static void suite_userspace_logic(void) {
    print("\n[CTests] userspace logic (no syscalls)\n");

    /* strlen */
    EXPECT_EQ("strlen empty",   0, (long)strlen(""));
    EXPECT_EQ("strlen hello",   5, (long)strlen("hello"));
    EXPECT_EQ("strlen 1 char",  1, (long)strlen("x"));

    /* BSS is zero-initialised */
    static int bss_var;   /* should be 0 */
    EXPECT_EQ("BSS variable is 0", 0, (long)bss_var);

    /* Basic arithmetic */
    volatile int a = 7, b = 3;
    EXPECT_EQ("7 + 3 == 10",  10, (long)(a + b));
    EXPECT_EQ("7 * 3 == 21",  21, (long)(a * b));
    EXPECT_EQ("7 - 3 == 4",    4, (long)(a - b));
    EXPECT_EQ("7 / 3 == 2",    2, (long)(a / b));
    EXPECT_EQ("7 % 3 == 1",    1, (long)(a % b));

    /* Pointer arithmetic */
    char arr[4] = {10, 20, 30, 40};
    EXPECT_EQ("arr[0] == 10", 10, (long)arr[0]);
    EXPECT_EQ("arr[3] == 40", 40, (long)arr[3]);
    EXPECT_EQ("ptr[2] == 30", 30, (long)*(arr + 2));

    /* Loops */
    int sum = 0;
    for (int i = 1; i <= 10; i++) sum += i;
    EXPECT_EQ("sum 1..10 == 55", 55, (long)sum);

    /* Stack-allocated struct */
    struct { int x; int y; } pt = {3, 7};
    EXPECT_EQ("struct field x", 3, (long)pt.x);
    EXPECT_EQ("struct field y", 7, (long)pt.y);
    pt.x += pt.y;
    EXPECT_EQ("struct mutation", 10, (long)pt.x);
}

/*
 * Suite 5 — multiple writes (stress test of the write path)
 *
 * Write many small strings in a row. Verifies the kernel serial path
 * doesn't corrupt state across repeated syscalls.
 */
static void suite_write_stress(void) {
    print("\n[CTests] write stress (20 sequential writes)\n");

    int ok = 1;
    for (int i = 0; i < 20; i++) {
        char ch[3] = {'A' + (i % 26), ' ', 0};
        long r = sys_write(1, ch, 2);
        if (r != 2) { ok = 0; break; }
    }
    sys_write(1,"\n", 1);

    EXPECT_EQ("all 20 writes returned 2", 1, (long)ok);
}

/* ── entry point ─────────────────────────────────────────────────────────── */

int main(int argc, char **argv) {
    (void)argc; (void)argv;

    print("============================================================\n");
    print("MofuOS userspace test suite\n");
    print("============================================================\n");

    suite_userspace_logic();
    suite_write();
    suite_echo_math();
    suite_unimplemented();
    suite_write_stress();

    print("\n============================================================\n");
    print("Results:");
    print_num(tests_passed);
    print("passed /");
    print_num(tests_run);
    print("total\n");

    if (tests_failed == 0) {
        print("ALL TESTS PASSED\n");
        sys_exit(0);
    } else {
        print("FAILURES:");
        print_num(tests_failed);
        print("\n");
        sys_exit(1);
    }

    return 0;
}
