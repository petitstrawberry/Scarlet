/* CPU-backed Android gralloc/framebuffer HAL. Wire handles contain no pointers. */
#define _GNU_SOURCE
#include <hardware/gralloc.h>
#include <hardware/fb.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/memfd.h>
#include <limits.h>
#include <poll.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <time.h>
#include "sws_client.h"
#include "scarlet_ui_window.h"

enum { MAX_DIMENSION = 4096, MAX_BUFFERS = 256, HANDLE_INTS = 9,
       MAX_BUFFER_BYTES = 64 * 1024 * 1024, MAX_MAPPED_BYTES = 128 * 1024 * 1024,
       MAGIC = 0x53474231 };
/* data[0] is an fd. The nine ints describe layout and a producer-local ID. */
enum { FD, TAG, VERSION, WIDTH, HEIGHT, STRIDE, FORMAT, LENGTH, ID_LOW, ID_HIGH };
typedef struct Mapping {
    struct Mapping *next;
    buffer_handle_t handle;
    int wire[1 + HANDLE_INTS];
    dev_t device;
    ino_t inode;
    void *address;
    size_t length;
    int owned, readers, writer;
} Mapping;
static pthread_mutex_t mappings_lock = PTHREAD_MUTEX_INITIALIZER;
static Mapping *mappings;
static size_t mapped_bytes;
static unsigned mapping_count, next_id;
extern gralloc_module_t HAL_MODULE_INFO_SYM;

static int pixel_bytes(int format) {
    switch (format) {
        case HAL_PIXEL_FORMAT_RGBA_8888:
        case HAL_PIXEL_FORMAT_RGBX_8888:
        case HAL_PIXEL_FORMAT_BGRA_8888: return 4;
        case HAL_PIXEL_FORMAT_RGB_565: return 2;
        default: return 0;
    }
}
static int layout(int width, int height, int format, int *stride, size_t *length) {
    int bpp = pixel_bytes(format);
    if (!bpp || width < 1 || height < 1 || width > MAX_DIMENSION || height > MAX_DIMENSION) return -EINVAL;
    *stride = (width + 63 / bpp) & ~(63 / bpp);
    *length = (size_t)*stride * height * bpp;
    return *length <= MAX_BUFFER_BYTES ? 0 : -ENOMEM;
}
static int describe(buffer_handle_t handle, struct stat *statbuf, size_t *length) {
    if (!handle || handle->version != sizeof(native_handle_t) || handle->numFds != 1 || handle->numInts != HANDLE_INTS) return -EINVAL;
    const int *d = handle->data;
    int stride; size_t computed;
    if (d[TAG] != MAGIC || d[VERSION] != 1 || d[FD] < 0 ||
        layout(d[WIDTH], d[HEIGHT], d[FORMAT], &stride, &computed) ||
        d[STRIDE] != stride || d[LENGTH] < 0 || (size_t)d[LENGTH] != computed) return -EINVAL;
    if (fstat(d[FD], statbuf) || !S_ISREG(statbuf->st_mode) || statbuf->st_size != (off_t)computed) return -EINVAL;
    int seals = fcntl(d[FD], F_GET_SEALS);
    if (seals < 0 || (seals & (F_SEAL_SHRINK | F_SEAL_GROW)) != (F_SEAL_SHRINK | F_SEAL_GROW)) return -EINVAL;
    *length = computed;
    return 0;
}
static Mapping *find_locked(buffer_handle_t handle) {
    for (Mapping *m = mappings; m; m = m->next) if (m->handle == handle) {
        struct stat st; size_t length;
        if (describe(handle, &st, &length) || m->device != st.st_dev || m->inode != st.st_ino ||
            memcmp(m->wire, handle->data, sizeof(m->wire))) return NULL;
        return m;
    }
    return NULL;
}
static int import_locked(buffer_handle_t handle, int owned) {
    struct stat st; size_t length;
    int result = describe(handle, &st, &length);
    if (result) return result;
    for (Mapping *m = mappings; m; m = m->next) if (m->handle == handle) return -EEXIST;
    if (mapping_count >= MAX_BUFFERS || length > MAX_MAPPED_BYTES - mapped_bytes) return -ENOMEM;
    Mapping *m = calloc(1, sizeof(*m));
    if (!m) return -ENOMEM;
    void *address = mmap(NULL, length, PROT_READ | PROT_WRITE, MAP_SHARED, handle->data[FD], 0);
    if (address == MAP_FAILED) { result = -errno; free(m); return result; }
    *m = (Mapping){ .next=mappings, .handle=handle, .device=st.st_dev, .inode=st.st_ino,
                    .address=address, .length=length, .owned=owned };
    memcpy(m->wire, handle->data, sizeof(m->wire));
    mappings=m; mapped_bytes+=length; mapping_count++;
    return 0;
}
static int remove_locked(buffer_handle_t handle, int owned) {
    Mapping *m = find_locked(handle);
    if (!m || m->owned != owned) return -EINVAL;
    if (m->readers || m->writer) return -EBUSY;
    Mapping **link = &mappings;
    while (*link != m) link = &(*link)->next;
    *link=m->next; mapped_bytes-=m->length; mapping_count--;
    munmap(m->address, m->length); free(m);
    return 0;
}
static int register_buffer(const gralloc_module_t *module, buffer_handle_t handle) {
    (void)module; pthread_mutex_lock(&mappings_lock);
    int result=import_locked(handle,0);
    pthread_mutex_unlock(&mappings_lock); return result;
}
static int unregister_buffer(const gralloc_module_t *module, buffer_handle_t handle) {
    (void)module; pthread_mutex_lock(&mappings_lock);
    int result=remove_locked(handle,0);
    pthread_mutex_unlock(&mappings_lock); return result;
}
static int allocate(alloc_device_t *dev, int width, int height, int format, int usage,
                    buffer_handle_t *handle, int *stride) {
    (void)dev;
    if (!handle || !stride) return -EINVAL;
    *handle=NULL; *stride=0;
    if (usage & GRALLOC_USAGE_PROTECTED) return -ENOTSUP;
    size_t length; int row;
    int result=layout(width,height,format,&row,&length);
    if (result) return result;
    native_handle_t *h=calloc(1,sizeof(*h)+(1+HANDLE_INTS)*sizeof(int));
    if (!h) return -ENOMEM;
    int fd=syscall(__NR_memfd_create,"scarlet-gralloc",MFD_CLOEXEC|MFD_ALLOW_SEALING);
    if (fd<0) { result=-errno; free(h); return result; }
    if (ftruncate(fd,length) || fcntl(fd,F_ADD_SEALS,F_SEAL_SHRINK|F_SEAL_GROW|F_SEAL_SEAL)) {
        result=-errno; close(fd); free(h); return result;
    }
    h->version=sizeof(*h); h->numFds=1; h->numInts=HANDLE_INTS;
    h->data[FD]=fd; h->data[TAG]=MAGIC; h->data[VERSION]=1;
    h->data[WIDTH]=width; h->data[HEIGHT]=height; h->data[STRIDE]=row;
    h->data[FORMAT]=format; h->data[LENGTH]=length; h->data[ID_HIGH]=getpid();
    pthread_mutex_lock(&mappings_lock);
    if (next_id == UINT32_MAX) result=-EOVERFLOW;
    else { h->data[ID_LOW]=++next_id; result=import_locked(h,1); }
    pthread_mutex_unlock(&mappings_lock);
    if (result) { close(fd); free(h); return result; }
    *handle=h; *stride=row; return 0;
}
static int release_buffer(alloc_device_t *dev, buffer_handle_t handle) {
    (void)dev; pthread_mutex_lock(&mappings_lock);
    int result=remove_locked(handle,1);
    pthread_mutex_unlock(&mappings_lock);
    if (!result) { close(handle->data[FD]); free((void *)handle); }
    return result;
}
static int lock_buffer(const gralloc_module_t *module, buffer_handle_t handle, int usage,
                       int left, int top, int width, int height, void **address) {
    (void)module;
    if (!address || usage & GRALLOC_USAGE_PROTECTED) return -EINVAL;
    *address=NULL; pthread_mutex_lock(&mappings_lock);
    Mapping *m=find_locked(handle); int result=-EINVAL;
    if (m && left>=0 && top>=0 && width>0 && height>0 &&
        left<=m->wire[WIDTH]-width && top<=m->wire[HEIGHT]-height) {
        int write=!!(usage & (GRALLOC_USAGE_SW_WRITE_MASK|GRALLOC_USAGE_HW_RENDER));
        if (m->writer || (write && m->readers)) result=-EBUSY;
        else if (!write && m->readers==INT_MAX) result=-EOVERFLOW;
        else { if (write) m->writer=1; else m->readers++; *address=m->address; result=0; }
    }
    pthread_mutex_unlock(&mappings_lock); return result;
}
static int unlock_buffer(const gralloc_module_t *module, buffer_handle_t handle) {
    (void)module; pthread_mutex_lock(&mappings_lock);
    Mapping *m=find_locked(handle); int result=-EINVAL;
    if (m && (m->writer || m->readers)) { if (m->writer) m->writer=0; else m->readers--; result=0; }
    pthread_mutex_unlock(&mappings_lock); return result;
}
static int wait_fence(int fd) {
    if (fd<0) return fd==-1 ? 0 : -EINVAL;
    struct pollfd p={.fd=fd,.events=POLLIN}; int result;
    do { result=poll(&p,1,-1); } while (result<0 && errno==EINTR);
    int error=result<0 ? -errno : (p.revents&(POLLERR|POLLNVAL)) ? -EINVAL : 0;
    close(fd); return error;
}
static int lock_async(const gralloc_module_t *module, buffer_handle_t handle, int usage,
                      int l,int t,int w,int h,void **address,int fence) {
    int result=wait_fence(fence);
    return result ? result : lock_buffer(module,handle,usage,l,t,w,h,address);
}
static int unlock_async(const gralloc_module_t *module,buffer_handle_t handle,int *fence) {
    if (!fence) return -EINVAL;
    *fence=-1; return unlock_buffer(module,handle);
}
static int transport_size(const gralloc_module_t *module,buffer_handle_t handle,uint32_t *fds,uint32_t *ints) {
    (void)module; struct stat st; size_t length;
    if (!fds || !ints || describe(handle,&st,&length)) return -EINVAL;
    *fds=1; *ints=HANDLE_INTS; return 0;
}
static int validate_size(const gralloc_module_t *module,buffer_handle_t handle,uint32_t w,uint32_t h,
                          int32_t format,int usage,uint32_t stride) {
    (void)module; struct stat st; size_t length;
    if (usage & GRALLOC_USAGE_PROTECTED || describe(handle,&st,&length)) return -EINVAL;
    return w==(uint32_t)handle->data[WIDTH] && h==(uint32_t)handle->data[HEIGHT] &&
           stride==(uint32_t)handle->data[STRIDE] && format==handle->data[FORMAT] ? 0 : -EINVAL;
}
static int unsupported(const gralloc_module_t *module,int operation,...) { (void)module;(void)operation;return -ENOTSUP; }
static int lock_ycbcr(const gralloc_module_t *m,buffer_handle_t b,int u,int l,int t,int w,int h,struct android_ycbcr *y) {
    (void)m;(void)b;(void)u;(void)l;(void)t;(void)w;(void)h;(void)y;return -ENOTSUP;
}
static int lock_async_ycbcr(const gralloc_module_t *m,buffer_handle_t b,int u,int l,int t,int w,int h,struct android_ycbcr *y,int fd) {
    int result=wait_fence(fd);return result ? result : lock_ycbcr(m,b,u,l,t,w,h,y);
}

/* Resolve the shared client only when a real framebuffer is requested. */
typedef struct Framebuffer {
    framebuffer_device_t device;
    pthread_mutex_t lock;
    pthread_cond_t wake;
    pthread_t ui_thread;
    void *client, *ui;
    ScarletUiFrame *frame;
    SwsSceneGeometry geometry;
    SwsSceneFrame chrome;
    uint8_t *chrome_pixels;
    SwsRect chrome_clip[256];
    int ready, stopping, pending, posting, result, visibility_changed;
    int x, y;
    unsigned frame_buttons, content_buttons;
    uint32_t window;
    int enabled, closed;
    uint8_t *pixels;
    size_t capacity;
    int32_t (*present)(uint32_t,const SwsSceneFrame *,const void *,size_t,uint32_t,uint32_t,size_t);
    int32_t (*create)(const char *,uint32_t,uint32_t,uint32_t,uint32_t *);
    int32_t (*scale)(uint32_t *);
    int32_t (*geometry_get)(uint32_t,SwsSceneGeometry *);
    uint32_t (*frame_abi)(void);
    int32_t (*frame_insets)(uint32_t,uint32_t,ScarletUiInsets *);
    ScarletUiFrame *(*frame_create)(void);
    int32_t (*frame_configure)(ScarletUiFrame *,const char *,uint32_t,uint32_t,uint32_t,uint32_t);
    int32_t (*frame_render)(ScarletUiFrame *,ScarletUiFrameView *);
    int32_t (*frame_pointer)(ScarletUiFrame *,uint32_t,int32_t,int32_t,uint32_t *);
    int32_t (*frame_dirty)(ScarletUiFrame *);
    void (*frame_destroy)(ScarletUiFrame *);
    int32_t (*destroy)(uint32_t);
    int32_t (*poll)(SwsEvent *);
    int32_t (*action)(uint32_t,uint32_t);
    int32_t (*fullscreen)(uint32_t,uint32_t);
} Framebuffer;
/* As in GLFW's Scarlet backend, the shared frame stays on one UI thread.
 * Android may post from other threads; posts hand off pixels and wait for the
 * presentation result. Idle input handling does not depend on Android draws. */
static int frame_refresh(Framebuffer *fb) {
    SwsSceneGeometry g;
    if (fb->geometry_get(fb->window,&g)<0) return -EIO;
    uint32_t flags=SCARLET_UI_FRAME_RESIZABLE;
    if (!(g.state & (4|16))) flags|=SCARLET_UI_FRAME_DECORATED;
    if (fb->frame_configure(fb->frame,"Android",g.width,g.height,g.scale_milli,flags)<0) return -EIO;
    ScarletUiInsets insets;
    if (fb->frame_insets(g.scale_milli,flags,&insets)<0) return -EIO;
    if (insets.left+insets.right>=g.width || insets.top+insets.bottom>=g.height) return -EINVAL;
    g.content=(SwsRect){insets.left,insets.top,g.width-insets.left-insets.right,g.height-insets.top-insets.bottom};
    fb->geometry=g;
    return 0;
}
static int frame_pointer(Framebuffer *fb,uint32_t kind,int x,int y) {
    uint32_t actions=0;
    if (fb->frame_pointer(fb->frame,kind,x,y,&actions)<0) return -EIO;
    if ((actions&SCARLET_UI_FRAME_MOVE) && fb->action(fb->window,SWS_WINDOW_MOVE)<0) return -EIO;
    if ((actions&SCARLET_UI_FRAME_MINIMIZE) && fb->action(fb->window,SWS_WINDOW_HIDE)<0) return -EIO;
    if (actions&SCARLET_UI_FRAME_TOGGLE_MAXIMIZE) {
        if (fb->action(fb->window,(fb->geometry.state&2) ? SWS_WINDOW_RESTORE : SWS_WINDOW_MAXIMIZE)<0) return -EIO;
    }
    if (actions&SCARLET_UI_FRAME_CLOSE) fb->closed=1;
    return 0;
}
static int in_content(Framebuffer *fb) {
    SwsRect r=fb->geometry.content;
    return fb->x>=(int)r.x && fb->y>=(int)r.y && fb->x<(int)(r.x+r.width) && fb->y<(int)(r.y+r.height);
}
static int frame_event(Framebuffer *fb,const SwsEvent *e) {
    if (e->window_id!=fb->window) return 0;
    if (e->kind==SWS_EVENT_DESTROYED || e->kind==SWS_EVENT_CLOSE_REQUESTED) { fb->closed=1;return 0; }
    if (e->kind==SWS_EVENT_CONFIGURE || e->kind==SWS_EVENT_SCALE || e->kind==SWS_EVENT_STATE) return frame_refresh(fb);
    if (e->kind==SWS_EVENT_FOCUS && !e->value) {
        if (fb->frame_buttons&1) {
            if (frame_pointer(fb,SCARLET_UI_POINTER_MOVE,-1,-1) || frame_pointer(fb,SCARLET_UI_POINTER_RELEASE,-1,-1)) return -EIO;
        }
        fb->frame_buttons=fb->content_buttons=0;
    }
    if (e->kind!=SWS_EVENT_INPUT) return 0;
    if ((e->type==2 || e->type==3) && e->code<=1) {
        int *position=e->code ? &fb->y : &fb->x;
        if (e->type==3) *position=e->value; else *position+=e->value;
        return frame_pointer(fb,SCARLET_UI_POINTER_MOVE,fb->x,fb->y);
    }
    if (e->type==1 && e->code>=0x110 && e->code<=0x117) {
        unsigned bit=1u<<(e->code-0x110);
        if (e->value) {
            if (fb->frame_buttons || (!fb->content_buttons && !in_content(fb))) {
                fb->frame_buttons|=bit;
                if (bit==1) return frame_pointer(fb,SCARLET_UI_POINTER_PRESS,fb->x,fb->y);
            } else fb->content_buttons|=bit;
        } else if (fb->frame_buttons&bit) {
            fb->frame_buttons&=~bit;
            if (bit==1) return frame_pointer(fb,SCARLET_UI_POINTER_RELEASE,fb->x,fb->y);
        } else fb->content_buttons&=~bit;
    }
    return 0;
}
static int frame_present(Framebuffer *fb) {
    int dirty=fb->frame_dirty(fb->frame);
    if (dirty<0) return -EIO;
    if (dirty || !fb->chrome_pixels) {
        ScarletUiFrameView view;
        if (fb->frame_render(fb->frame,&view)<0 || !view.clip_count || view.clip_count>=256 || view.bytes>MAX_BUFFER_BYTES) return -EIO;
        void *pixels=realloc(fb->chrome_pixels,view.bytes);
        if (!pixels) return -ENOMEM;
        fb->chrome_pixels=pixels;
        memcpy(fb->chrome_pixels,view.pixels,view.bytes);
        for (size_t i=0;i<view.clip_count;i++)
            fb->chrome_clip[i]=(SwsRect){view.clip[i].x,view.clip[i].y,view.clip[i].width,view.clip[i].height};
        fb->chrome=(SwsSceneFrame){view.width,view.height,view.stride,
            {view.insets.left,view.insets.top,view.insets.right,view.insets.bottom},
            fb->chrome_pixels,view.bytes,fb->chrome_clip,view.clip_count};
    }
    /* The FFI view expires on the next frame call. Keep only its pixel snapshot
     * between repaints; the live shared controls remain owned by this thread. */
    return fb->present(fb->window,&fb->chrome,fb->pixels,fb->capacity,fb->device.width,fb->device.height,(size_t)fb->device.width*4)<0 ? -EIO : 0;
}
static void *frame_loop(void *opaque) {
    Framebuffer *fb=opaque;
    pthread_mutex_lock(&fb->lock);
    uint32_t scale;ScarletUiInsets inset;
    fb->result=-EIO;
    if (fb->scale(&scale)<0 || fb->frame_insets(scale,3,&inset)<0) goto finished;
    fb->frame=fb->frame_create();
    if (!fb->frame || fb->create("Android",fb->device.width+inset.left+inset.right,
        fb->device.height+inset.top+inset.bottom,1,&fb->window)<0) goto finished;
    const char *fullscreen=getenv("SCARLET_ANDROID_FULLSCREEN");
    if (fullscreen && !strcmp(fullscreen,"1") && fb->fullscreen(fb->window,1)<0) goto finished;
    if (frame_refresh(fb) || frame_present(fb)) goto finished;
    fb->result=0;fb->ready=1;pthread_cond_broadcast(&fb->wake);
    while (!fb->stopping && !fb->closed) {
        int result=0,polled;SwsEvent event;
        /* Bound one batch so a continuous pointer stream cannot starve posts. */
        for (int i=0;i<128;i++) {
            polled=fb->poll(&event);
            if (polled<0) { result=-EIO;break; }
            if (!polled) break;
            result=frame_event(fb,&event);
            if (result || fb->closed) break;
        }
        if (result || fb->closed) { fb->result=result ? result : -ENODEV;break; }
        if (fb->visibility_changed) {
            result=fb->action(fb->window,fb->enabled ? SWS_WINDOW_SHOW : SWS_WINDOW_HIDE)<0 ? -EIO : 0;
            fb->visibility_changed=0;
        }
        int dirty=fb->frame_dirty(fb->frame);
        if (dirty<0) result=-EIO;
        if (!result && (fb->pending || dirty)) result=frame_present(fb);
        fb->result=result;
        fb->pending=0;pthread_cond_broadcast(&fb->wake);
        if (result) break;
        struct timespec until;
        if (clock_gettime(CLOCK_MONOTONIC,&until)) { fb->result=-errno;break; }
        until.tv_nsec+=16000000;
        if (until.tv_nsec>=1000000000) { until.tv_sec++;until.tv_nsec-=1000000000; }
        pthread_cond_timedwait(&fb->wake,&fb->lock,&until);
    }
finished:
    if (fb->result && !fb->stopping) fprintf(stderr,"SWS framebuffer event loop failed: %d\n",fb->result);
    free(fb->chrome_pixels);fb->chrome_pixels=NULL;
    if (fb->window) fb->destroy(fb->window);
    if (fb->frame) fb->frame_destroy(fb->frame);
    fb->closed=1;fb->ready=1;fb->pending=0;pthread_cond_broadcast(&fb->wake);
    pthread_mutex_unlock(&fb->lock);
    return NULL;
}
static int framebuffer_post(framebuffer_device_t *device,buffer_handle_t handle) {
    Framebuffer *fb=(Framebuffer *)device;
    pthread_mutex_lock(&fb->lock); int result=-ENODEV;
    static unsigned diagnostic_frames;
    static unsigned diagnostic_failures;
    while (fb->posting && !fb->closed) pthread_cond_wait(&fb->wake,&fb->lock);
    if (fb->closed) { pthread_mutex_unlock(&fb->lock);return -ENODEV; }
    fb->posting=1;
    if (!fb->enabled) { result=0; goto done; }
    uint32_t width=device->width,height=device->height;
    size_t bytes=(size_t)width*height*4;
    if (bytes>MAX_BUFFER_BYTES) { result=-ENOMEM;goto done; }
    if (bytes>fb->capacity) {
        void *pixels=realloc(fb->pixels,bytes);
        if (!pixels) { result=-ENOMEM;goto done; }
        fb->pixels=pixels;fb->capacity=bytes;
    }
    pthread_mutex_lock(&mappings_lock);
    Mapping *m=find_locked(handle);
    result=-EINVAL;
    if (m && m->wire[WIDTH]==(int)device->width && m->wire[HEIGHT]==(int)device->height && !m->readers && !m->writer) {
        /* Keep the Android scanout extent stable. The scene presenter owns
         * window chrome, resizing and aspect-preserving letterboxing. */
        int bpp=pixel_bytes(m->wire[FORMAT]);
        for (uint32_t y=0;y<height;y++) for (uint32_t x=0;x<width;x++) {
            const uint8_t *p=(const uint8_t *)m->address+(y*m->wire[STRIDE]+x)*bpp;
            uint8_t *q=fb->pixels+(y*width+x)*4;
            if (m->wire[FORMAT]==HAL_PIXEL_FORMAT_BGRA_8888) memcpy(q,p,4);
            else if (bpp==4) { q[0]=p[2];q[1]=p[1];q[2]=p[0];q[3]=m->wire[FORMAT]==HAL_PIXEL_FORMAT_RGBX_8888 ? 255 : p[3]; }
            else { uint16_t rgb=(uint16_t)p[0]|(uint16_t)p[1]<<8;q[0]=(rgb&31)*255/31;q[1]=((rgb>>5)&63)*255/63;q[2]=(rgb>>11)*255/31;q[3]=255; }
        }
        result=0;
    } else if (m && (m->readers || m->writer)) result=-EBUSY;
    pthread_mutex_unlock(&mappings_lock);
    if (!result) {
        fb->pending=1;pthread_cond_broadcast(&fb->wake);
        while (fb->pending && !fb->closed) pthread_cond_wait(&fb->wake,&fb->lock);
        result=fb->closed ? -ENODEV : fb->result;
    }
    if (!result && __atomic_fetch_add(&diagnostic_frames,1,__ATOMIC_RELAXED)<4) {
        size_t nonzero=0;
        for (size_t offset=0;offset<bytes;offset+=256)
            nonzero+=(fb->pixels[offset]|fb->pixels[offset+1]|fb->pixels[offset+2])!=0;
        fprintf(stderr,"SCARLET_ANDROID_FB_POST width=%u height=%u sampled_nonzero=%zu\n",
                width,height,nonzero);
    }
done:
    if (result && __atomic_fetch_add(&diagnostic_failures,1,__ATOMIC_RELAXED)<8)
        fprintf(stderr,"SCARLET_ANDROID_FB_POST_ERROR result=%d\n",result);
    fb->posting=0;pthread_cond_broadcast(&fb->wake);
    pthread_mutex_unlock(&fb->lock);return result;
}
static int framebuffer_close_device(hw_device_t *device) {
    Framebuffer *fb=(Framebuffer *)device;
    pthread_mutex_lock(&fb->lock);
    fb->stopping=1;pthread_cond_broadcast(&fb->wake);
    pthread_mutex_unlock(&fb->lock);
    pthread_join(fb->ui_thread,NULL);
    free(fb->pixels);pthread_cond_destroy(&fb->wake);pthread_mutex_destroy(&fb->lock);
    /* NODELETE retains the shared client's connection; balance this open ref. */
    dlclose(fb->ui);dlclose(fb->client);
    free(fb);return 0;
}
static int swap_interval(framebuffer_device_t *dev,int interval) { (void)dev;return interval==1 ? 0 : -EINVAL; }
static int composition_complete(framebuffer_device_t *dev) { (void)dev;return 0; }
static int enable_screen(framebuffer_device_t *dev,int enabled) {
    if (enabled!=0 && enabled!=1) return -EINVAL;
    Framebuffer *fb=(Framebuffer *)dev;
    pthread_mutex_lock(&fb->lock);
    int result=fb->closed ? -ENODEV : 0;
    if (!result) {
        fb->enabled=enabled;fb->visibility_changed=1;pthread_cond_broadcast(&fb->wake);
        while (fb->visibility_changed && !fb->closed) pthread_cond_wait(&fb->wake,&fb->lock);
        result=fb->closed ? -ENODEV : fb->result;
    }
    pthread_mutex_unlock(&fb->lock);return result;
}
static int open_framebuffer(const hw_module_t *module,hw_device_t **device) {
    Framebuffer *fb=calloc(1,sizeof(*fb));
    if (!fb) return -ENOMEM;
    fb->client=dlopen("libsws_client_c.so",RTLD_NOW|RTLD_LOCAL|RTLD_NODELETE);
    if (!fb->client) { fprintf(stderr,"SWS framebuffer client load failed: %s\n",dlerror());free(fb);return -ENODEV; }
    fb->ui=dlopen("libscarlet_ui_window.so",RTLD_NOW|RTLD_LOCAL|RTLD_NODELETE);
    if (!fb->ui) { fprintf(stderr,"ScarletUI frame load failed: %s\n",dlerror());goto failed; }
#define LOAD(target,library,symbol) do { fb->target=dlsym(fb->library,symbol);if (!fb->target) goto failed; } while (0)
    LOAD(create,client,"sws_scene_create");LOAD(present,client,"sws_scene_cpu_present");
    LOAD(scale,client,"sws_get_output_scale");LOAD(geometry_get,client,"sws_scene_geometry");
    LOAD(destroy,client,"sws_window_destroy");LOAD(poll,client,"sws_poll_event");
    LOAD(action,client,"sws_window_action");LOAD(fullscreen,client,"sws_window_fullscreen");
    LOAD(frame_abi,ui,"scarlet_ui_frame_abi");LOAD(frame_insets,ui,"scarlet_ui_frame_insets");
    LOAD(frame_create,ui,"scarlet_ui_frame_create");LOAD(frame_configure,ui,"scarlet_ui_frame_configure");
    LOAD(frame_render,ui,"scarlet_ui_frame_render");LOAD(frame_pointer,ui,"scarlet_ui_frame_pointer");
    LOAD(frame_dirty,ui,"scarlet_ui_frame_dirty");LOAD(frame_destroy,ui,"scarlet_ui_frame_destroy");
#undef LOAD
    if (fb->frame_abi()!=1) goto failed;
    uint32_t width=640,height=480;
    /* Initialize the ABI's const fields as a whole object, before publication. */
    framebuffer_device_t initialized={
        .common={.tag=HARDWARE_DEVICE_TAG,.version=0,.module=(hw_module_t *)module,.close=framebuffer_close_device},
        .width=width,.height=height,.stride=width,.format=HAL_PIXEL_FORMAT_BGRA_8888,
        .xdpi=160,.ydpi=160,.fps=60,.minSwapInterval=1,.maxSwapInterval=1,.numFramebuffers=2,
        .setSwapInterval=swap_interval,.post=framebuffer_post,.compositionComplete=composition_complete,.enableScreen=enable_screen,
    };
    memcpy(&fb->device,&initialized,sizeof(initialized));
    fb->capacity=(size_t)width*height*4;fb->pixels=calloc(1,fb->capacity);
    if (!fb->pixels) goto failed;
    for (size_t i=3;i<fb->capacity;i+=4) fb->pixels[i]=255;
    if (pthread_mutex_init(&fb->lock,NULL)) goto failed;
    pthread_condattr_t attr;
    if (pthread_condattr_init(&attr)) goto failed_mutex;
    int error=pthread_condattr_setclock(&attr,CLOCK_MONOTONIC);
    if (!error) error=pthread_cond_init(&fb->wake,&attr);
    pthread_condattr_destroy(&attr);
    if (error) goto failed_mutex;
    fb->enabled=1;
    if (pthread_create(&fb->ui_thread,NULL,frame_loop,fb)) goto failed_cond;
    pthread_mutex_lock(&fb->lock);
    while (!fb->ready) pthread_cond_wait(&fb->wake,&fb->lock);
    int result=fb->result;int closed=fb->closed;
    pthread_mutex_unlock(&fb->lock);
    if (closed || result) { pthread_join(fb->ui_thread,NULL);goto failed_cond; }
    *device=&fb->device.common;
    fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_READY width=%u height=%u chrome=scarlet-ui\n",width,height);
    return 0;
failed_cond:
    pthread_cond_destroy(&fb->wake);
failed_mutex:
    pthread_mutex_destroy(&fb->lock);
failed:
    free(fb->pixels);
    if (fb->ui) dlclose(fb->ui);
    dlclose(fb->client);free(fb);return -ENODEV;
}

static int allocation_close(hw_device_t *device) { free(device);return 0; }
static int open_device(const hw_module_t *module,const char *name,hw_device_t **device) {
    if (!name || !device) return -EINVAL;
    *device=NULL;
    if (!strcmp(name,GRALLOC_HARDWARE_FB0)) return open_framebuffer(module,device);
    if (strcmp(name,GRALLOC_HARDWARE_GPU0)) return -EINVAL;
    alloc_device_t *allocation=calloc(1,sizeof(*allocation));
    if (!allocation) return -ENOMEM;
    allocation->common=(hw_device_t){.tag=HARDWARE_DEVICE_TAG,.version=0,.module=(hw_module_t *)module,.close=allocation_close};
    allocation->alloc=allocate;allocation->free=release_buffer;*device=&allocation->common;return 0;
}
static hw_module_methods_t methods={.open=open_device};
gralloc_module_t HAL_MODULE_INFO_SYM={
    .common={.tag=HARDWARE_MODULE_TAG,.module_api_version=GRALLOC_MODULE_API_VERSION_0_3,
             .hal_api_version=HARDWARE_HAL_API_VERSION,.id=GRALLOC_HARDWARE_MODULE_ID,
             .name="Scarlet CPU buffers and SWS framebuffer",.author="Scarlet",.methods=&methods},
    .registerBuffer=register_buffer,.unregisterBuffer=unregister_buffer,.lock=lock_buffer,.unlock=unlock_buffer,
    .perform=unsupported,.lock_ycbcr=lock_ycbcr,.lockAsync=lock_async,.unlockAsync=unlock_async,
    .lockAsync_ycbcr=lock_async_ycbcr,.getTransportSize=transport_size,.validateBufferSize=validate_size,
};
