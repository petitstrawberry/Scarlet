// Exercise unmodified Android libselinux against Shield's Android translation.
// Public function signatures follow AOSP Android 13 libselinux/include/selinux.
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/statfs.h>
#include <sys/wait.h>
#include <unistd.h>

static const char *subject = "u:r:scarlet_probe:s0";
static const char *denied = "u:r:scarlet_denied:s0";
static const char *target = "u:object_r:scarlet_service:s0";
#define CHECK(x) do { if (!(x)) { dprintf(2,"SCARLET_SHIELD_FAIL line=%d %s errno=%d\n",__LINE__,#x,errno); return 1; } } while (0)

int main(int argc,char **argv) {
    void *library=dlopen("/system/lib64/libselinux.so",RTLD_NOW|RTLD_LOCAL);
    CHECK(library);
    int (*enabled)(void)=dlsym(library,"is_selinux_enabled");
    int (*enforcing)(void)=dlsym(library,"security_getenforce");
    int (*getcon_fn)(char **)=dlsym(library,"getcon");
    void (*freecon_fn)(char *)=dlsym(library,"freecon");
    int (*status_open)(int)=dlsym(library,"selinux_status_open");
    int (*check)(const char *,const char *,const char *,const char *,void *)=dlsym(library,"selinux_check_access");
    CHECK(enabled && enforcing && getcon_fn && freecon_fn && status_open && check);
    Dl_info info={0};
    CHECK(dladdr((void *)check,&info) && strcmp(info.dli_fname,"/system/lib64/libselinux.so")==0);
    char *context=NULL;
    CHECK(getcon_fn(&context)==0 && context && strcmp(context,subject)==0);
    freecon_fn(context);
    if (argc==2 && strcmp(argv[1],"exec")==0) {
        dprintf(1,"SCARLET_SHIELD_EXEC_LABEL_OK\n");
        return 0;
    }
    dprintf(1,"SCARLET_SHIELD_BEGIN\n");
    dprintf(1,"SCARLET_SHIELD_RUNTIME /system/lib64/libselinux.so\n");
    CHECK(enabled()==1 && enforcing()==1 && status_open(0)==0);
    dprintf(1,"SCARLET_SHIELD_STATUS_OK\n");
    for (int repeat=0; repeat<3; ++repeat) {
        CHECK(check(subject,target,"service_manager","add",NULL)==0);
        CHECK(check(subject,target,"service_manager","find",NULL)==0);
        CHECK(check(subject,subject,"service_manager","list",NULL)==0);
        errno=0;
        CHECK(check(denied,target,"service_manager","add",NULL)==-1 && errno==EACCES);
        errno=0;
        CHECK(check(denied,target,"service_manager","find",NULL)==-1 && errno==EACCES);
        errno=0;
        CHECK(check(subject,target,"service_manager","list",NULL)==-1 && errno==EACCES);
    }
    dprintf(1,"SCARLET_SHIELD_ALLOW_DENY_OK\n");
    CHECK(check(subject,target,"service_manager","unknown",NULL)==-1);
    CHECK(check(subject,target,"unknown_class","add",NULL)==-1);
    CHECK(check("u:r:unknown:s0",target,"service_manager","add",NULL)==-1);
    dprintf(1,"SCARLET_SHIELD_UNKNOWN_DENIED\n");
    CHECK(open("/proc/self/attr/current",O_WRONLY|O_CLOEXEC)==-1);
    CHECK(open("/sys/fs/selinux/enforce",O_WRONLY|O_CLOEXEC)==-1);
    CHECK(open("/sys/fs/selinux/load",O_WRONLY|O_CLOEXEC)==-1);
    int fd=open("/sys/fs/selinux/status",O_RDONLY|O_CLOEXEC);
    CHECK(fd>=0);
    CHECK(mmap(NULL,4096,PROT_READ|PROT_WRITE,MAP_SHARED,fd,0)==MAP_FAILED);
    unsigned *status=mmap(NULL,4096,PROT_READ,MAP_SHARED,fd,0);
    CHECK(status!=MAP_FAILED && status[0]==1 && status[2]==1 && status[4]==1);
    CHECK(mprotect(status,4096,PROT_READ|PROT_WRITE)==-1);
    CHECK(munmap(status,4096)==0 && close(fd)==0);
    CHECK(enforcing()==1);
    dprintf(1,"SCARLET_SHIELD_MUTATION_DENIED\n");
    pid_t child=fork();
    CHECK(child>=0);
    if (!child) {
        char *child_context=NULL;
        if (getcon_fn(&child_context)!=0 || !child_context || strcmp(child_context,subject)) _exit(2);
        freecon_fn(child_context);
        execl("/system/bin/scarlet-program-probe","/system/bin/scarlet-program-probe","exec",NULL);
        _exit(3);
    }
    int result=0;pid_t waited;
    do { waited=waitpid(child,&result,0); } while (waited<0 && errno==EINTR);
    CHECK(waited==child && WIFEXITED(result) && WEXITSTATUS(result)==0);
    dprintf(1,"SCARLET_SHIELD_FORK_EXEC_OK\n");
    dprintf(1,"SCARLET_SHIELD_OK\n");
    return 0;
}
