#ifndef SCARLET_STRING_H
#define SCARLET_STRING_H
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Supplied by the matching Rust compiler-builtins runtime. */
void *memcpy(void *, const void *, size_t);
void *memmove(void *, const void *, size_t);
void *memset(void *, int, size_t);
int memcmp(const void *, const void *, size_t);
size_t strlen(const char *);
void *memchr(const void *, int, size_t);
size_t strnlen(const char *, size_t);
int strcmp(const char *, const char *);
int strncmp(const char *, const char *, size_t);
char *strcpy(char *, const char *);
char *strncpy(char *, const char *, size_t);
char *strcat(char *, const char *);
char *strncat(char *, const char *, size_t);
char *strchr(const char *, int);
char *strrchr(const char *, int);
char *strstr(const char *, const char *);
size_t strspn(const char *, const char *);
size_t strcspn(const char *, const char *);
char *strpbrk(const char *, const char *);
char *strtok(char *, const char *);
char *strtok_r(char *, const char *, char **);
char *strdup(const char *);
char *strndup(const char *, size_t);
char *strerror(int);
#ifdef __cplusplus
}
#endif
#endif
