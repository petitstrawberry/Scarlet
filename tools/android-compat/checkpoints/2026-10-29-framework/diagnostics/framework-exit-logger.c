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
