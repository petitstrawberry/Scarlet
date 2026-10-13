#ifndef SCARLET_SYS_TIME_H
#define SCARLET_SYS_TIME_H
#include <sys/types.h>
typedef long suseconds_t;
struct timeval { time_t tv_sec; suseconds_t tv_usec; };
/* BSD timeval helpers. Arguments may be evaluated more than once. */
#define timerisset(tv) ((tv)->tv_sec || (tv)->tv_usec)
#define timerclear(tv) ((tv)->tv_sec = (tv)->tv_usec = 0)
#define timercmp(a, b, cmp) \
    (((a)->tv_sec == (b)->tv_sec) \
        ? ((a)->tv_usec cmp (b)->tv_usec) \
        : ((a)->tv_sec cmp (b)->tv_sec))
#ifdef __cplusplus
extern "C" {
#endif
/* The obsolete timezone argument must be NULL. */
int gettimeofday(struct timeval *, void *);
int utimes(const char *, const struct timeval [2]);
#ifdef __cplusplus
}
#endif
#include <sys/select.h>
#endif
