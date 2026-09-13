#ifndef SWS_CLIENT_H
#define SWS_CLIENT_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* Link all consumers in a process to the same libsws_client_c.so. The library
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
       SWS_EVENT_DESTROYED = 3, SWS_EVENT_FOCUS = 4 };
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
int32_t sws_window_destroy(uint32_t window_id);
int32_t sws_window_fullscreen(uint32_t window_id, uint32_t enabled);
int32_t sws_window_pointer_lock(uint32_t window_id, uint32_t enabled);
int32_t sws_poll_event(SwsEvent *event);
/* raw_handle is a borrowed Scarlet object handle, never a Linux fd. Registration
 * duplicates it before sending it to SWS and retains the caller's ownership. */
int32_t sws_gpu_register(SwsBuffer buffer, uint32_t width, uint32_t height,
                         int32_t raw_handle);
int32_t sws_gpu_commit(SwsBuffer buffer, uint64_t commit_serial,
                       uint32_t width, uint32_t height);
int32_t sws_gpu_destroy(SwsBuffer buffer);
int32_t sws_gpu_poll(uint32_t window_id, SwsGpuEvent *event);

#ifdef __cplusplus
}
#endif
#endif
