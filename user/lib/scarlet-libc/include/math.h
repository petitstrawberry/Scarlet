#ifndef SCARLET_MATH_H
#define SCARLET_MATH_H
#define INFINITY (__builtin_inff())
#define NAN (__builtin_nanf(""))
#define M_PI 3.14159265358979323846
#define M_PI_2 1.57079632679489661923
#define M_PI_4 0.785398163397448309616
#define M_E 2.71828182845904523536
#define isnan(value) __builtin_isnan(value)
#define FP_NAN 0
#define FP_INFINITE 1
#define FP_ZERO 2
#define FP_SUBNORMAL 3
#define FP_NORMAL 4
#ifdef __cplusplus
extern "C" {
#endif

#define isinf(x) __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define signbit(x) __builtin_signbit(x)
#define isnormal(x) __builtin_isnormal(x)
#define fpclassify(x) __builtin_fpclassify(FP_NAN,FP_INFINITE,FP_NORMAL,FP_SUBNORMAL,FP_ZERO,x)
typedef float float_t;
typedef double double_t;
double acos(double);
float acosf(float);
double asin(double);
float asinf(float);
double atan(double);
float atanf(float);
double acosh(double);
float acoshf(float);
double asinh(double);
float asinhf(float);
double atanh(double);
float atanhf(float);
double cos(double);
float cosf(float);
double sin(double);
float sinf(float);
double tan(double);
float tanf(float);
double cosh(double);
float coshf(float);
double sinh(double);
float sinhf(float);
double tanh(double);
float tanhf(float);
double exp(double);
float expf(float);
double exp2(double);
float exp2f(float);
double expm1(double);
float expm1f(float);
double log(double);
float logf(float);
double log2(double);
float log2f(float);
double log10(double);
float log10f(float);
double log1p(double);
float log1pf(float);
double sqrt(double);
float sqrtf(float);
double cbrt(double);
float cbrtf(float);
double ceil(double);
float ceilf(float);
double floor(double);
float floorf(float);
double trunc(double);
float truncf(float);
double round(double);
float roundf(float);
double rint(double);
float rintf(float);
double atan2(double, double);
float atan2f(float, float);
double pow(double, double);
float powf(float, float);
double hypot(double, double);
float hypotf(float, float);
double fmod(double, double);
float fmodf(float, float);
double remainder(double, double);
float remainderf(float, float);
double copysign(double, double);
float copysignf(float, float);
double nextafter(double, double);
float nextafterf(float, float);
double fdim(double, double);
float fdimf(float, float);
double fmax(double, double);
float fmaxf(float, float);
double fmin(double, double);
float fminf(float, float);
double ldexp(double, int);
float ldexpf(float, int);
double scalbn(double, int);
float scalbnf(float, int);
double frexp(double, int *);
float frexpf(float, int *);
double modf(double, double *);
float modff(float, float *);
double fabs(double);
float fabsf(float);
/* Both Scarlet C targets have IEEE binary128 long double. No fabsl is exposed
 * until the Rust implementation can honor that ABI without narrowing. */
#ifdef __cplusplus
}
#endif
#endif
