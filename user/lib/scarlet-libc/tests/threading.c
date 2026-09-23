#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stddef.h>
#include <time.h>

#define CHECK(expr) do { if (!(expr)) return __LINE__; } while (0)
#define THREADS 8

static pthread_once_t once = PTHREAD_ONCE_INIT;
static atomic_int once_count;
static pthread_key_t key;

struct context {
    pthread_t self;
    atomic_int destructor_count;
    atomic_int error;
};

static void once_init(void) {
    atomic_fetch_add_explicit(&once_count, 1, memory_order_relaxed);
    errno = EINVAL;
}

static void key_destructor(void *value) {
    struct context *ctx = value;
    if (pthread_getspecific(key) != NULL || !pthread_equal(pthread_self(), ctx->self))
        atomic_store_explicit(&ctx->error, __LINE__, memory_order_relaxed);
    errno = ERANGE;
    /* Reinstall on all four rounds. A fifth callback must never occur. This
       calls into the C TLS API while its Rust cleanup marker is dropping. */
    if (pthread_setspecific(key, ctx) != 0 || errno != ERANGE)
        atomic_store_explicit(&ctx->error, __LINE__, memory_order_relaxed);
    atomic_fetch_add_explicit(&ctx->destructor_count, 1, memory_order_release);
}

static void *worker(void *value) {
    struct context *ctx = value;
    ctx->self = pthread_self();
    errno = EBUSY;
    if (pthread_once(&once, once_init) != 0 || errno != EBUSY ||
        pthread_setspecific(key, ctx) != 0 || errno != EBUSY ||
        pthread_getspecific(key) != ctx || errno != EBUSY ||
        pthread_join(pthread_self(), NULL) != EDEADLK || errno != EBUSY)
        atomic_store_explicit(&ctx->error, __LINE__, memory_order_relaxed);
    return value;
}

static void *roundtrip(void *value) { return value; }

static int await_destructors(struct context *ctx) {
    const struct timespec delay = { 0, 1000000 };
    /* Bounded wait: detached threads have no join handle. Acquiring the final
       counter ensures the callback has finished its access to this context. */
    for (int i = 0; i < 10000; ++i) {
        if (atomic_load_explicit(&ctx->destructor_count, memory_order_acquire) ==
            PTHREAD_DESTRUCTOR_ITERATIONS)
            return atomic_load_explicit(&ctx->error, memory_order_relaxed);
        nanosleep(&delay, NULL);
    }
    return __LINE__;
}

int scarlet_libc_threading_test(void) {
    pthread_t ids[THREADS];
    struct context contexts[THREADS] = {0};
    pthread_attr_t attr;
    int state;
    size_t stack_size;
    errno = ERANGE;
    pthread_t main_thread = pthread_self();
    CHECK(main_thread != 0 && pthread_equal(main_thread, pthread_self()));
    CHECK(pthread_join(main_thread, NULL) == EDEADLK && errno == ERANGE);
    CHECK(pthread_join((pthread_t)-1, NULL) == ESRCH && errno == ERANGE);
    CHECK(pthread_detach((pthread_t)-1) == ESRCH && errno == ERANGE);
    CHECK(pthread_create(NULL, NULL, roundtrip, NULL) == EINVAL && errno == ERANGE);
    CHECK(pthread_attr_init(&attr) == 0 && errno == ERANGE);
    CHECK(pthread_attr_getdetachstate(&attr, &state) == 0 && state == PTHREAD_CREATE_JOINABLE);
    CHECK(pthread_attr_setdetachstate(&attr, 8) == EINVAL && errno == ERANGE);
    CHECK(pthread_attr_getstacksize(&attr, &stack_size) == 0 && stack_size >= PTHREAD_STACK_MIN);
    CHECK(pthread_attr_setstacksize(&attr, PTHREAD_STACK_MIN - 1) == EINVAL);
    CHECK(pthread_attr_setstacksize(&attr, PTHREAD_STACK_MIN + 1) == EINVAL);
    CHECK(pthread_attr_setstacksize(&attr, (size_t)-1) == EINVAL);
    CHECK(pthread_attr_setstacksize(&attr, 256 * 1024) == 0);
    CHECK(pthread_attr_getstacksize(&attr, &stack_size) == 0 && stack_size == 256 * 1024);
    CHECK(pthread_key_create(&key, key_destructor) == 0 && errno == ERANGE);
    CHECK(pthread_getspecific(key) == NULL && errno == ERANGE);
    CHECK(pthread_setspecific(key, NULL) == 0 && errno == ERANGE);
    for (int i = 0; i < THREADS; ++i) {
        CHECK(pthread_create(&ids[i], &attr, worker, &contexts[i]) == 0 && errno == ERANGE);
        CHECK(!pthread_equal(ids[i], main_thread));
        for (int j = 0; j < i; ++j) CHECK(!pthread_equal(ids[i], ids[j]));
    }
    for (int i = 0; i < THREADS; ++i) {
        void *result = NULL;
        CHECK(pthread_join(ids[i], &result) == 0 && result == &contexts[i]);
        CHECK(pthread_equal(ids[i], contexts[i].self));
        CHECK(atomic_load(&contexts[i].error) == 0);
        CHECK(atomic_load(&contexts[i].destructor_count) == PTHREAD_DESTRUCTOR_ITERATIONS);
        CHECK(pthread_join(ids[i], NULL) == ESRCH && errno == ERANGE);
        CHECK(pthread_detach(ids[i]) == ESRCH && errno == ERANGE);
    }
    CHECK(atomic_load(&once_count) == 1);
    CHECK(pthread_getspecific(key) == NULL);
    for (int i = 0; i < 16; ++i) {
        void *result = NULL;
        CHECK(pthread_create(&ids[0], NULL, roundtrip, &attr) == 0);
        CHECK(pthread_join(ids[0], &result) == 0 && result == &attr);
    }
    /* Both create-detached and detach-after-create must eventually run all
       TSD destructor rounds. Each callback context remains alive until then. */
    for (int detached_at_create = 0; detached_at_create < 2; ++detached_at_create) {
        CHECK(pthread_attr_setdetachstate(&attr, detached_at_create ? PTHREAD_CREATE_DETACHED : PTHREAD_CREATE_JOINABLE) == 0);
        for (int i = 0; i < THREADS; ++i) {
            atomic_store(&contexts[i].destructor_count, 0);
            CHECK(pthread_create(&ids[i], &attr, worker, &contexts[i]) == 0);
            if (!detached_at_create) CHECK(pthread_detach(ids[i]) == 0);
            int error = pthread_join(ids[i], NULL);
            CHECK(error == EINVAL || error == ESRCH);
        }
        for (int i = 0; i < THREADS; ++i) CHECK(await_destructors(&contexts[i]) == 0);
    }
    errno = EBUSY;
    pthread_key_t deleted_key;
    CHECK(pthread_key_create(&deleted_key, key_destructor) == 0);
    CHECK(pthread_setspecific(deleted_key, &attr) == 0);
    CHECK(pthread_key_delete(deleted_key) == 0);
    CHECK(pthread_getspecific(deleted_key) == NULL);
    CHECK(pthread_setspecific(deleted_key, NULL) == EINVAL && errno == EBUSY);
    CHECK(pthread_key_delete(deleted_key) == EINVAL && errno == EBUSY);
    CHECK(pthread_key_delete(key) == 0 && errno == EBUSY);
    CHECK(pthread_attr_destroy(&attr) == 0 && errno == EBUSY);
    CHECK(pthread_attr_getstacksize(&attr, &stack_size) == EINVAL && errno == EBUSY);
    return 0;
}
