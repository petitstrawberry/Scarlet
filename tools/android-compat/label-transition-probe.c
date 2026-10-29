// Real stock libselinux setcon over native, principal-bound Shield transitions.
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
static int (*getcon_fn)(char **), (*setcon_fn)(const char *);
static void (*freecon_fn)(char *);
static const char *labels[]={"u:r:untrusted_app:s0:c1,c256,c512,c768","u:r:untrusted_app:s0:c2,c256,c512,c768"};
#define CHECK(x) do { if (!(x)) { dprintf(2,"SCARLET_LABEL_TRANSITION_FAIL line=%d %s errno=%d\n",__LINE__,#x,errno); return 1; } } while(0)
static int context_is(const char *expected) {
    char *value=NULL;int rc=getcon_fn(&value);
    int matches=rc==0 && value && !strcmp(value,expected);
    if(value)freecon_fn(value);return matches;
}
static int child(int index,int inherited,const char *program) {
    unsigned uid=10001+index;const char *target=labels[index];
    CHECK(setresgid(uid,uid,uid)==0 && setresuid(uid,uid,uid)==0);
    CHECK(write(inherited,target,strlen(target)+1)==-1 && errno==EACCES);
    CHECK(context_is("u:r:zygote:s0"));
    CHECK(setcon_fn(labels[1-index])==-1);
    CHECK(context_is("u:r:zygote:s0"));
    CHECK(setcon_fn(target)==0 && context_is(target));
    CHECK(setcon_fn("u:r:zygote:s0")==-1 && context_is(target));
    pid_t pid=fork();CHECK(pid>=0);
    if(!pid) {execl(program,program,index?"exec-10002":"exec-10001",NULL);_exit(90);}
    int status;CHECK(waitpid(pid,&status,0)==pid && WIFEXITED(status) && WEXITSTATUS(status)==0);
    return 0;
}
int main(int argc,char **argv) {
    void *library=dlopen("/system/lib64/libselinux.so",RTLD_NOW|RTLD_LOCAL);CHECK(library);
    getcon_fn=dlsym(library,"getcon");setcon_fn=dlsym(library,"setcon");freecon_fn=dlsym(library,"freecon");
    int (*enabled)(void)=dlsym(library,"is_selinux_enabled");
    int (*enforcing)(void)=dlsym(library,"security_getenforce");
    CHECK(getcon_fn && setcon_fn && freecon_fn && enabled && enforcing);
    Dl_info info={0};CHECK(dladdr((void *)setcon_fn,&info) && !strcmp(info.dli_fname,"/system/lib64/libselinux.so"));
    CHECK(enabled()==1 && enforcing()==1);
    if(argc==2) {
        int index=!strcmp(argv[1],"exec-10002")?1:0;
        CHECK(getuid()==(unsigned)(10001+index) && context_is(labels[index]));
        dprintf(1,"SCARLET_LABEL_TRANSITION_EXEC_OK %d\n",10001+index);return 0;
    }
    dprintf(1,"SCARLET_LABEL_TRANSITION_BEGIN\n");
    CHECK(context_is("u:r:zygote:s0"));
    CHECK(setcon_fn(labels[0])==-1 && context_is("u:r:zygote:s0"));
    CHECK(setcon_fn("u:r:unregistered:s0")==-1 && context_is("u:r:zygote:s0"));
    dprintf(1,"SCARLET_LABEL_TRANSITION_PARENT_DENIED\n");
    int inherited=open("/proc/self/attr/current",O_WRONLY);CHECK(inherited>=0);
    for(int index=0;index<2;index++) {
        pid_t pid=fork();CHECK(pid>=0);
        if(!pid)_exit(child(index,inherited,argv[0]));
        int status;CHECK(waitpid(pid,&status,0)==pid && WIFEXITED(status) && WEXITSTATUS(status)==0);
        CHECK(context_is("u:r:zygote:s0"));
        dprintf(1,"SCARLET_LABEL_TRANSITION_CHILD_OK %d\n",10001+index);
    }
    CHECK(close(inherited)==0);
    dprintf(1,"SCARLET_LABEL_TRANSITION_OK\n");return 0;
}
