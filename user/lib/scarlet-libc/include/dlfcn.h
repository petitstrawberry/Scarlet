#ifndef SCARLET_DLFCN_H
#define SCARLET_DLFCN_H

/* scarlet-ld currently supports immediate binding and the global namespace.
 * Unsupported modes are deliberately not declared as working alternatives. */
#define RTLD_NOW 2
#define RTLD_GLOBAL 0x100

#ifdef __cplusplus
extern "C" {
#endif
void *dlopen(const char *, int);
void *dlsym(void *, const char *);
int dlclose(void *);
char *dlerror(void);
#ifdef __cplusplus
}
#endif
#endif
