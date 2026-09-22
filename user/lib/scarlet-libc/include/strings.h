#ifndef SCARLET_STRINGS_H
#define SCARLET_STRINGS_H
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
/* ASCII C-locale comparison; bytes outside ASCII are unchanged. */
int strcasecmp(const char *, const char *);
int strncasecmp(const char *, const char *, size_t);
#ifdef __cplusplus
}
#endif
#endif
