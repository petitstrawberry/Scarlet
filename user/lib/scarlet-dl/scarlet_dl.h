#ifndef SCARLET_DL_H
#define SCARLET_DL_H

/* The initial implementation accepts exactly RTLD_NOW | RTLD_GLOBAL.
 * All loaded objects remain mapped until process exit. */
#define RTLD_NOW 2
#define RTLD_GLOBAL 0x100
#define RTLD_DEFAULT ((void *)0)

#ifdef __cplusplus
extern "C" {
#endif
void *dlopen(const char *path, int flags);
void *dlsym(void *handle, const char *name);
int dlclose(void *handle);
char *dlerror(void);
#ifdef __cplusplus
}
#endif

#endif
