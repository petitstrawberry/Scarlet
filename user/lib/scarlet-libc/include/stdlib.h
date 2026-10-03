#ifndef SCARLET_STDLIB_H
#define SCARLET_STDLIB_H
#include <stddef.h>
#define MB_CUR_MAX ((size_t)1)
#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1
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
int atexit(void (*)(void));
#if defined(__cplusplus)
[[noreturn]] void abort(void);
[[noreturn]] void _Exit(int);
[[noreturn]] void exit(int);
#else
_Noreturn void abort(void);
_Noreturn void _Exit(int);
_Noreturn void exit(int);
#endif
char *realpath(const char *, char *);
char *getenv(const char *);
void qsort(void *, size_t, size_t, int (*)(const void *, const void *));
void *bsearch(const void *, const void *, size_t, size_t, int (*)(const void *, const void *));
int abs(int);
long labs(long);
long long llabs(long long);
typedef struct { int quot, rem; } div_t;
typedef struct { long quot, rem; } ldiv_t;
typedef struct { long long quot, rem; } lldiv_t;
div_t div(int, int);
ldiv_t ldiv(long, long);
lldiv_t lldiv(long long, long long);
/* Provided by the companion native libscarlet_float archive. */
float strtof(const char *, char **);
double strtod(const char *, char **);
long double strtold(const char *, char **);
double atof(const char *);
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
