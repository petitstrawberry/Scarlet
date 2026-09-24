#ifndef SCARLET_TIME_H
#define SCARLET_TIME_H
#include <sys/types.h>
struct timespec { time_t tv_sec; long tv_nsec; };
struct tm {
    int tm_sec;
    int tm_min;
    int tm_hour;
    int tm_mday;
    int tm_mon;
    int tm_year;
    int tm_wday;
    int tm_yday;
    int tm_isdst;
    long tm_gmtoff;
    const char *tm_zone;
};
typedef int clockid_t;
#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
#ifdef __cplusplus
extern "C" {
#endif
time_t time(time_t *);
time_t mktime(struct tm *);
double difftime(time_t, time_t);
int clock_gettime(clockid_t, struct timespec *);
/* Native clock quantization is 1 microsecond; RTC accuracy is separate. */
int clock_getres(clockid_t, struct timespec *);
/* Native Sleep has no EINTR result; remaining is unchanged on success. */
int nanosleep(const struct timespec *, struct timespec *);
/* Until Scarlet has a timezone database, local time is UTC. */
struct tm *gmtime_r(const time_t *, struct tm *);
struct tm *localtime_r(const time_t *, struct tm *);
struct tm *gmtime(const time_t *);
struct tm *localtime(const time_t *);
#ifdef __cplusplus
}
#endif
#endif
