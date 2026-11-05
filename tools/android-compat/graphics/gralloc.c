/* CPU-backed Android gralloc/framebuffer HAL. Wire handles contain no pointers. */
#define _GNU_SOURCE
#include <hardware/gralloc.h>
#include <hardware/fb.h>
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
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <time.h>

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
    if (m && width==0 && height==0) {
        left=0;top=0;width=m->wire[WIDTH];height=m->wire[HEIGHT];
    }
    if (m && left>=0 && top>=0 && width>0 && height>0 &&
        left<=m->wire[WIDTH]-width && top<=m->wire[HEIGHT]-height) {
        int write=!!(usage & (GRALLOC_USAGE_SW_WRITE_MASK|GRALLOC_USAGE_HW_RENDER));
        if (write && (m->writer || m->readers)) result=-EBUSY;
        else if (!write && m->readers==INT_MAX) result=-EOVERFLOW;
        else { if (write) m->writer=1; else m->readers++; *address=m->address; result=0; }
    } else if (m) fprintf(stderr,"SCARLET_ANDROID_GRALLOC_LOCK_REJECT bounds=%d,%d+%dx%d buffer=%dx%d readers=%d writer=%d usage=%x\n",left,top,width,height,m->wire[WIDTH],m->wire[HEIGHT],m->readers,m->writer,usage);
    else fprintf(stderr,"SCARLET_ANDROID_GRALLOC_LOCK_REJECT unknown_handle usage=%x\n",usage);
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

/* The native manager owns the window, decoration and input. This HAL is a
 * synchronous, bounded CPU scanout producer; it sends no window/input commands. */
typedef struct Framebuffer {
    framebuffer_device_t device;
    pthread_mutex_t lock;
    int socket, enabled, error;
    uint64_t sequence;
    uint64_t mode_generation;
    uint32_t current_width, current_height, current_dpi;
    uint8_t *pixels;
    size_t capacity;
    pthread_t control_thread;
    int control_started;
    volatile int control_stop;
    volatile int awaiting_present;
} Framebuffer;

/* Dynamic framebuffer ABI used by the Scarlet composer. The public fb ABI
 * exposes immutable mode fields, so mode changes are published separately. */
__attribute__((visibility("default")))
int scarlet_framebuffer_get_mode(framebuffer_device_t *device,uint32_t *width,
                                 uint32_t *height,uint32_t *dpi) {
    if (!device || !width || !height || !dpi) return -EINVAL;
    Framebuffer *fb=(Framebuffer *)device;
    pthread_mutex_lock(&fb->lock);
    *width=fb->current_width;*height=fb->current_height;*dpi=fb->current_dpi;
    pthread_mutex_unlock(&fb->lock);
    return 0;
}

static void put16(uint8_t *p,uint16_t n) { p[0]=n;p[1]=n>>8; }
static void put32(uint8_t *p,uint32_t n) { for (unsigned i=0;i<4;i++)p[i]=n>>(i*8); }
static void put64(uint8_t *p,uint64_t n) { for (unsigned i=0;i<8;i++)p[i]=n>>(i*8); }
static uint32_t get32(const uint8_t *p) { return (uint32_t)p[0]|(uint32_t)p[1]<<8|(uint32_t)p[2]<<16|(uint32_t)p[3]<<24; }
static uint64_t get64(const uint8_t *p) { return get32(p)|(uint64_t)get32(p+4)<<32; }
static int resize_framebuffer_locked(Framebuffer *fb,uint32_t width,uint32_t height,uint32_t dpi) {
    if (width<2 || height<2 || width>MAX_DIMENSION || height>MAX_DIMENSION ||
        (uint64_t)width*height*4>MAX_BUFFER_BYTES)return -EINVAL;
    if (dpi<72 || dpi>640) return -EINVAL;
    if (width==fb->current_width && height==fb->current_height && dpi==fb->current_dpi)return 0;
    size_t capacity=(size_t)width*height*4;
    if (width!=fb->current_width || height!=fb->current_height) {
        uint8_t *resized=realloc(fb->pixels,capacity);
        if (!resized)return -ENOMEM;
        fb->pixels=resized;fb->capacity=capacity;
    }
    fb->current_width=width;fb->current_height=height;fb->current_dpi=dpi;
    ++fb->mode_generation;
    fb->awaiting_present=1;
    fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_RESIZED width=%u height=%u dpi=%u\n",width,height,dpi);
    return 0;
}
static int send_bytes(int fd,const void *input,size_t size) {
    const uint8_t *p=input;
    while (size) { ssize_t n=send(fd,p,size,MSG_NOSIGNAL);if (n<0 && errno==EINTR)continue;
        if (n<=0)return n ? -errno : -EPIPE;p+=n;size-=(size_t)n; }
    return 0;
}
static int receive_bytes(int fd,void *output,size_t size) {
    uint8_t *p=output;
    while (size) { ssize_t n=recv(fd,p,size,0);if (n<0 && errno==EINTR)continue;
        if (n<=0)return n ? -errno : -EPIPE;p+=n;size-=(size_t)n; }
    return 0;
}
static int transmit(Framebuffer *fb,unsigned kind,uint32_t width,uint32_t height,const void *pixels,size_t bytes) {
    if (fb->error)return fb->error;
    if (fb->sequence==UINT64_MAX)return -EOVERFLOW;
    uint8_t header[32]={0},ack[16];memcpy(header,"SADF",4);put16(header+4,1);put16(header+6,(uint16_t)kind);
    put64(header+8,++fb->sequence);put32(header+16,width);put32(header+20,height);
    put32(header+24,kind<=2 ? width*4 : 0);put32(header+28,(uint32_t)bytes);
    int error=send_bytes(fb->socket,header,sizeof(header));
    if (!error && bytes)error=send_bytes(fb->socket,pixels,bytes);
    if (!error)error=receive_bytes(fb->socket,ack,sizeof(ack));
    uint32_t status=error ? 0 : get32(ack+4);
    if (!error && memcmp(ack,"SADA",4))error=-EPROTO;
    if (!error && get64(ack+8)!=fb->sequence)error=-EPROTO;
    if (!error && (status & 0x80000000u)) {
        uint32_t requested_width=(status>>15)&0x7fffu,requested_height=status&0x7fffu;
        fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_RESIZE_REQUEST width=%u height=%u current=%ux%u\n",
                requested_width,requested_height,fb->current_width,fb->current_height);
        error=resize_framebuffer_locked(fb,requested_width,requested_height,fb->current_dpi);
    } else if (!error && status) error=-EPROTO;
    if (error) {
        fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_TRANSMIT_FAIL kind=%u error=%d status=%u\n",kind,error,status);
        fb->error=error;close(fb->socket);fb->socket=-1;
    }
    return error;
}
static void *display_control_loop(void *context) {
    Framebuffer *fb=context;
    int fd=open("/scarlet/runtime/display-request",O_RDONLY|O_CLOEXEC);
    uint32_t last_width=0,last_height=0,last_dpi=0;
    while (!fb->control_stop) {
        if (fd<0) {
            fd=open("/scarlet/runtime/display-request",O_RDONLY|O_CLOEXEC);
            if (fd<0) { usleep(100000); continue; }
        }
        uint8_t request[12];
        if (lseek(fd,0,SEEK_SET)<0) { close(fd);fd=-1;usleep(100000);continue; }
        ssize_t count=read(fd,request,sizeof(request));
        if (count!=12) { usleep(100000); continue; }
        uint32_t width=get32(request),height=get32(request+4);
        uint32_t dpi=get32(request+8);
        if (width==last_width && height==last_height && dpi==last_dpi) { usleep(100000); continue; }
        for (unsigned waited=0; fb->awaiting_present && !fb->control_stop && waited<100; ++waited) usleep(10000);
        if (fb->awaiting_present) {
            fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_PRESENT_TIMEOUT\n");
            fb->awaiting_present=0;
        }
        if (fb->control_stop) break;
        if (lseek(fd,0,SEEK_SET)<0) { close(fd);fd=-1;continue; }
        count=read(fd,request,sizeof(request));
        if (count!=12) continue;
        width=get32(request);height=get32(request+4);dpi=get32(request+8);
        if (width==last_width && height==last_height && dpi==last_dpi) { usleep(100000); continue; }
        last_width=width;last_height=height;last_dpi=dpi;
        pthread_mutex_lock(&fb->lock);
        int error=resize_framebuffer_locked(fb,width,height,dpi);
        pthread_mutex_unlock(&fb->lock);
        if (error)fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_CONTROL_FAIL width=%u height=%u dpi=%u error=%d\n",width,height,dpi,error);
        usleep(100000);
    }
    if (fd>=0)close(fd);
    return NULL;
}
static int framebuffer_post(framebuffer_device_t *device,buffer_handle_t handle) {
    Framebuffer *fb=(Framebuffer *)device;
    pthread_mutex_lock(&fb->lock);
    int result=fb->error;
    if (result || !fb->enabled)goto done;
    uint32_t width=fb->current_width,height=fb->current_height;
    uint64_t mode_generation=fb->mode_generation;
    pthread_mutex_lock(&mappings_lock);
    Mapping *m=find_locked(handle);result=-EINVAL;
    if (m && m->wire[WIDTH]==(int)width && m->wire[HEIGHT]==(int)height && !m->readers && !m->writer) {
        int bpp=pixel_bytes(m->wire[FORMAT]);
        for (uint32_t y=0;y<height;y++)for (uint32_t x=0;x<width;x++) {
            const uint8_t *p=(const uint8_t *)m->address+((size_t)y*m->wire[STRIDE]+x)*bpp;
            uint8_t *q=fb->pixels+((size_t)y*width+x)*4;
            if (m->wire[FORMAT]==HAL_PIXEL_FORMAT_BGRA_8888)memcpy(q,p,4);
            else if (bpp==4) {q[0]=p[2];q[1]=p[1];q[2]=p[0];q[3]=m->wire[FORMAT]==HAL_PIXEL_FORMAT_RGBX_8888 ? 255 : p[3];}
            else {uint16_t rgb=(uint16_t)p[0]|(uint16_t)p[1]<<8;q[0]=(rgb&31)*255/31;q[1]=((rgb>>5)&63)*255/63;q[2]=(rgb>>11)*255/31;q[3]=255;}
        }
        result=0;
    }else if(m && (m->readers||m->writer))result=-EBUSY;
    pthread_mutex_unlock(&mappings_lock);
    if (!result)result=transmit(fb,2,width,height,fb->pixels,fb->capacity);
    if (!result && fb->mode_generation==mode_generation)fb->awaiting_present=0;
done:
    pthread_mutex_unlock(&fb->lock);return result;
}
static int framebuffer_close_device(hw_device_t *device) {
    Framebuffer *fb=(Framebuffer *)device;
    if (fb->control_started) {
        fb->control_stop=1;
        pthread_join(fb->control_thread,NULL);
    }
    if (fb->socket>=0)close(fb->socket);
    free(fb->pixels);pthread_mutex_destroy(&fb->lock);free(fb);return 0;
}
static int swap_interval(framebuffer_device_t *dev,int interval) { (void)dev;return interval==1 ? 0 : -EINVAL; }
static int composition_complete(framebuffer_device_t *dev) { (void)dev;return 0; }
static int enable_screen(framebuffer_device_t *dev,int enabled) {
    if (enabled!=0 && enabled!=1)return -EINVAL;
    Framebuffer *fb=(Framebuffer *)dev;pthread_mutex_lock(&fb->lock);
    int result=transmit(fb,3,(uint32_t)enabled,0,NULL,0);
    if (!result)fb->enabled=enabled;
    pthread_mutex_unlock(&fb->lock);return result;
}
static int open_framebuffer(const hw_module_t *module,hw_device_t **device) {
    char configuration[257],path[108];unsigned width=0,height=0,dpi=0;int end=0;
    int file=open("/scarlet/runtime/display-endpoint",O_RDONLY|O_CLOEXEC);
    if (file<0)return -errno;
    ssize_t size;do {size=pread(file,configuration,sizeof(configuration)-1,0);}while (size<0 && errno==EINTR);
    int saved=errno;close(file);if (size<0)return -saved;
    if (!size || size==(ssize_t)sizeof(configuration)-1)return -EINVAL;
    configuration[size]=0;
    if (sscanf(configuration,"SAD1\n%107[^\n]\n%u %u %u\n%n",path,&width,&height,&dpi,&end)!=4 || end!=size || path[0]!='/' ||
        width<2 || height<2 || width>MAX_DIMENSION || height>MAX_DIMENSION || dpi<72 || dpi>640 ||
        (uint64_t)width*height*4>MAX_BUFFER_BYTES) {
        fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_CONFIG_REJECT size=%zd end=%d path=%s width=%u height=%u dpi=%u\n",
                size,end,path,width,height,dpi);
        return -EINVAL;
    }
    Framebuffer *fb=calloc(1,sizeof(*fb));if (!fb)return -ENOMEM;
    fb->socket=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0);
    if (fb->socket<0){saved=errno;free(fb);return -saved;}
    struct sockaddr_un address={.sun_family=AF_UNIX};strcpy(address.sun_path,path);
    if (connect(fb->socket,(struct sockaddr *)&address,sizeof(address))) {saved=errno;close(fb->socket);free(fb);return -saved;}
    framebuffer_device_t initialized={
        .common={.tag=HARDWARE_DEVICE_TAG,.version=0,.module=(hw_module_t *)module,.close=framebuffer_close_device},
        .width=width,.height=height,.stride=width,.format=HAL_PIXEL_FORMAT_BGRA_8888,
        .xdpi=dpi,.ydpi=dpi,.fps=60,.minSwapInterval=1,.maxSwapInterval=1,.numFramebuffers=2,
        .setSwapInterval=swap_interval,.post=framebuffer_post,.compositionComplete=composition_complete,.enableScreen=enable_screen,
    };
    memcpy(&fb->device,&initialized,sizeof(initialized));fb->capacity=(size_t)width*height*4;
    fb->current_width=width;fb->current_height=height;fb->current_dpi=dpi;
    fb->pixels=malloc(fb->capacity);if(!fb->pixels){close(fb->socket);free(fb);return -ENOMEM;}
    int error=pthread_mutex_init(&fb->lock,NULL);
    if(error){close(fb->socket);free(fb->pixels);free(fb);return -error;}
    fb->enabled=1;error=transmit(fb,1,width,height,NULL,0);
    if(error){framebuffer_close_device(&fb->device.common);return error;}
    if (!pthread_create(&fb->control_thread,NULL,display_control_loop,fb))fb->control_started=1;
    *device=&fb->device.common;
    fprintf(stderr,"SCARLET_ANDROID_FRAMEBUFFER_READY width=%u height=%u host=native\n",width,height);return 0;
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
