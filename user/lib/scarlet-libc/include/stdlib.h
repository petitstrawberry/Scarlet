#ifndef SCARLET_STDLIB_H
#define SCARLET_STDLIB_H
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
void *malloc(size_t);
void *calloc(size_t, size_t);
void *aligned_alloc(size_t, size_t);
int posix_memalign(void **, size_t, size_t);
void *realloc(void *, size_t);
void *reallocarray(void *, size_t, size_t);
void free(void *);
#if defined(__cplusplus)
[[noreturn]] void abort(void);
#else
_Noreturn void abort(void);
#endif
char *realpath(const char *, char *);
void qsort(void *, size_t, size_t, int (*)(const void *, const void *));
void *bsearch(const void *, const void *, size_t, size_t, int (*)(const void *, const void *));
int abs(int);
long labs(long);
long long llabs(long long);
/* C-locale C17/POSIX grammar: 0x/0X and octal prefixes, without C23 0b/0B. */
long strtol(const char *, char **, int);
unsigned long strtoul(const char *, char **, int);
long long strtoll(const char *, char **, int);
unsigned long long strtoull(const char *, char **, int);
int atoi(const char *);
long atol(const char *);
long long atoll(const char *);
#ifdef __cplusplus
}
#endif
#endif
