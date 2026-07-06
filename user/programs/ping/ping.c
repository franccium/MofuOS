#include "../../libc/syscall.h"

int snprintf(char *str, size_t size, const char *format, ...) {
    int pos = 0;
    va_list args;
    va_start(args, format);
    
    while (*format && pos < (int)size - 1) {
        if (*format != '%') {
            str[pos++] = *format++;
            continue;
        }
        
        format++;  // Skip '%'
        
        if (*format == 'u') {
            unsigned int num = va_arg(args, unsigned int);
            char num_buf[32];
            int i = 0;
            
            // Convert to string
            if (num == 0) {
                num_buf[i++] = '0';
            } else {
                char temp[32];
                int j = 0;
                while (num > 0) {
                    temp[j++] = '0' + (num % 10);
                    num /= 10;
                }
                while (j > 0) {
                    num_buf[i++] = temp[--j];
                }
            }
            num_buf[i] = '\0';
            
            // Copy number to output
            for (int j = 0; j < i && pos < (int)size - 1; j++) {
                str[pos++] = num_buf[j];
            }
            format++;
        } else if (*format == 's') {
            char *s = va_arg(args, char*);
            while (*s && pos < (int)size - 1) {
                str[pos++] = *s++;
            }
            format++;
        } else {
            str[pos++] = '%';
            if (pos < (int)size - 1) {
                str[pos++] = *format++;
            }
        }
    }
    
    va_end(args);
    str[pos] = '\0';
    return pos;
}

int main(int argc, char **argv) {
    const char msg[] = "ping\n";
    char buffer[256];
    unsigned int iter = 0;

    while (1) {
        int len = snprintf(buffer, sizeof(buffer), "msg %u\n", iter);
        sys_write(1, buffer, len);
        iter++;
    }

    return 0;
}
