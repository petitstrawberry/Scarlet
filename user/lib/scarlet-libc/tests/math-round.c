#include <fenv.h>
#include <limits.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>

#if !defined(__aarch64__)
#error "The rounding probe reads the native AArch64 floating environment"
#endif
_Static_assert(sizeof(long) == 8 && sizeof(long long) == 8, "LP64 required");

static int failures;
static unsigned checks;
static unsigned calls;

/* Native FPSR cumulative exception flags, deliberately not a public fenv
 * replacement. IOC=invalid, IXC=inexact, IDC=input denormal. */
static unsigned long read_fpsr(void)
{
    unsigned long value;
    __asm__ volatile("mrs %0, fpsr" : "=r"(value) : : "memory");
    return value;
}
static void write_fpsr(unsigned long value)
{
    __asm__ volatile("msr fpsr, %0" : : "r"(value) : "memory");
}
static unsigned long read_fpcr(void)
{
    unsigned long value;
    __asm__ volatile("mrs %0, fpcr" : "=r"(value) : : "memory");
    return value;
}
static void report(const char *function, const char *name, int ok)
{
    ++checks;
    if (!ok) ++failures;
    printf("WEBCORE_MATH_ROUND %s_%s %s\n", function, name, ok ? "PASS" : "FAIL");
}
static double double_bits(uint64_t bits)
{
    double value;
    __builtin_memcpy(&value, &bits, sizeof(value));
    return value;
}
static float float_bits(uint32_t bits)
{
    float value;
    __builtin_memcpy(&value, &bits, sizeof(value));
    return value;
}

struct double_case { const char *name; double value; long long expected; int invalid; };
struct float_case { const char *name; float value; long long expected; int invalid; };

static const int modes[] = { FE_TONEAREST, FE_UPWARD, FE_DOWNWARD, FE_TOWARDZERO };
/* Clean, each exception flag, all flags, and input-denormal flag. IXC must
 * neither appear from a clean state nor disappear from a seeded state. */
static const unsigned long seeds[] = { 0, 1, 2, 4, 8, 16, 31, 128 };

static int check_result(const char *function, const char *name, int mode,
    unsigned long seed, unsigned long fpcr, long long result,
    long long expected, int invalid)
{
    const unsigned long flags = read_fpsr();
    const unsigned long control = read_fpcr();
    ++calls;
    if (result == expected && flags == (seed | (invalid ? 1UL : 0UL))
        && control == fpcr)
        return 1;
    printf("WEBCORE_MATH_ROUND DETAIL %s_%s mode=%x seed=%lx "
        "got=%lld expected=%lld flags=%lx control=%lx expected_control=%lx\n",
        function, name, mode, seed, result, expected, flags, control, fpcr);
    return 0;
}

static void test_double(const struct double_case *item)
{
    int long_ok = 1, long_long_ok = 1;
    for (unsigned m = 0; m < sizeof(modes) / sizeof(modes[0]); ++m) {
        if (fesetround(modes[m])) { long_ok = long_long_ok = 0; continue; }
        const unsigned long fpcr = read_fpcr();
        for (unsigned s = 0; s < sizeof(seeds) / sizeof(seeds[0]); ++s) {
            write_fpsr(seeds[s]);
            const long result = lround(item->value);
            long_ok &= check_result("lround", item->name, modes[m], seeds[s],
                fpcr, result, item->expected, item->invalid);
            write_fpsr(seeds[s]);
            const long long ll_result = llround(item->value);
            long_long_ok &= check_result("llround", item->name, modes[m], seeds[s],
                fpcr, ll_result, item->expected, item->invalid);
        }
    }
    report("lround", item->name, long_ok);
    report("llround", item->name, long_long_ok);
}
static void test_float(const struct float_case *item)
{
    int long_ok = 1, long_long_ok = 1;
    for (unsigned m = 0; m < sizeof(modes) / sizeof(modes[0]); ++m) {
        if (fesetround(modes[m])) { long_ok = long_long_ok = 0; continue; }
        const unsigned long fpcr = read_fpcr();
        for (unsigned s = 0; s < sizeof(seeds) / sizeof(seeds[0]); ++s) {
            write_fpsr(seeds[s]);
            const long result = lroundf(item->value);
            long_ok &= check_result("lroundf", item->name, modes[m], seeds[s],
                fpcr, result, item->expected, item->invalid);
            write_fpsr(seeds[s]);
            const long long ll_result = llroundf(item->value);
            long_long_ok &= check_result("llroundf", item->name, modes[m], seeds[s],
                fpcr, ll_result, item->expected, item->invalid);
        }
    }
    report("lroundf", item->name, long_ok);
    report("llroundf", item->name, long_long_ok);
}

int main(void)
{
    const unsigned long initial_flags = read_fpsr();
    const int initial_mode = fegetround();
    const struct double_case doubles[] = {
        { "positive_zero", 0.0, 0, 0 },
        { "negative_zero", -0.0, 0, 0 },
        { "positive_subnormal", 0x1p-1074, 0, 0 },
        { "negative_subnormal", -0x1p-1074, 0, 0 },
        { "positive_minimum_normal", 0x1p-1022, 0, 0 },
        { "negative_minimum_normal", -0x1p-1022, 0, 0 },
        { "positive_below_half", 0x1.fffffffffffffp-2, 0, 0 },
        { "negative_below_half", -0x1.fffffffffffffp-2, 0, 0 },
        { "positive_half", 0.5, 1, 0 },
        { "negative_half", -0.5, -1, 0 },
        { "positive_above_half", 0x1.0000000000001p-1, 1, 0 },
        { "negative_above_half", -0x1.0000000000001p-1, -1, 0 },
        { "positive_below_one_half", 0x1.7ffffffffffffp+0, 1, 0 },
        { "negative_below_one_half", -0x1.7ffffffffffffp+0, -1, 0 },
        { "positive_one_half", 1.5, 2, 0 },
        { "negative_one_half", -1.5, -2, 0 },
        { "positive_above_one_half", 0x1.8000000000001p+0, 2, 0 },
        { "negative_above_one_half", -0x1.8000000000001p+0, -2, 0 },
        { "positive_below_two_half", 0x1.3ffffffffffffp+1, 2, 0 },
        { "negative_below_two_half", -0x1.3ffffffffffffp+1, -2, 0 },
        { "positive_two_half", 2.5, 3, 0 },
        { "negative_two_half", -2.5, -3, 0 },
        { "positive_above_two_half", 0x1.4000000000001p+1, 3, 0 },
        { "negative_above_two_half", -0x1.4000000000001p+1, -3, 0 },
        { "positive_exact_integer", 42.0, 42, 0 },
        { "negative_exact_integer", -42.0, -42, 0 },
        { "positive_fraction_precision_edge", 0x1.fffffffffffffp+51, 4503599627370496LL, 0 },
        { "negative_fraction_precision_edge", -0x1.fffffffffffffp+51, -4503599627370496LL, 0 },
        { "positive_integer_precision_edge", 0x1.0000000000001p+52, 4503599627370497LL, 0 },
        { "negative_integer_precision_edge", -0x1.0000000000001p+52, -4503599627370497LL, 0 },
        { "largest_positive_valid", 0x1.fffffffffffffp+62, LONG_MAX - 1023, 0 },
        { "negative_before_minimum", -0x1.fffffffffffffp+62, -(LONG_MAX - 1023), 0 },
        { "minimum_valid", -0x1p+63, LONG_MIN, 0 },
        { "positive_limit_invalid", 0x1p+63, LONG_MIN, 1 },
        { "negative_beyond_minimum", -0x1.0000000000001p+63, LONG_MIN, 1 },
        { "positive_beyond_limit", 0x1.0000000000001p+63, LONG_MIN, 1 },
        { "positive_huge", 0x1.fffffffffffffp+1023, LONG_MIN, 1 },
        { "negative_huge", -0x1.fffffffffffffp+1023, LONG_MIN, 1 },
        { "positive_infinity", double_bits(UINT64_C(0x7ff0000000000000)), LONG_MIN, 1 },
        { "negative_infinity", double_bits(UINT64_C(0xfff0000000000000)), LONG_MIN, 1 },
        { "positive_quiet_nan", double_bits(UINT64_C(0x7ff8000000001234)), LONG_MIN, 1 },
        { "negative_quiet_nan", double_bits(UINT64_C(0xfff8000000001234)), LONG_MIN, 1 },
        { "positive_signaling_nan", double_bits(UINT64_C(0x7ff0000000001234)), LONG_MIN, 1 },
        { "negative_signaling_nan", double_bits(UINT64_C(0xfff0000000001234)), LONG_MIN, 1 },
    };
    const struct float_case floats[] = {
        { "positive_zero", 0.0f, 0, 0 },
        { "negative_zero", -0.0f, 0, 0 },
        { "positive_subnormal", 0x1p-149f, 0, 0 },
        { "negative_subnormal", -0x1p-149f, 0, 0 },
        { "positive_minimum_normal", 0x1p-126f, 0, 0 },
        { "negative_minimum_normal", -0x1p-126f, 0, 0 },
        { "positive_below_half", 0x1.fffffep-2f, 0, 0 },
        { "negative_below_half", -0x1.fffffep-2f, 0, 0 },
        { "positive_half", 0.5f, 1, 0 },
        { "negative_half", -0.5f, -1, 0 },
        { "positive_above_half", 0x1.000002p-1f, 1, 0 },
        { "negative_above_half", -0x1.000002p-1f, -1, 0 },
        { "positive_below_one_half", 0x1.7ffffep+0f, 1, 0 },
        { "negative_below_one_half", -0x1.7ffffep+0f, -1, 0 },
        { "positive_one_half", 1.5f, 2, 0 },
        { "negative_one_half", -1.5f, -2, 0 },
        { "positive_above_one_half", 0x1.800002p+0f, 2, 0 },
        { "negative_above_one_half", -0x1.800002p+0f, -2, 0 },
        { "positive_below_two_half", 0x1.3ffffep+1f, 2, 0 },
        { "negative_below_two_half", -0x1.3ffffep+1f, -2, 0 },
        { "positive_two_half", 2.5f, 3, 0 },
        { "negative_two_half", -2.5f, -3, 0 },
        { "positive_above_two_half", 0x1.400002p+1f, 3, 0 },
        { "negative_above_two_half", -0x1.400002p+1f, -3, 0 },
        { "positive_exact_integer", 42.0f, 42, 0 },
        { "negative_exact_integer", -42.0f, -42, 0 },
        { "positive_fraction_precision_edge", 0x1.fffffep+22f, 8388608, 0 },
        { "negative_fraction_precision_edge", -0x1.fffffep+22f, -8388608, 0 },
        { "positive_integer_precision_edge", 0x1.000002p+23f, 8388609, 0 },
        { "negative_integer_precision_edge", -0x1.000002p+23f, -8388609, 0 },
        { "largest_positive_valid", 0x1.fffffep+62f, LONG_MAX - ((1LL << 39) - 1), 0 },
        { "negative_before_minimum", -0x1.fffffep+62f, -(LONG_MAX - ((1LL << 39) - 1)), 0 },
        { "minimum_valid", -0x1p+63f, LONG_MIN, 0 },
        { "positive_limit_invalid", 0x1p+63f, LONG_MIN, 1 },
        { "negative_beyond_minimum", -0x1.000002p+63f, LONG_MIN, 1 },
        { "positive_beyond_limit", 0x1.000002p+63f, LONG_MIN, 1 },
        { "positive_huge", 0x1.fffffep+127f, LONG_MIN, 1 },
        { "negative_huge", -0x1.fffffep+127f, LONG_MIN, 1 },
        { "positive_infinity", float_bits(UINT32_C(0x7f800000)), LONG_MIN, 1 },
        { "negative_infinity", float_bits(UINT32_C(0xff800000)), LONG_MIN, 1 },
        { "positive_quiet_nan", float_bits(UINT32_C(0x7fc01234)), LONG_MIN, 1 },
        { "negative_quiet_nan", float_bits(UINT32_C(0xffc01234)), LONG_MIN, 1 },
        { "positive_signaling_nan", float_bits(UINT32_C(0x7f801234)), LONG_MIN, 1 },
        { "negative_signaling_nan", float_bits(UINT32_C(0xff801234)), LONG_MIN, 1 },
    };
    for (unsigned i = 0; i < sizeof(doubles) / sizeof(doubles[0]); ++i)
        test_double(&doubles[i]);
    for (unsigned i = 0; i < sizeof(floats) / sizeof(floats[0]); ++i)
        test_float(&floats[i]);
    (void)fesetround(initial_mode);
    write_fpsr(initial_flags);
    printf("WEBCORE_MATH_ROUND RESULT checks=%u calls=%u failures=%d\n",
        checks, calls, failures);
    return failures ? 1 : 0;
}
