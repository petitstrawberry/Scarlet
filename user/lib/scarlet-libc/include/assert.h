/* assert is deliberately redefined on every inclusion, per the C standard. */
#ifndef SCARLET_ASSERT_H
#define SCARLET_ASSERT_H
#ifdef __cplusplus
extern "C" {
[[noreturn]] void __assert_fail(const char *, const char *, unsigned int, const char *);
}
#else
_Noreturn void __assert_fail(const char *, const char *, unsigned int, const char *);
#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
#define static_assert _Static_assert
#endif
#endif
#endif
#undef assert
#ifdef NDEBUG
#define assert(expression) ((void)0)
#else
#define assert(expression) ((expression) ? (void)0 : __assert_fail(#expression, __FILE__, __LINE__, __func__))
#endif
