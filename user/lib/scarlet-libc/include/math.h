#ifndef SCARLET_MATH_H
#define SCARLET_MATH_H
#define INFINITY (__builtin_inff())
#define NAN (__builtin_nanf(""))
#define isnan(value) __builtin_isnan(value)
#ifdef __cplusplus
extern "C" {
#endif
double fabs(double);
float fabsf(float);
/* Both Scarlet C targets have IEEE binary128 long double. No fabsl is exposed
 * until the Rust implementation can honor that ABI without narrowing. */
#ifdef __cplusplus
}
#endif
#endif
