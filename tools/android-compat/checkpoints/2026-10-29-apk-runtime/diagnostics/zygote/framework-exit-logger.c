#define _GNU_SOURCE
#include <android/log.h>
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <unistd.h>
static void logger(const struct __android_log_message *m) {
    dprintf(2,"ANDROID_LOG %d %s: %s\n",m->priority,m->tag?m->tag:"",m->message?m->message:"");
}
void __android_log_set_logger(__android_logger_function sink) {
    (void)sink;
    void (*setter)(__android_logger_function)=dlsym(RTLD_NEXT,"__android_log_set_logger");
    if (setter) setter(logger);
}
void __android_log_logd_logger(const struct __android_log_message *m) {logger(m);}
__attribute__((constructor)) static void install(void) {__android_log_set_logger(logger);}
__attribute__((noreturn)) void _exit(int status) {
    void *caller=__builtin_return_address(0);Dl_info where={0};dladdr(caller,&where);
    dprintf(2,"DIAGNOSTIC__EXIT status=%d caller=%p module=%s offset=%lx\n",status,caller,
        where.dli_fname?where.dli_fname:"?",(unsigned long)caller-(unsigned long)where.dli_fbase);
    syscall(__NR_exit_group,status);__builtin_unreachable();
}
__attribute__((noreturn)) void exit(int status) {
    void *caller=__builtin_return_address(0);Dl_info where={0};dladdr(caller,&where);
    dprintf(2,"DIAGNOSTIC_EXIT status=%d caller=%p module=%s offset=%lx\n",status,caller,
        where.dli_fname?where.dli_fname:"?",(unsigned long)caller-(unsigned long)where.dli_fbase);
    void (*real)(int)=dlsym(RTLD_NEXT,"exit");
    if(real)real(status);
    syscall(__NR_exit_group,status);__builtin_unreachable();
}

#include <stdarg.h>
#include <errno.h>
#include <sys/ioctl.h>
int ioctl(int fd, int request, ...) {
    va_list ap; va_start(ap,request); void *arg=va_arg(ap,void *); va_end(ap);
    int (*real)(int,int,...)=dlsym(RTLD_NEXT,"ioctl");
    int result=real(fd,request,arg), saved=errno;
    if (((unsigned)request>>8&255)==98) {
        if ((unsigned)request==0xc0306201 && arg) {
            unsigned long *b=arg;
            if (result < 0 && b[0] >= 68) {
                unsigned *c=(unsigned *)b[2];
                dprintf(2,"BINDER_USER_FAILED pid=%d command=%x flags=%x\n",getpid(),c[0],c[6]);
            }
            dprintf(2,"BINDER_USER pid=%d tid=%d fd=%d req=%x result=%d errno=%d write=%lu/%lu read=%lu/%lu\n",getpid(),gettid(),fd,request,result,saved,b[1],b[0],b[4],b[3]);
        } else dprintf(2,"BINDER_USER pid=%d tid=%d fd=%d req=%x result=%d errno=%d\n",getpid(),gettid(),fd,request,result,saved);
    }
    errno=saved; return result;
}
