#define _GNU_SOURCE
#include <hardware/gralloc.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

extern gralloc_module_t HAL_MODULE_INFO_SYM;
#define CHECK(c) do { if (!(c)) { fprintf(stderr,"GRALLOC_FAIL line=%d errno=%d: %s\n",__LINE__,errno,#c);return 2; } } while (0)

static int imported_copy(gralloc_module_t *module,buffer_handle_t buffer) {
    size_t bytes=sizeof(*buffer)+(buffer->numFds+buffer->numInts)*sizeof(int);
    native_handle_t *copy=malloc(bytes);
    CHECK(copy);
    memcpy(copy,buffer,bytes);copy->data[0]=dup(buffer->data[0]);CHECK(copy->data[0]>=0);
    CHECK(module->registerBuffer(module,copy)==0);
    CHECK(module->registerBuffer(module,copy)==-EEXIST);
    void *mapped=NULL;
    CHECK(module->lock(module,copy,GRALLOC_USAGE_SW_READ_OFTEN|GRALLOC_USAGE_SW_WRITE_OFTEN,0,0,17,9,&mapped)==0);
    CHECK(((uint32_t *)mapped)[0]==0x12345678);
    ((uint32_t *)mapped)[0]=0xaabbccdd;
    CHECK(module->unregisterBuffer(module,copy)==-EBUSY);
    int fence=99;CHECK(module->unlockAsync(module,copy,&fence)==0 && fence==-1);
    CHECK(module->unregisterBuffer(module,copy)==0);
    CHECK(module->lock(module,copy,GRALLOC_USAGE_SW_READ_OFTEN,0,0,17,9,&mapped)==-EINVAL);
    copy->data[3]=18;CHECK(module->registerBuffer(module,copy)==0);
    /* Equal-byte layouts can legitimately describe another width. */
    CHECK(module->unregisterBuffer(module,copy)==0);
    copy->data[7]--;CHECK(module->registerBuffer(module,copy)==-EINVAL);
    close(copy->data[0]);free(copy);return 0;
}
static int framebuffer_case(void) {
    gralloc_module_t *module=&HAL_MODULE_INFO_SYM;framebuffer_device_t *fb=NULL;
    for (int i=0;i<100 && !fb;i++) {
        if (framebuffer_open(&module->common,&fb)) {fb=NULL;usleep(100000);}
    }
    CHECK(fb && fb->width && fb->height);
    alloc_device_t *allocator=NULL;CHECK(gralloc_open(&module->common,&allocator)==0);
    buffer_handle_t buffer;int stride;
    CHECK(allocator->alloc(allocator,fb->width,fb->height,HAL_PIXEL_FORMAT_RGBA_8888,
          GRALLOC_USAGE_HW_FB|GRALLOC_USAGE_SW_WRITE_OFTEN,&buffer,&stride)==0);
    void *pixels;CHECK(module->lock(module,buffer,GRALLOC_USAGE_SW_WRITE_OFTEN,0,0,fb->width,fb->height,&pixels)==0);
    for (uint32_t y=0;y<fb->height;y++) for (uint32_t x=0;x<fb->width;x++) {
        uint8_t *p=(uint8_t *)pixels+((size_t)y*stride+x)*4;
        p[0]=x<fb->width/3 ? 240 : 24;p[1]=x>=fb->width/3 && x<fb->width*2/3 ? 220 : 32;
        p[2]=x>=fb->width*2/3 ? 230 : 40;p[3]=255;
        if (y<24 || y+24>=fb->height) {p[0]=32;p[1]=32;p[2]=32;}
    }
    CHECK(module->unlock(module,buffer)==0);
    CHECK(fb->post(fb,buffer)==0);
    /* A standalone SWS may still be retrying its optional stemd notification. */
    puts("SCARLET_ANDROID_FRAMEBUFFER_PRESENTED");fflush(stdout);sleep(15);
    CHECK(allocator->free(allocator,buffer)==0 && gralloc_close(allocator)==0);
    CHECK(framebuffer_close(fb)==0);
    puts("SCARLET_ANDROID_FRAMEBUFFER_OK");return 0;
}
int main(int argc,char **argv) {
    if (argc==2 && !strcmp(argv[1],"--framebuffer")) return framebuffer_case();
    gralloc_module_t *module=&HAL_MODULE_INFO_SYM;
    alloc_device_t *allocator=NULL;
    CHECK(gralloc_open(&module->common,&allocator)==0 && allocator);
    buffer_handle_t buffer=NULL;int stride;
    CHECK(allocator->alloc(allocator,17,9,HAL_PIXEL_FORMAT_RGBA_8888,
          GRALLOC_USAGE_SW_READ_OFTEN|GRALLOC_USAGE_SW_WRITE_OFTEN,&buffer,&stride)==0);
    CHECK(stride==32 && buffer->numFds==1);
    CHECK(ftruncate(buffer->data[0],0)<0 && errno==EPERM);
    CHECK(ftruncate(buffer->data[0],1048576)<0 && errno==EPERM);
    uint32_t fds,ints;
    CHECK(module->getTransportSize(module,buffer,&fds,&ints)==0 && fds==1 && ints==9);
    CHECK(module->validateBufferSize(module,buffer,17,9,HAL_PIXEL_FORMAT_RGBA_8888,0,stride)==0);
    CHECK(module->validateBufferSize(module,buffer,18,9,HAL_PIXEL_FORMAT_RGBA_8888,0,stride)==-EINVAL);
    void *mapped=NULL;
    CHECK(module->lock(module,buffer,GRALLOC_USAGE_SW_WRITE_OFTEN,0,0,17,9,&mapped)==0);
    CHECK(((uint32_t *)mapped)[0]==0);((uint32_t *)mapped)[0]=0x12345678;
    CHECK(allocator->free(allocator,buffer)==-EBUSY);
    CHECK(module->lock(module,buffer,GRALLOC_USAGE_SW_READ_OFTEN,0,0,17,9,&mapped)==-EBUSY);
    CHECK(module->unlock(module,buffer)==0);
    pid_t pid=fork();CHECK(pid>=0);
    if (!pid) _exit(imported_copy(module,buffer));
    int status;CHECK(waitpid(pid,&status,0)==pid && WIFEXITED(status) && WEXITSTATUS(status)==0);
    CHECK(module->lock(module,buffer,GRALLOC_USAGE_SW_READ_OFTEN,0,0,17,9,&mapped)==0);
    CHECK(((uint32_t *)mapped)[0]==0xaabbccdd);
    CHECK(module->unlock(module,buffer)==0);
    CHECK(module->unlock(module,buffer)==-EINVAL);
    CHECK(module->lock(module,buffer,GRALLOC_USAGE_SW_READ_OFTEN,16,0,2,1,&mapped)==-EINVAL);
    CHECK(allocator->free(allocator,buffer)==0);
    CHECK(allocator->alloc(allocator,17,9,HAL_PIXEL_FORMAT_RGBA_8888,GRALLOC_USAGE_PROTECTED,&buffer,&stride)==-ENOTSUP);
    CHECK(allocator->alloc(allocator,4097,9,HAL_PIXEL_FORMAT_RGBA_8888,0,&buffer,&stride)==-EINVAL);
    CHECK(allocator->alloc(allocator,17,9,HAL_PIXEL_FORMAT_YCBCR_420_888,0,&buffer,&stride)==-EINVAL);
    CHECK(gralloc_close(allocator)==0);
    puts("SCARLET_ANDROID_GRALLOC_OK");return 0;
}
