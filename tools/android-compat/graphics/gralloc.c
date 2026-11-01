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
#include "sws_client.h"

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
    void *client;
    uint32_t window;
    int enabled, closed;
    uint8_t *pixels;
    size_t capacity;
    int32_t (*size)(uint32_t,uint32_t *,uint32_t *);
    int32_t (*present)(uint32_t,const void *,size_t,uint32_t,uint32_t,size_t);
    int32_t (*destroy)(uint32_t);
    int32_t (*poll)(SwsEvent *);
    int32_t (*action)(uint32_t,uint32_t);
    int32_t (*fullscreen)(uint32_t,uint32_t);
} Framebuffer;
static int framebuffer_post(framebuffer_device_t *device,buffer_handle_t handle) {
    Framebuffer *fb=(Framebuffer *)device;
    pthread_mutex_lock(&fb->lock); int result=-ENODEV;
    static unsigned diagnostic_frames;
    SwsEvent event;
    for (int i=0;i<64 && fb->poll(&event)>0;i++)
        if (event.window_id==fb->window && event.kind==SWS_EVENT_DESTROYED) fb->closed=1;
    if (fb->closed) goto done;
    if (!fb->enabled) { result=0; goto done; }
    uint32_t width,height;
    if (fb->size(fb->window,&width,&height)<0 || !width || !height || width>MAX_DIMENSION || height>MAX_DIMENSION) goto done;
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
        /* Letterbox a fixed Android display when SWS changes the outer extent. */
        uint32_t outw=width,outh=(uint64_t)width*device->height/device->width;
        if (outh>height) { outh=height;outw=(uint64_t)height*device->width/device->height; }
        uint32_t left=(width-outw)/2,top=(height-outh)/2;
        memset(fb->pixels,0,bytes);
        int bpp=pixel_bytes(m->wire[FORMAT]);
        for (uint32_t y=0;y<outh;y++) for (uint32_t x=0;x<outw;x++) {
            size_t sx=(uint64_t)x*device->width/outw,sy=(uint64_t)y*device->height/outh;
            const uint8_t *p=(const uint8_t *)m->address+(sy*m->wire[STRIDE]+sx)*bpp;
            uint8_t *q=fb->pixels+((y+top)*width+x+left)*4;
            if (m->wire[FORMAT]==HAL_PIXEL_FORMAT_BGRA_8888) memcpy(q,p,4);
            else if (bpp==4) { q[0]=p[2];q[1]=p[1];q[2]=p[0];q[3]=m->wire[FORMAT]==HAL_PIXEL_FORMAT_RGBX_8888 ? 255 : p[3]; }
            else { uint16_t rgb=(uint16_t)p[0]|(uint16_t)p[1]<<8;q[0]=(rgb&31)*255/31;q[1]=((rgb>>5)&63)*255/63;q[2]=(rgb>>11)*255/31;q[3]=255; }
        }
        result=0;
    } else if (m && (m->readers || m->writer)) result=-EBUSY;
    pthread_mutex_unlock(&mappings_lock);
    if (!result && fb->present(fb->window,fb->pixels,bytes,width,height,(size_t)width*4)<0) result=-EIO;
    if (!result && __atomic_fetch_add(&diagnostic_frames,1,__ATOMIC_RELAXED)<4) {
        size_t nonzero=0;
        for (size_t offset=0;offset<bytes;offset+=256)
            nonzero+=(fb->pixels[offset]|fb->pixels[offset+1]|fb->pixels[offset+2])!=0;
        fprintf(stderr,"SCARLET_ANDROID_FB_POST width=%u height=%u sampled_nonzero=%zu\n",
                width,height,nonzero);
    }
done:
    pthread_mutex_unlock(&fb->lock);return result;
}
static int framebuffer_close_device(hw_device_t *device) {
    Framebuffer *fb=(Framebuffer *)device;
    pthread_mutex_lock(&fb->lock);
    fb->destroy(fb->window);free(fb->pixels);
    pthread_mutex_unlock(&fb->lock);pthread_mutex_destroy(&fb->lock);
    /* NODELETE retains the shared client's connection; balance this open ref. */
    dlclose(fb->client);
    free(fb);return 0;
}
static int swap_interval(framebuffer_device_t *dev,int interval) { (void)dev;return interval==1 ? 0 : -EINVAL; }
static int composition_complete(framebuffer_device_t *dev) { (void)dev;return 0; }
static int enable_screen(framebuffer_device_t *dev,int enabled) {
    if (enabled!=0 && enabled!=1) return -EINVAL;
    Framebuffer *fb=(Framebuffer *)dev;
    pthread_mutex_lock(&fb->lock);
    int result=fb->action(fb->window,enabled ? SWS_WINDOW_SHOW : SWS_WINDOW_HIDE)<0 ? -EIO : 0;
    if (!result) fb->enabled=enabled;
    pthread_mutex_unlock(&fb->lock);return result;
}
static int open_framebuffer(const hw_module_t *module,hw_device_t **device) {
    Framebuffer *fb=calloc(1,sizeof(*fb));
    if (!fb) return -ENOMEM;
    fb->client=dlopen("libsws_client_c.so",RTLD_NOW|RTLD_LOCAL|RTLD_NODELETE);
    if (!fb->client) { fprintf(stderr,"SWS framebuffer client load failed: %s\n",dlerror());free(fb);return -ENODEV; }
    int32_t (*create)(const char *,const char *,uint32_t,uint32_t,uint32_t,uint32_t *)=dlsym(fb->client,"sws_window_create_ex");
    fb->size=dlsym(fb->client,"sws_window_size");fb->present=dlsym(fb->client,"sws_window_present");
    fb->destroy=dlsym(fb->client,"sws_window_destroy");fb->poll=dlsym(fb->client,"sws_poll_event");
    fb->action=dlsym(fb->client,"sws_window_action");
    fb->fullscreen=dlsym(fb->client,"sws_window_fullscreen");
    uint32_t width=640,height=480;
    if (!create || !fb->size || !fb->present || !fb->destroy || !fb->poll || !fb->action ||
        create("dev.scarlet.android","Android",width,height,0,&fb->window)<0) { dlclose(fb->client);free(fb);return -ENODEV; }
    /* The Android framebuffer is the foreground device surface. Ask SWS to
     * use the complete primary output instead of presenting a fixed-size
     * desktop window. The configure event is consumed by the fullscreen call,
     * so query the resulting extent before publishing the HAL framebuffer. */
    if (fb->fullscreen && fb->fullscreen(fb->window,1)<0)
        fprintf(stderr,"SWS framebuffer fullscreen request failed\n");
    if (fb->size(fb->window,&width,&height)<0 || !width || !height || width>MAX_DIMENSION || height>MAX_DIMENSION) {
        fb->destroy(fb->window);dlclose(fb->client);free(fb);return -ENODEV;
    }
    /* Initialize the ABI's const fields as a whole object, before publication. */
    framebuffer_device_t initialized={
        .common={.tag=HARDWARE_DEVICE_TAG,.version=0,.module=(hw_module_t *)module,.close=framebuffer_close_device},
        .width=width,.height=height,.stride=width,.format=HAL_PIXEL_FORMAT_BGRA_8888,
        .xdpi=160,.ydpi=160,.fps=60,.minSwapInterval=1,.maxSwapInterval=1,.numFramebuffers=2,
        .setSwapInterval=swap_interval,.post=framebuffer_post,.compositionComplete=composition_complete,.enableScreen=enable_screen,
    };
    memcpy(&fb->device,&initialized,sizeof(initialized));
    pthread_mutex_init(&fb->lock,NULL);fb->enabled=1;*device=&fb->device.common;
    fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_READY width=%u height=%u\n",width,height);
    return 0;
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
