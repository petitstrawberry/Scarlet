#ifndef SCARLET_SYS_TIME_H
#define SCARLET_SYS_TIME_H
#include <sys/types.h>
typedef long suseconds_t;
struct timeval { time_t tv_sec; suseconds_t tv_usec; };
#ifdef __cplusplus
extern "C" {
#endif
/* The obsolete timezone argument must be NULL. */
int gettimeofday(struct timeval *, void *);
#ifdef __cplusplus
}
#endif
#endif
