#ifndef TYPES_H
#define TYPES_H

typedef unsigned long size_t;
typedef unsigned char uint8_t;
typedef unsigned short uint16_t;
typedef unsigned int uint32_t;
typedef unsigned long long uint64_t;
typedef unsigned long uintptr_t;
typedef long intptr_t;
typedef long ssize_t;

#define NULL ((void*)0)

typedef __builtin_va_list va_list;
#define va_start(v,l) __builtin_va_start(v,l)
#define va_arg(v,l) __builtin_va_arg(v,l)
#define va_end(v) __builtin_va_end(v)

#endif