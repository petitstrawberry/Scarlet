/* Upstream os.h explicitly supports replacing individual allocation wrappers.
 */
#ifndef SCARLET_WPA_OS_CONFIG_H
#define SCARLET_WPA_OS_CONFIG_H
#ifdef __scarlet__
#include "native_includes.h"
#endif
#include <stddef.h>
void *swpa_alloc(size_t);
void *swpa_resize(void *, size_t);
void swpa_release(void *);
char *swpa_strdup(const char *);
int swpa_format(char *, size_t, const char *, ...);
#define os_snprintf swpa_format
#ifdef __scarlet__
void swpa_native_abort(void) __attribute__((noreturn));
#define abort swpa_native_abort
#endif
#define os_malloc swpa_alloc
#define os_realloc swpa_resize
#define os_free swpa_release
#define os_strdup swpa_strdup
#endif
