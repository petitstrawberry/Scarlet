#include <float.h>
#include <limits.h>
#include <math.h>
#include <stdint.h>

#if !defined(__aarch64__)
#error "The native Scarlet rounding runtime requires AArch64"
#endif
_Static_assert(CHAR_BIT == 8 && sizeof(long) == 8 && sizeof(long long) == 8,
    "The native Scarlet rounding runtime requires LP64");
_Static_assert(FLT_RADIX == 2 && FLT_MANT_DIG == 24 && FLT_MAX_EXP == 128
    && sizeof(float) == 4 && DBL_MANT_DIG == 53 && DBL_MAX_EXP == 1024
    && sizeof(double) == 8, "IEEE binary32 and binary64 are required");

/* The failure result is LONG_MIN (and hence LLONG_MIN on this target).
 * An actual 0/0 raises only invalid, preserving all incoming FPSR flags;
 * unlike writing FPSR directly it also honors an enabled invalid trap.
 * Inline assembly prevents folding away the deliberate exception. */
static long invalid_result(void)
{
    double ignored;
    const double zero = 0.0;
    __asm__ volatile("fdiv %d0, %d1, %d1"
        : "=w"(ignored) : "w"(zero) : "memory");
    return LONG_MIN;
}

/* No floating arithmetic or floating-to-integer conversion is used on a
 * successful path. Bit inspection also leaves subnormal and sNaN operands
 * untouched until the explicit invalid operation, independently of FPCR. */
static long round_bits(uint64_t bits, unsigned fraction_width,
    unsigned exponent_width, int exponent_bias)
{
    const uint64_t fraction_mask = (UINT64_C(1) << fraction_width) - 1;
    const unsigned exponent_mask = (1U << exponent_width) - 1;
    const unsigned negative = (unsigned)(bits >> (fraction_width + exponent_width));
    const unsigned raw_exponent = (unsigned)(bits >> fraction_width) & exponent_mask;
    const uint64_t fraction = bits & fraction_mask;
    if (raw_exponent == exponent_mask)
        return invalid_result();

    const int exponent = (int)raw_exponent - exponent_bias;
    if (exponent < -1)
        return 0;
    if (exponent == -1)
        return negative ? -1 : 1;
    if (exponent >= 63) {
        /* -2^63 is representable; every other value with this exponent or
         * above is outside the signed return range. No overflowing shift. */
        if (exponent == 63 && negative && fraction == 0)
            return LONG_MIN;
        return invalid_result();
    }

    const uint64_t significand = fraction | (UINT64_C(1) << fraction_width);
    uint64_t magnitude;
    if (exponent >= (int)fraction_width)
        magnitude = significand << ((unsigned)exponent - fraction_width);
    else {
        const unsigned shift = fraction_width - (unsigned)exponent;
        /* Adding the most significant discarded bit rounds halfway cases
         * away from zero, without depending on the rounding direction. */
        magnitude = (significand >> shift) + ((significand >> (shift - 1)) & 1);
    }
    /* exponent <= 62 guarantees magnitude <= LONG_MAX for IEEE binary32
     * and binary64, whose precision is strictly below 64. */
    return negative ? -(long)magnitude : (long)magnitude;
}

long lround(double value)
{
    uint64_t bits;
    __builtin_memcpy(&bits, &value, sizeof(bits));
    return round_bits(bits, 52, 11, 1023);
}

long lroundf(float value)
{
    uint32_t bits;
    __builtin_memcpy(&bits, &value, sizeof(bits));
    return round_bits(bits, 23, 8, 127);
}

long long llround(double value)
{
    return lround(value);
}

long long llroundf(float value)
{
    return lroundf(value);
}
