#ifndef SCARLET_TIME_H
#define SCARLET_TIME_H
#include <sys/types.h>
struct timespec { time_t tv_sec; long tv_nsec; };
typedef int clockid_t;
#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
#ifdef __cplusplus
extern "C" {
#endif
time_t time(time_t *);
int clock_gettime(clockid_t, struct timespec *);
/* Native clock quantization is 1 microsecond; RTC accuracy is separate. */
int clock_getres(clockid_t, struct timespec *);
/* Native Sleep has no EINTR result; remaining is unchanged on success. */
int nanosleep(const struct timespec *, struct timespec *);
#ifdef __cplusplus
}
#endif
#endif
