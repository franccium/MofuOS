#include "../../libc/syscall.h"

int main(int argc, char **argv) {
    const char msg[] = "ping\n";

    while (1) {
        //sys_write(1, msg, sizeof(msg) - 1);
    }

    return 0;
}
