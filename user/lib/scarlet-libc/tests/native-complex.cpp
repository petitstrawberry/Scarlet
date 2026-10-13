// Native acceptance of libc++ complex<float/double> with Scarlet C headers.
// Binary128 ABI is checked without claiming a binary128 atan2l implementation.
#include <complex>
#include <float.h>
#include <math.h>
#include <stdio.h>
#include <type_traits>

static_assert(sizeof(long double) == 16 && LDBL_MANT_DIG == 113);
static_assert(std::is_same_v<decltype(HUGE_VAL), double>);
static_assert(std::is_same_v<decltype(HUGE_VALF), float>);
static_assert(std::is_same_v<decltype(HUGE_VALL), long double>);

static int failures;
#define CHECK(name, condition) do { \
    bool good = !!(condition); \
    printf("SCARLET_NATIVE_COMPLEX %s %s\n", name, good ? "PASS" : "FAIL"); \
    if (!good) ++failures; \
} while (0)

template<class T> void complex_cases(const char* norm, const char* axis, const char* scalar) {
    volatile T two = 2, three = 3, four = 4, minusFive = -5;
    const std::complex<T> value(two, three);
    const std::complex<T> sum = value + std::complex<T>(four, minusFive);
    CHECK(norm, std::norm(value) == 13 && sum.real() == 6 && sum.imag() == -2);
    volatile T zero = 0, one = 1, minusOne = -1;
    T angle = std::arg(std::complex<T>(zero, one));
    CHECK(axis, angle > T(1.5707) && angle < T(1.5709));
    angle = std::arg(T(minusOne));
    CHECK(scalar, angle > T(3.1415) && angle < T(3.1417));
}

int main() {
    complex_cases<float>("float_norm_sum", "float_arg_axis", "float_arg_scalar");
    complex_cases<double>("double_norm_sum", "double_arg_axis", "double_arg_scalar");
    CHECK("huge_constants", isinf(HUGE_VAL) && HUGE_VAL > 0
        && isinf(HUGE_VALF) && HUGE_VALF > 0
        && isinf(HUGE_VALL) && HUGE_VALL > 0);
    if (failures) {
        printf("SCARLET_NATIVE_COMPLEX RESULT failures=%d\n", failures);
        return 1;
    }
    puts("SCARLET_NATIVE_COMPLEX_PASS checks=7 long_double_bytes=16 mantissa=113");
    return 0;
}
