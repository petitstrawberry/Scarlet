/* Regression for SDL/Wayland cursor-theme pool growth. This exercises the
 * real protocol with Linux file-backed descriptors, without a GPU or window.
 */
#define _GNU_SOURCE
#include <wayland-client.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static struct wl_shm *shm;
static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version)
{
    (void)data;
    (void)version;
    if (!strcmp(interface, "wl_shm"))
        shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
}
static void removed(void *data, struct wl_registry *registry, uint32_t name)
{
    (void)data;
    (void)registry;
    (void)name;
}
static const struct wl_registry_listener listener = {global, removed};

static int resize_pool(struct wl_display *display, int fd, const char *kind)
{
    if (fd < 0 || ftruncate(fd, 4096)) return 1;
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, 4096);
    if (!pool || wl_display_roundtrip(display) < 0) return 2;
    /* A client owns the backing file; the compositor only remaps it. */
    if (ftruncate(fd, 8192)) return 3;
    wl_shm_pool_resize(pool, 8192);
    struct wl_buffer *buffer = wl_shm_pool_create_buffer(pool, 4096, 16, 16,
        16 * 4, WL_SHM_FORMAT_ARGB8888);
    if (!buffer || wl_display_roundtrip(display) < 0) return 4;
    wl_buffer_destroy(buffer);
    wl_shm_pool_destroy(pool);
    if (wl_display_roundtrip(display) < 0) return 5;
    close(fd);
    printf("PASS %s pool: 4096 -> 8192, buffer beyond original extent\n", kind);
    return 0;
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) { perror("Wayland connect"); return 1; }
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !shm) return 2;
    int status = resize_pool(display, memfd_create("wayland-resize-regression", MFD_CLOEXEC), "memfd");
    if (status) { fprintf(stderr, "FAIL memfd phase=%d error=%d\n", status, wl_display_get_error(display)); return 10 + status; }
    char name[] = "/tmp/wayland-resize-regression-XXXXXX";
    int fd = mkstemp(name);
    if (fd >= 0) unlink(name);
    status = resize_pool(display, fd, "unlinked file");
    if (status) { fprintf(stderr, "FAIL file phase=%d error=%d\n", status, wl_display_get_error(display)); return 20 + status; }
    wl_shm_destroy(shm);
    wl_registry_destroy(registry);
    wl_display_disconnect(display);
    return 0;
}
