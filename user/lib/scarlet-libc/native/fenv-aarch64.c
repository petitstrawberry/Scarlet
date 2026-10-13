#include <fenv.h>
#if !defined(__aarch64__)
#error "This native fenv adapter requires AArch64"
#endif
int fegetround(void) {
    unsigned long fpcr;
    __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
    return (int)(fpcr & 0xc00000UL);
}
int fesetround(int rounding) {
    if (rounding != FE_TONEAREST && rounding != FE_UPWARD &&
        rounding != FE_DOWNWARD && rounding != FE_TOWARDZERO)
        return -1;
    unsigned long fpcr;
    __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
    fpcr = (fpcr & ~0xc00000UL) | (unsigned long)rounding;
    __asm__ volatile("msr fpcr, %0\n\tisb" : : "r"(fpcr) : "memory");
    return 0;
}
