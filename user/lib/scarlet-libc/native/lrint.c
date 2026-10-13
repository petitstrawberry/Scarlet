#include <limits.h>
#include <math.h>

#if !defined(__aarch64__)
#error "The native Scarlet lrint runtime requires AArch64"
#endif
_Static_assert(CHAR_BIT == 8 && sizeof(long) == 8,
    "The native Scarlet lrint runtime requires LP64");

/* AArch64 FRINTX obeys FPCR rounding and records inexact; FCVTZS then
 * converts the already integral value, including architectural invalid flags.
 * This is the instruction sequence used by the recovered JSC runtime. */
long lrint(double value)
{
    long result;
    __asm__ volatile("frintx %d1, %d1\n\tfcvtzs %0, %d1"
        : "=r"(result), "+w"(value));
    return result;
}
