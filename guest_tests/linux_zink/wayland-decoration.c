/* Live xdg-decoration regression. No SDL, GL, Vulkan, or GPU readback.
 * Press Enter after each STAGE to switch SSD -> CSD -> SSD -> fullscreen
 * -> restored SSD. Each stage waits for its Wayland frame callback first.
 * Build with the official generated xdg-shell/xdg-decoration client code. */
#define _GNU_SOURCE
#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"
#include "xdg-decoration-unstable-v1-client-protocol.h"
#include <sys/mman.h>
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>

static struct wl_display *display;
static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct xdg_wm_base *wm;
static struct zxdg_decoration_manager_v1 *manager;
static struct wl_surface *surface;
static struct xdg_surface *xdg;
static struct xdg_toplevel *top;
static struct zxdg_toplevel_decoration_v1 *deco;
static uint32_t mode, configure_count;
static int width = 400, height = 200, fullscreen, presented;
struct backing { struct wl_buffer *buffer; void *pixels; size_t size; };
static struct backing backings[8];
static unsigned backing_count;

static void ping(void *data, struct xdg_wm_base *base, uint32_t serial)
{ (void)data; xdg_wm_base_pong(base, serial); }
static const struct xdg_wm_base_listener wm_listener = { ping };
static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version)
{
    (void)data; (void)version;
    if (!strcmp(interface, "wl_compositor"))
        compositor = wl_registry_bind(registry, name, &wl_compositor_interface, 4);
    else if (!strcmp(interface, "wl_shm"))
        shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
    else if (!strcmp(interface, "xdg_wm_base")) {
        wm = wl_registry_bind(registry, name, &xdg_wm_base_interface, 2);
        xdg_wm_base_add_listener(wm, &wm_listener, NULL);
    } else if (!strcmp(interface, "zxdg_decoration_manager_v1"))
        manager = wl_registry_bind(registry, name, &zxdg_decoration_manager_v1_interface, 1);
}
static void removed(void *data, struct wl_registry *registry, uint32_t name)
{ (void)data; (void)registry; (void)name; }
static const struct wl_registry_listener registry_listener = {global, removed};
static void mode_configure(void *data, struct zxdg_toplevel_decoration_v1 *object, uint32_t value)
{ (void)data; (void)object; mode = value; }
static const struct zxdg_toplevel_decoration_v1_listener deco_listener = { mode_configure };
static void top_configure(void *data, struct xdg_toplevel *object, int32_t w, int32_t h,
                          struct wl_array *states)
{
    (void)data; (void)object;
    if (w > 0) width = w;
    if (h > 0) height = h;
    fullscreen = 0;
    uint32_t *state;
    wl_array_for_each(state, states)
        if (*state == XDG_TOPLEVEL_STATE_FULLSCREEN) fullscreen = 1;
}
static void close_requested(void *data, struct xdg_toplevel *object)
{ (void)data; (void)object; fprintf(stderr, "Unexpected close\n"); exit(10); }
static const struct xdg_toplevel_listener top_listener = { top_configure, close_requested };
static void surface_configure(void *data, struct xdg_surface *object, uint32_t serial)
{ (void)data; xdg_surface_ack_configure(object, serial); configure_count++; }
static const struct xdg_surface_listener surface_listener = { surface_configure };
static void frame_done(void *data, struct wl_callback *callback, uint32_t time)
{ (void)data; (void)time; wl_callback_destroy(callback); presented = 1; }
static const struct wl_callback_listener frame_listener = { frame_done };
static void dispatch(void)
{
    if (wl_display_dispatch(display) < 0) {
        fprintf(stderr, "Wayland failure errno=%d\n", wl_display_get_error(display)); exit(11);
    }
}
static void await_configure(uint32_t previous)
{ while (configure_count <= previous) dispatch(); }
static void present(const char *stage, uint32_t expected_mode, int expected_fullscreen)
{
    if (mode != expected_mode || fullscreen != expected_fullscreen || width <= 0 || height <= 0
        || width > 4096 || height > 4096 || backing_count == 8) {
        fprintf(stderr, "FAIL %s mode=%u fullscreen=%d size=%dx%d\n", stage, mode, fullscreen, width, height);
        exit(12);
    }
    size_t size = (size_t)width * height * 4;
    char path[] = "/tmp/wayland-decoration-probe-XXXXXX";
    int fd = mkstemp(path);
    if (fd < 0) { perror("mkstemp"); exit(13); }
    unlink(path);
    if (ftruncate(fd, size)) { perror("ftruncate"); exit(14); }
    uint32_t *pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) { perror("mmap"); exit(15); }
    for (int y = 0; y < height; y++) for (int x = 0; x < width; x++) {
        uint32_t color = 0xff285e90;
        if (x < 12 && y < 12) color = 0xffffffff; /* exact client origin */
        else if (x < 12 || y >= height - 12) color = 0xff46b978;
        if (mode == ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE && y < 32) {
            color = 0xffeea642; /* the client's own CSD band */
            if (x >= width-22 && x < width-10 && y >= 10 && y < 22) color = 0xff242424;
        }
        pixels[y * width + x] = color;
    }
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, size);
    struct wl_buffer *buffer = wl_shm_pool_create_buffer(pool, 0, width, height, width*4, WL_SHM_FORMAT_ARGB8888);
    wl_shm_pool_destroy(pool);
    close(fd);
    backings[backing_count++] = (struct backing){buffer, pixels, size};
    presented = 0;
    struct wl_callback *callback = wl_surface_frame(surface);
    wl_callback_add_listener(callback, &frame_listener, NULL);
    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, width, height);
    wl_surface_commit(surface);
    while (!presented) dispatch();
    printf("STAGE %s mode=%u fullscreen=%d client=%dx%d frame=presented\n", stage, mode, fullscreen, width, height);
    if (getchar() == EOF) { fprintf(stderr, "Stage input ended\n"); exit(16); }
}
int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    display = wl_display_connect(NULL);
    if (!display) { perror("wl_display_connect"); return 1; }
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !compositor || !shm || !wm || !manager) return 2;
    surface = wl_compositor_create_surface(compositor);
    xdg = xdg_wm_base_get_xdg_surface(wm, surface);
    xdg_surface_add_listener(xdg, &surface_listener, NULL);
    top = xdg_surface_get_toplevel(xdg);
    xdg_toplevel_add_listener(top, &top_listener, NULL);
    xdg_toplevel_set_title(top, "Wayland decoration probe");
    deco = zxdg_decoration_manager_v1_get_toplevel_decoration(manager, top);
    zxdg_toplevel_decoration_v1_add_listener(deco, &deco_listener, NULL);
    wl_surface_commit(surface);
    await_configure(0);
    present("SSD", ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE, 0);
    uint32_t previous = configure_count;
    zxdg_toplevel_decoration_v1_set_mode(deco, ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE);
    await_configure(previous);
    present("CSD", ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE, 0);
    previous = configure_count;
    zxdg_toplevel_decoration_v1_set_mode(deco, ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE);
    await_configure(previous);
    present("SSD_AGAIN", ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE, 0);
    previous = configure_count;
    xdg_toplevel_set_fullscreen(top, NULL);
    await_configure(previous);
    present("FULLSCREEN", ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE, 1);
    previous = configure_count;
    xdg_toplevel_unset_fullscreen(top);
    await_configure(previous);
    present("RESTORED", ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE, 0);
    zxdg_toplevel_decoration_v1_destroy(deco);
    xdg_toplevel_destroy(top);
    xdg_surface_destroy(xdg);
    wl_surface_destroy(surface);
    for (unsigned i = 0; i < backing_count; i++) {
        wl_buffer_destroy(backings[i].buffer);
        munmap(backings[i].pixels, backings[i].size);
    }
    if (wl_display_roundtrip(display) < 0) return 3;
    zxdg_decoration_manager_v1_destroy(manager);
    xdg_wm_base_destroy(wm);
    wl_shm_destroy(shm);
    wl_compositor_destroy(compositor);
    wl_registry_destroy(registry);
    wl_display_disconnect(display);
    puts("PASS SSD/CSD/SSD/fullscreen/restore/teardown");
    return 0;
}
