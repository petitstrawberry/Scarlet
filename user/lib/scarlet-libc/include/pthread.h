#ifndef SCARLET_PTHREAD_H
#define SCARLET_PTHREAD_H
#include <stddef.h>
#include <time.h>

typedef size_t pthread_t;
typedef unsigned int pthread_key_t;
typedef struct { size_t __stack_size; int __detach_state; unsigned int __magic; } pthread_attr_t;
typedef struct { size_t __state; } pthread_once_t;
#define PTHREAD_ONCE_INIT { 0 }
#define PTHREAD_CREATE_JOINABLE 0
#define PTHREAD_CREATE_DETACHED 1
#define PTHREAD_STACK_MIN 65536
#define PTHREAD_KEYS_MAX 1024
#define PTHREAD_DESTRUCTOR_ITERATIONS 4

#ifdef __cplusplus
extern "C" {
#endif
int pthread_create(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *);
int pthread_join(pthread_t, void **);
int pthread_detach(pthread_t);
pthread_t pthread_self(void);
int pthread_equal(pthread_t, pthread_t);
int pthread_attr_init(pthread_attr_t *);
int pthread_attr_destroy(pthread_attr_t *);
int pthread_attr_getdetachstate(const pthread_attr_t *, int *);
int pthread_attr_setdetachstate(pthread_attr_t *, int);
int pthread_attr_getstacksize(const pthread_attr_t *, size_t *);
/* Scarlet's Native backend accepts page-aligned stack sizes >= PTHREAD_STACK_MIN. */
int pthread_attr_setstacksize(pthread_attr_t *, size_t);
int pthread_once(pthread_once_t *, void (*)(void));
int pthread_key_create(pthread_key_t *, void (*)(void *));
int pthread_key_delete(pthread_key_t);
void *pthread_getspecific(pthread_key_t);
int pthread_setspecific(pthread_key_t, const void *);
#ifdef __cplusplus
}
#endif

#include <bits/pthread_sync.h>
#endif
