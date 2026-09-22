#include <assert.h>
#include <errno.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/time.h>
#include <time.h>

#define CHECK(expression) do { if (!(expression)) return __LINE__; } while (0)

static int assert_once(void) {
    int evaluations = 0;
    assert(++evaluations == 1);
    return evaluations;
}
#define NDEBUG
#include <assert.h>
static int assert_disabled(void) {
    int evaluations = 0;
    assert(++evaluations);
    return evaluations;
}
#undef NDEBUG
#include <assert.h>
static int assert_enabled_again(void) {
    int evaluations = 0;
    assert(++evaluations == 1);
    return evaluations;
}

/* Root harness runs these separately: they must terminate the whole process
 * with exit 134, never reach the return and never flush user stdio buffers. */
int scarlet_libc_assert_failure_test(void) {
    assert(0 && "scarlet assertion fixture");
    return 1;
}
int scarlet_libc_abort_test(void) {
    abort();
    return 1;
}

int scarlet_libc_runtime_test(void) {
    static_assert(sizeof(struct timespec) == 16, "timespec ABI");
    static_assert(sizeof(struct timeval) == 16, "timeval ABI");
    CHECK(assert_once() == 1 && assert_disabled() == 0 && assert_enabled_again() == 1);
    uint64_t bits64 = UINT64_C(0xfff8000000000042), output64;
    uint32_t bits32 = UINT32_C(0xffc00042), output32;
    double value64;
    float value32;
    memcpy(&value64, &bits64, sizeof(value64));
    value64 = fabs(value64);
    memcpy(&output64, &value64, sizeof(output64));
    CHECK(output64 == UINT64_C(0x7ff8000000000042));
    memcpy(&value32, &bits32, sizeof(value32));
    value32 = fabsf(value32);
    memcpy(&output32, &value32, sizeof(output32));
    CHECK(output32 == UINT32_C(0x7fc00042));
    value64 = fabs(-0.0);
    memcpy(&output64, &value64, sizeof(output64));
    CHECK(output64 == 0);
    value32 = fabsf(-0.0f);
    memcpy(&output32, &value32, sizeof(output32));
    CHECK(output32 == 0 && fabs(-3.5) == 3.5 && fabsf(-4.5f) == 4.5f);
    volatile float infinity = INFINITY, nan = NAN;
    CHECK(infinity > 1.0f && nan != nan);

    struct timespec before, after, resolution;
    CHECK(clock_getres(CLOCK_REALTIME, &resolution) == 0);
    CHECK(resolution.tv_sec == 0 && resolution.tv_nsec == 1000);
    CHECK(clock_getres(CLOCK_MONOTONIC, NULL) == 0);
    resolution.tv_sec = 17;
    CHECK(clock_getres(-1, &resolution) == -1 && errno == EINVAL && resolution.tv_sec == 17);
    CHECK(clock_gettime(2, &resolution) == -1 && errno == EINVAL);
    CHECK(clock_gettime(CLOCK_MONOTONIC, NULL) == -1 && errno == EFAULT);
    CHECK(clock_gettime(CLOCK_MONOTONIC, &before) == 0);
    CHECK(before.tv_sec >= 0 && before.tv_nsec >= 0 && before.tv_nsec < 1000000000L);
    CHECK(before.tv_nsec % 1000 == 0);
    struct timespec remaining = {41, 42}, request = {0, 2000000};
    CHECK(nanosleep(&request, &remaining) == 0 && remaining.tv_sec == 41 && remaining.tv_nsec == 42);
    CHECK(clock_gettime(CLOCK_MONOTONIC, &after) == 0);
    CHECK((after.tv_sec - before.tv_sec) * INT64_C(1000000000) + after.tv_nsec - before.tv_nsec >= 2000000);
    request.tv_nsec = 0;
    CHECK(nanosleep(&request, NULL) == 0);
    CHECK(nanosleep(NULL, &remaining) == -1 && errno == EFAULT);
    request.tv_nsec = -1;
    CHECK(nanosleep(&request, &remaining) == -1 && errno == EINVAL);
    request.tv_nsec = 1000000000L;
    CHECK(nanosleep(&request, &remaining) == -1 && errno == EINVAL);
    request.tv_sec = -1;
    request.tv_nsec = 0;
    CHECK(nanosleep(&request, &remaining) == -1 && errno == EINVAL);
    request.tv_sec = INT64_MAX;
    CHECK(nanosleep(&request, &remaining) == -1 && errno == EOVERFLOW);
    CHECK(remaining.tv_sec == 41 && remaining.tv_nsec == 42);

    struct timeval wall;
    CHECK(gettimeofday(NULL, NULL) == -1 && errno == EFAULT);
    CHECK(gettimeofday(&wall, &remaining) == -1 && errno == EINVAL);
    CHECK(clock_gettime(CLOCK_REALTIME, &before) == 0);
    CHECK(gettimeofday(&wall, NULL) == 0);
    time_t seconds = -2;
    CHECK(time(&seconds) >= 0 && seconds >= before.tv_sec);
    CHECK(clock_gettime(CLOCK_REALTIME, &after) == 0);
    CHECK(wall.tv_sec >= before.tv_sec && wall.tv_sec <= after.tv_sec);
    CHECK(wall.tv_usec >= 0 && wall.tv_usec < 1000000L);
    CHECK(seconds <= after.tv_sec);
    CHECK(time(NULL) >= seconds);

    unsigned char random[256];
    CHECK(getrandom(NULL, 0, 0) == 0 && getentropy(NULL, 0) == 0);
    CHECK(getrandom(NULL, 1, 0) == -1 && errno == EFAULT);
    CHECK(getentropy(NULL, 1) == -1 && errno == EFAULT);
    CHECK(getentropy(random, 257) == -1 && errno == EIO);
    CHECK(getrandom(random, 1, 4) == -1 && errno == EINVAL);
    CHECK(getrandom(random, 1, GRND_NONBLOCK) == -1 && errno == ENOTSUP);
    CHECK(getrandom(random, 1, GRND_RANDOM) == -1 && errno == ENOTSUP);
    CHECK(getrandom(random, (size_t)INT64_MAX + 1, 0) == -1 && errno == EINVAL);
    /* RTC is supplied by the native harness. Entropy hardware is optional;
     * absence must be explicit EIO, never a successful emergency PRNG fill. */
    ssize_t count = getrandom(random, sizeof(random), 0);
    CHECK((count >= 0 && count <= (ssize_t)sizeof(random)) || (count == -1 && errno == EIO));
    int entropy = getentropy(random, sizeof(random));
    CHECK(entropy == 0 || (entropy == -1 && errno == EIO));
    return 0;
}
