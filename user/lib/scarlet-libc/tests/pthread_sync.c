#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <time.h>

#define CHECK(expression) do { if (!(expression)) return __LINE__; } while (0)
#define WORK_CHECK(expression) do { if (!(expression)) return (void *)(uintptr_t)__LINE__; } while (0)

struct counter_state {
    pthread_mutex_t mutex;
    unsigned counter;
};
static void *counter_worker(void *argument) {
    struct counter_state *state = argument;
    errno = 817;
    for (unsigned i = 0; i < 2000; ++i) {
        WORK_CHECK(pthread_mutex_lock(&state->mutex) == 0);
        state->counter++;
        WORK_CHECK(pthread_mutex_unlock(&state->mutex) == 0);
    }
    WORK_CHECK(errno == 817);
    return NULL;
}
static void *wrong_owner(void *argument) {
    pthread_mutex_t *mutex = argument;
    errno = 815;
    WORK_CHECK(pthread_mutex_trylock(mutex) == EBUSY);
    WORK_CHECK(pthread_mutex_unlock(mutex) == EPERM);
    WORK_CHECK(errno == 815);
    return NULL;
}
struct gate_state {
    pthread_mutex_t mutex;
    pthread_cond_t cond;
    int ready;
    int go;
    int turn;
};
static void *gate_worker(void *argument) {
    struct gate_state *state = argument;
    WORK_CHECK(pthread_mutex_lock(&state->mutex) == 0);
    state->ready = 1;
    WORK_CHECK(pthread_cond_broadcast(&state->cond) == 0);
    while (!state->go) WORK_CHECK(pthread_cond_wait(&state->cond, &state->mutex) == 0);
    WORK_CHECK(pthread_mutex_unlock(&state->mutex) == 0);
    for (unsigned i = 0; i < 200; ++i) {
        WORK_CHECK(pthread_mutex_lock(&state->mutex) == 0);
        while (!state->turn) WORK_CHECK(pthread_cond_wait(&state->cond, &state->mutex) == 0);
        state->turn = 0;
        WORK_CHECK(pthread_cond_signal(&state->cond) == 0);
        WORK_CHECK(pthread_mutex_unlock(&state->mutex) == 0);
    }
    return NULL;
}

struct binding_state {
    pthread_mutex_t old_mutex;
    pthread_cond_t cond;
    int ready;
    int go;
    int returned;
};
static void *binding_worker(void *argument) {
    struct binding_state *state = argument;
    WORK_CHECK(pthread_mutex_lock(&state->old_mutex) == 0);
    state->ready = 1;
    WORK_CHECK(pthread_cond_broadcast(&state->cond) == 0);
    while (!state->go) WORK_CHECK(pthread_cond_wait(&state->cond, &state->old_mutex) == 0);
    state->returned = 1;
    WORK_CHECK(pthread_mutex_unlock(&state->old_mutex) == 0);
    return NULL;
}

int scarlet_libc_pthread_sync_test(void) {
    _Static_assert(sizeof(pthread_mutex_t) == sizeof(uintptr_t), "mutex ABI");
    _Static_assert(sizeof(pthread_cond_t) == sizeof(uintptr_t), "condition ABI");
    pthread_mutexattr_t attr;
    pthread_mutex_t mutex;
    CHECK(pthread_mutexattr_init(&attr) == 0);
    int kind = -1;
    CHECK(pthread_mutexattr_gettype(&attr, &kind) == 0 && kind == PTHREAD_MUTEX_NORMAL);
    CHECK(pthread_mutexattr_setpshared(&attr, PTHREAD_PROCESS_SHARED) == ENOTSUP);
    CHECK(pthread_mutexattr_settype(&attr, 97) == EINVAL);
    CHECK(pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_RECURSIVE) == 0);
    CHECK(pthread_mutex_init(&mutex, &attr) == 0);
    errno = 819;
    CHECK(pthread_mutex_lock(&mutex) == 0 && pthread_mutex_trylock(&mutex) == 0);
    CHECK(pthread_mutex_destroy(&mutex) == EBUSY);
    pthread_cond_t nested = PTHREAD_COND_INITIALIZER;
    CHECK(pthread_cond_wait(&nested, &mutex) == EINVAL);
    CHECK(pthread_cond_destroy(&nested) == 0);
    CHECK(pthread_mutex_unlock(&mutex) == 0 && pthread_mutex_unlock(&mutex) == 0);
    CHECK(pthread_mutex_unlock(&mutex) == EPERM);
    CHECK(pthread_mutex_destroy(&mutex) == 0 && errno == 819);
    CHECK(pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_ERRORCHECK) == 0);
    CHECK(pthread_mutex_init(&mutex, &attr) == 0);
    CHECK(pthread_mutex_lock(&mutex) == 0);
    CHECK(pthread_mutex_lock(&mutex) == EDEADLK && pthread_mutex_trylock(&mutex) == EBUSY);
    pthread_t thread;
    void *result = (void *)(uintptr_t)1;
    CHECK(pthread_create(&thread, NULL, wrong_owner, &mutex) == 0);
    CHECK(pthread_join(thread, &result) == 0 && result == NULL);
    CHECK(pthread_mutex_unlock(&mutex) == 0);
    CHECK(pthread_mutex_destroy(&mutex) == 0 && pthread_mutexattr_destroy(&attr) == 0);

    struct counter_state counter = { PTHREAD_MUTEX_INITIALIZER, 0 };
    pthread_t workers[4];
    for (unsigned i = 0; i < 4; ++i) CHECK(pthread_create(&workers[i], NULL, counter_worker, &counter) == 0);
    for (unsigned i = 0; i < 4; ++i) CHECK(pthread_join(workers[i], &result) == 0 && result == NULL);
    CHECK(counter.counter == 8000);
    CHECK(pthread_mutex_destroy(&counter.mutex) == 0);

    struct gate_state gate = { PTHREAD_MUTEX_INITIALIZER, PTHREAD_COND_INITIALIZER, 0, 0, 0 };
    CHECK(pthread_create(&thread, NULL, gate_worker, &gate) == 0);
    CHECK(pthread_mutex_lock(&gate.mutex) == 0);
    while (!gate.ready) CHECK(pthread_cond_wait(&gate.cond, &gate.mutex) == 0);
    CHECK(pthread_cond_destroy(&gate.cond) == EBUSY);
    CHECK(pthread_mutex_destroy(&gate.mutex) == EBUSY);
    gate.go = 1;
    CHECK(pthread_cond_broadcast(&gate.cond) == 0);
    CHECK(pthread_mutex_unlock(&gate.mutex) == 0);
    for (unsigned i = 0; i < 200; ++i) {
        CHECK(pthread_mutex_lock(&gate.mutex) == 0);
        while (gate.turn) CHECK(pthread_cond_wait(&gate.cond, &gate.mutex) == 0);
        gate.turn = 1;
        CHECK(pthread_cond_signal(&gate.cond) == 0);
        CHECK(pthread_mutex_unlock(&gate.mutex) == 0);
    }
    CHECK(pthread_join(thread, &result) == 0 && result == NULL);
    CHECK(pthread_cond_destroy(&gate.cond) == 0 && pthread_mutex_destroy(&gate.mutex) == 0);

    /* Notification ends the dynamic mutex binding before an old waiter can
     * reacquire its mutex. The condition can even be destroyed and reused. */
    struct binding_state binding = { PTHREAD_MUTEX_INITIALIZER, PTHREAD_COND_INITIALIZER, 0, 0, 0 };
    CHECK(pthread_create(&thread, NULL, binding_worker, &binding) == 0);
    CHECK(pthread_mutex_lock(&binding.old_mutex) == 0);
    while (!binding.ready) CHECK(pthread_cond_wait(&binding.cond, &binding.old_mutex) == 0);
    binding.go = 1;
    CHECK(pthread_cond_broadcast(&binding.cond) == 0);
    pthread_mutex_t new_mutex = PTHREAD_MUTEX_INITIALIZER;
    struct timespec expired = { -1, 0 };
    CHECK(pthread_mutex_lock(&new_mutex) == 0);
    CHECK(pthread_cond_timedwait(&binding.cond, &new_mutex, &expired) == ETIMEDOUT);
    CHECK(binding.returned == 0);
    CHECK(pthread_cond_destroy(&binding.cond) == 0);
    CHECK(pthread_cond_init(&binding.cond, NULL) == 0);
    CHECK(pthread_cond_timedwait(&binding.cond, &new_mutex, &expired) == ETIMEDOUT);
    CHECK(pthread_cond_destroy(&binding.cond) == 0);
    CHECK(pthread_mutex_unlock(&new_mutex) == 0 && pthread_mutex_destroy(&new_mutex) == 0);
    CHECK(pthread_mutex_unlock(&binding.old_mutex) == 0);
    CHECK(pthread_join(thread, &result) == 0 && result == NULL && binding.returned == 1);
    CHECK(pthread_mutex_destroy(&binding.old_mutex) == 0);

    pthread_condattr_t condattr;
    CHECK(pthread_condattr_init(&condattr) == 0);
    clockid_t clock;
    CHECK(pthread_condattr_getclock(&condattr, &clock) == 0 && clock == CLOCK_REALTIME);
    CHECK(pthread_condattr_setclock(&condattr, 99) == EINVAL);
    CHECK(pthread_condattr_setpshared(&condattr, PTHREAD_PROCESS_SHARED) == ENOTSUP);
    CHECK(pthread_mutex_init(&mutex, NULL) == 0);
    for (clock = CLOCK_REALTIME; clock <= CLOCK_MONOTONIC; ++clock) {
        CHECK(pthread_condattr_setclock(&condattr, clock) == 0);
        pthread_cond_t cond;
        CHECK(pthread_cond_init(&cond, &condattr) == 0);
        CHECK(pthread_mutex_lock(&mutex) == 0);
        struct timespec deadline;
        CHECK(clock_gettime(clock, &deadline) == 0);
        deadline.tv_nsec += 2000000;
        if (deadline.tv_nsec >= 1000000000) { deadline.tv_sec++; deadline.tv_nsec -= 1000000000; }
        errno = 823;
        CHECK(pthread_cond_timedwait(&cond, &mutex, &deadline) == ETIMEDOUT && errno == 823);
        CHECK(pthread_mutex_trylock(&mutex) == EBUSY);
        deadline.tv_nsec = 1000000000;
        CHECK(pthread_cond_timedwait(&cond, &mutex, &deadline) == EINVAL);
        CHECK(pthread_mutex_unlock(&mutex) == 0);
        CHECK(pthread_cond_destroy(&cond) == 0);
    }
    CHECK(pthread_mutex_destroy(&mutex) == 0 && pthread_condattr_destroy(&condattr) == 0);
    return 0;
}
