/* Compile this file as C11 and, via headers.cpp, C++11. Compile each public
 * header on its own as well: the aggregate cannot detect missing includes. */
#include <string.h>
#include <ctype.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <stdlib.h>
#include <limits.h>
#include <fcntl.h>
#include <errno.h>

/* Reverse the order and repeat every header to exercise include guards. */
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdlib.h>
#include <time.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <string.h>
#include <ctype.h>
#include <stdio.h>
#include <unistd.h>

#ifdef __cplusplus
#define HEADER_ASSERT(condition) static_assert(condition, #condition)
#define HEADER_ALIGNOF(type) alignof(type)
#else
#define HEADER_ASSERT(condition) _Static_assert(condition, #condition)
#define HEADER_ALIGNOF(type) _Alignof(type)
#endif

HEADER_ASSERT(CHAR_BIT == 8);
HEADER_ASSERT(SCHAR_MIN == -128 && SCHAR_MAX == 127 && UCHAR_MAX == 255);
HEADER_ASSERT(SHRT_MIN == -32768 && SHRT_MAX == 32767 && USHRT_MAX == 65535);
HEADER_ASSERT(INT_MIN == (-2147483647 - 1) && INT_MAX == 2147483647);
HEADER_ASSERT(UINT_MAX == 4294967295U);
HEADER_ASSERT(LONG_MIN == (-9223372036854775807L - 1L));
HEADER_ASSERT(LONG_MAX == 9223372036854775807L);
HEADER_ASSERT(ULONG_MAX == 18446744073709551615UL);
HEADER_ASSERT(LLONG_MIN == (-9223372036854775807LL - 1LL));
HEADER_ASSERT(LLONG_MAX == 9223372036854775807LL);
HEADER_ASSERT(ULLONG_MAX == 18446744073709551615ULL);
#ifdef __CHAR_UNSIGNED__
HEADER_ASSERT(CHAR_MIN == 0 && CHAR_MAX == 255);
#else
HEADER_ASSERT(CHAR_MIN == -128 && CHAR_MAX == 127);
#endif

/* The supported C ABI is LP64 on both AArch64 and RV64GC. */
HEADER_ASSERT(sizeof(void *) == 8 && sizeof(int) == 4 && sizeof(long) == 8);
HEADER_ASSERT(sizeof(size_t) == 8 && sizeof(ssize_t) == 8);
HEADER_ASSERT(sizeof(mode_t) == 4);
HEADER_ASSERT(sizeof(time_t) == 8 && sizeof(off_t) == 8);
HEADER_ASSERT(sizeof(struct timespec) == 16);
HEADER_ASSERT(HEADER_ALIGNOF(struct timespec) == 8);
HEADER_ASSERT(offsetof(struct timespec, tv_nsec) == 8);
HEADER_ASSERT(HEADER_ALIGNOF(max_align_t) <= 16);
HEADER_ASSERT((time_t)-1 < 0 && (off_t)-1 < 0 && (ssize_t)-1 < 0);
HEADER_ASSERT(PATH_MAX == 1024);
HEADER_ASSERT(UTIME_NOW != UTIME_OMIT);
HEADER_ASSERT(UTIME_NOW > 999999999L && UTIME_OMIT > 999999999L);

/* A repeated C declaration also detects accidental C++ linkage in a header. */
#ifdef __cplusplus
extern "C" {
#endif
int *__errno_location(void);
void *malloc(size_t);
void *calloc(size_t, size_t);
void *aligned_alloc(size_t, size_t);
int posix_memalign(void **, size_t, size_t);
void *realloc(void *, size_t);
void *reallocarray(void *, size_t, size_t);
void free(void *);
char *realpath(const char *, char *);
int futimens(int, const struct timespec *);
int utimensat(int, const char *, const struct timespec *, int);
int fsync(int);
int fdatasync(int);
#ifdef __cplusplus
}
#endif

int scarlet_libc_headers_probe(void) {
    /* errno must be an addressable, assignable int expression. */
    int *error = &errno;
    errno = ENOMEM;
    return *error;
}
