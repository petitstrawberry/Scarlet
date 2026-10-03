#ifndef SCARLET_SETJMP_H
#define SCARLET_SETJMP_H
#include <bits/llvm-libc/jmp_buf.h>
#ifdef __cplusplus
extern "C" {
#endif
int setjmp(jmp_buf) __attribute__((returns_twice));
#ifdef __cplusplus
[[noreturn]] void longjmp(jmp_buf, int);
#else
_Noreturn void longjmp(jmp_buf, int);
#endif
#ifdef __cplusplus
}
#endif
/* No signal mask variants: Scarlet native signals are outside this prototype. */
#endif
