#ifndef SWS_CLIENT_H
#define SWS_CLIENT_H
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif

/* Link all consumers in a process to the same libsws_client_c.so, or link one
 * native static copy into the final executable. The library
 * owns one shared SWS connection; input and GPU lifecycle use separate queues.
 * Calls return 0 on success, or a negative SWS error. Poll returns 1 for an event,
 * 0 for an empty queue. All output pointers must point to writable records. */
typedef struct SwsDisplay {
    uint32_t width, height, compositor_epoch, compositor_backend;
    uint64_t capabilities;
} SwsDisplay;
typedef struct SwsEvent {
    uint32_t window_id, kind;
    uint64_t time;
    uint16_t type, code;
    int32_t value;
    uint32_t width, height;
} SwsEvent;
enum { SWS_EVENT_INPUT = 1, SWS_EVENT_CONFIGURE = 2,
       SWS_EVENT_DESTROYED = 3, SWS_EVENT_FOCUS = 4, SWS_EVENT_SCALE = 5, SWS_EVENT_STATE = 6, SWS_EVENT_CLOSE_REQUESTED = 7 };
/* SWS_EVENT_FOCUS.value is 1 when this window gains focus and 0 when it loses it. */
typedef struct SwsBuffer {
    uint32_t window_id, buffer_id, generation, compositor_epoch;
} SwsBuffer;
typedef struct SwsGpuEvent {
    SwsBuffer buffer;
    uint64_t commit_serial;
    uint32_t kind, code;
} SwsGpuEvent;
enum { SWS_GPU_RELEASED = 1, SWS_GPU_REJECTED = 2, SWS_GPU_BACKEND_LOST = 3 };

int32_t sws_get_display(SwsDisplay *display);
int32_t sws_window_create(const char *app_id, const char *title,
                          uint32_t width, uint32_t height, uint32_t *window_id);
int32_t sws_window_create_ex(const char *app_id, const char *title,
                           uint32_t width, uint32_t height, uint32_t resizable,
                           uint32_t *window_id);
int32_t sws_window_size(uint32_t window_id, uint32_t *width, uint32_t *height);
int32_t sws_window_resize(uint32_t window_id, uint32_t width, uint32_t height);
int32_t sws_window_title(uint32_t window_id, const char *title);
enum { SWS_WINDOW_SHOW=1, SWS_WINDOW_HIDE=2, SWS_WINDOW_RAISE=3,
       SWS_WINDOW_MAXIMIZE=4, SWS_WINDOW_RESTORE=5, SWS_WINDOW_MOVE=6 };
int32_t sws_window_action(uint32_t window_id, uint32_t action);
/* Copy a complete little-endian BGRA frame into the server's shared buffer.
 * Memory remains caller-owned and is borrowed only for this call. Pitched rows
 * and size are validated. A stale size after configure returns -1. */
int32_t sws_window_present(uint32_t window_id, const void *pixels, size_t bytes,
                           uint32_t width, uint32_t height, size_t pitch);
int32_t sws_window_destroy(uint32_t window_id);
/* Wait for the compositor's WINDOW_DESTROYED notification, sent after imported
 * GPU resources are retired. On timeout callers must retain producer resources;
 * retrying this call waits for the already-sent destruction request. */
int32_t sws_window_destroy_sync(uint32_t window_id, uint32_t timeout_ms);
int32_t sws_window_fullscreen(uint32_t window_id, uint32_t enabled);
int32_t sws_window_pointer_lock(uint32_t window_id, uint32_t enabled);
int32_t sws_poll_event(SwsEvent *event);
/* raw_handle is a borrowed Scarlet object handle, never a Linux fd. Registration
 * duplicates it before sending it to SWS and retains the caller's ownership. */
int32_t sws_gpu_register(SwsBuffer buffer, uint32_t width, uint32_t height,
                         int32_t raw_handle);
/* Opaque surfaces (including Vulkan COMPOSITE_ALPHA_OPAQUE) ignore stored alpha.
 * The original registration retains premultiplied-alpha scene semantics. */
int32_t sws_gpu_register_opaque(SwsBuffer buffer, uint32_t width, uint32_t height,
                                int32_t raw_handle);
int32_t sws_gpu_commit(SwsBuffer buffer, uint64_t commit_serial,
                       uint32_t width, uint32_t height);
int32_t sws_gpu_destroy(SwsBuffer buffer);
int32_t sws_gpu_poll(uint32_t window_id, SwsGpuEvent *event);

/* Compound scene windows use physical pixels throughout. size/resize and GPU
 * commits refer to the content area; input events remain outer-surface local.
 * STATE.value uses minimized=1, maximized=2, fullscreen=4, tablet-managed=16. SCALE.value is milli.
 * A frame is borrowed and copied before returning. Published pixels are immutable;
 * GPU content is composed by SWS, with no CPU readback. All consumers retain the
 * same shared connection and existing GPU identity/release serial contract. */
typedef struct SwsRect { uint32_t x,y,width,height; } SwsRect;
typedef struct SwsInsets { uint32_t left,top,right,bottom; } SwsInsets;
typedef struct SwsSceneGeometry {
    uint32_t width,height,scale_milli,state;
    SwsRect content;
} SwsSceneGeometry;
typedef struct SwsSceneFrame {
    uint32_t width,height,stride;
    SwsInsets insets;
    const void *pixels; size_t bytes;
    const SwsRect *clip; size_t clip_count;
} SwsSceneFrame;
/* Optional presentation timing: operation 2=commit, 3=size, 4=GPU events.
 * Called after releasing the client lock. Keep callback code loaded until
 * cleared; pass NULL to disable all timing clock reads. */
typedef void (*sws_timing_callback)(uint32_t operation, uint64_t lock_wait_ns, uint64_t service_ns);
void sws_set_timing_callback(sws_timing_callback callback);

int32_t sws_get_output_scale(uint32_t *scale_milli);
int32_t sws_scene_create(const char *title, uint32_t outer_width, uint32_t outer_height,
                         uint32_t resizable, uint32_t *window_id);
/* Resize the complete outer surface, for a host changing its frame insets. */
int32_t sws_scene_resize(uint32_t window_id, uint32_t outer_width, uint32_t outer_height);
int32_t sws_scene_geometry(uint32_t window_id, SwsSceneGeometry *geometry);
int32_t sws_scene_frame(uint32_t window_id, const SwsSceneFrame *frame);
int32_t sws_window_resizable(uint32_t window_id, uint32_t enabled);

#ifdef __cplusplus
}
#endif
#endif
