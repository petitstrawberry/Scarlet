#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>

static int failures;
static void check(int condition, const char *name) {
    printf("%s: %s\n", condition ? "PASS" : "FAIL", name);
    failures += !condition;
}
static uint64_t now(clockid_t clock) {
    struct timespec time;
    if (clock_gettime(clock, &time)) {
        perror("clock_gettime");
        ++failures;
        return 0;
    }
    return (uint64_t)time.tv_sec * 1000000000 + time.tv_nsec;
}
int main(void) {
    struct timespec requested = {0, 5000000};
    uint64_t start = now(CLOCK_MONOTONIC);
    check(clock_nanosleep(CLOCK_MONOTONIC, 0, &requested, NULL) == 0 &&
          now(CLOCK_MONOTONIC) - start >= 5000000, "relative monotonic deadline");
    uint64_t deadline = now(CLOCK_MONOTONIC) + 5000000;
    requested = (struct timespec){deadline / 1000000000, deadline % 1000000000};
    check(clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &requested,
                          (struct timespec *)(uintptr_t)1) == 0 &&
          now(CLOCK_MONOTONIC) >= deadline, "absolute deadline ignores remain");
    requested = (struct timespec){0, 0};
    check(clock_nanosleep(CLOCK_REALTIME, TIMER_ABSTIME, &requested, NULL) == 0,
          "past realtime deadline");
    requested = (struct timespec){0, 1000000};
    start = now(CLOCK_BOOTTIME);
    check(clock_nanosleep(CLOCK_BOOTTIME, 0, &requested, NULL) == 0 &&
          now(CLOCK_BOOTTIME) - start >= 1000000, "relative boottime deadline");
    check(clock_nanosleep(CLOCK_THREAD_CPUTIME_ID, 0, &requested, NULL) == EINVAL,
          "thread CPU clock is invalid for clock_nanosleep");
    check(clock_nanosleep(CLOCK_MONOTONIC, 2, &requested, NULL) == EINVAL,
          "invalid flags");
    requested = (struct timespec){-1, 0};
    check(clock_nanosleep(CLOCK_MONOTONIC, 0, &requested, NULL) == EINVAL,
          "negative seconds");
    requested = (struct timespec){0, 1000000000};
    check(clock_nanosleep(CLOCK_MONOTONIC, 0, &requested, NULL) == EINVAL,
          "nanoseconds outside timespec range");
    check(clock_nanosleep(CLOCK_MONOTONIC, 0, NULL, NULL) == EFAULT,
          "null request pointer");
    check(clock_nanosleep(CLOCK_MONOTONIC, 0,
                          (struct timespec *)(uintptr_t)1, NULL) == EFAULT,
          "unmapped request pointer");
    requested = (struct timespec){0, -1};
    errno = 0;
    check(nanosleep(&requested, NULL) == -1 && errno == EINVAL,
          "nanosleep validates nanoseconds");
    requested = (struct timespec){0, 1000000};
    start = now(CLOCK_MONOTONIC);
    check(nanosleep(&requested, NULL) == 0 && now(CLOCK_MONOTONIC) - start >= 1000000,
          "nanosleep relative deadline");
    return failures != 0;
}
