/* SPDX-License-Identifier: MIT */
#include <stddef.h>
#include <pthread.h>
#include <stdlib.h>

/* The SDK CRT owns the initial environment; the Rust bridge exposes its
 * validated std::env snapshot to POSIX consumers before the VM starts. */
char **environ;
extern char **scarlet_jvm_environment(void);
void scarlet_jvm_initialize_runtime(void) { environ = scarlet_jvm_environment(); }

size_t wcslen(const wchar_t *s) {
    const wchar_t *end = s;
    while (*end) ++end;
    return (size_t)(end-s);
}

struct thread_destructor {
    void (*function)(void *);
    void *object;
    struct thread_destructor *next;
};
static pthread_key_t destructor_key;
static pthread_once_t destructor_once = PTHREAD_ONCE_INIT;
static int destructor_key_status;
static void destroy_thread_objects(void *value) {
    struct thread_destructor *current = value;
    /* POSIX clears the key before calling us. Preserve the remaining list
     * while invoking user code so newly registered destructors run first. */
    while (current) {
        struct thread_destructor *entry = current;
        pthread_setspecific(destructor_key, entry->next);
        entry->function(entry->object);
        free(entry);
        current = pthread_getspecific(destructor_key);
    }
    pthread_setspecific(destructor_key, NULL);
}
static void create_destructor_key(void) {
    destructor_key_status = pthread_key_create(&destructor_key, destroy_thread_objects);
}
int __cxa_thread_atexit_impl(void (*function)(void *), void *object, void *dso) {
    (void)dso; /* Static libraries cannot be unloaded. */
    if (pthread_once(&destructor_once, create_destructor_key) || destructor_key_status) return -1;
    struct thread_destructor *entry = malloc(sizeof(*entry));
    if (!entry) return -1;
    *entry = (struct thread_destructor){ function, object, pthread_getspecific(destructor_key) };
    int result = pthread_setspecific(destructor_key, entry);
    if (result) free(entry);
    return result ? -1 : 0;
}
