#ifndef SCARLET_STDLIB_H
#define SCARLET_STDLIB_H
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
void *malloc(size_t);
void *calloc(size_t, size_t);
void *realloc(void *, size_t);
void free(void *);
char *realpath(const char *, char *);
#ifdef __cplusplus
}
#endif
#endif
