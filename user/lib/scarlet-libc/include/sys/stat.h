#ifndef SCARLET_SYS_STAT_H
#define SCARLET_SYS_STAT_H
#include <time.h>
#define UTIME_NOW 1073741823L
#define UTIME_OMIT 1073741822L
#ifdef __cplusplus
extern "C" {
#endif
int futimens(int, const struct timespec [2]);
int utimensat(int, const char *, const struct timespec [2], int);
#ifdef __cplusplus
}
#endif
#endif
