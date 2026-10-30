/* Real Bionic FIFO dispatch, permission checks and ambient exec delegation. */
#define _GNU_SOURCE
#include <errno.h>
#include <linux/capability.h>
#include <linux/securebits.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHECK(c) do {if (!(c)) {fprintf(stderr,"SCARLET_REALTIME_FAIL line=%d errno=%d: %s\n",__LINE__,errno,#c);return 1;}} while (0)
static int policy(int priority) {
    struct sched_param parameter={.sched_priority=priority};
    return sched_setscheduler(0,priority?SCHED_FIFO:SCHED_OTHER,&parameter);
}
static int reap(pid_t pid) {
    int status;CHECK(waitpid(pid,&status,0)==pid);
    CHECK(WIFEXITED(status) && WEXITSTATUS(status)==0);return 0;
}
static int read_exact(int fd,char *bytes,size_t size) {
    for (size_t offset=0;offset<size;) {ssize_t n=read(fd,bytes+offset,size-offset);CHECK(n>0);offset+=(size_t)n;}
    return 0;
}
int realtime_exec_case(const char *program,const char *mode) {
    CHECK(getuid()==1000 && geteuid()==1000);
    struct __user_cap_header_struct header={_LINUX_CAPABILITY_VERSION_3,0};
    struct __user_cap_data_struct caps[2]={{0}};
    CHECK(syscall(__NR_capget,&header,caps)==0);
    unsigned bit=1u<<CAP_SYS_NICE;
    if (!strcmp(mode,"realtime-stripped")) {
        CHECK(caps[0].effective==0 && caps[0].permitted==0 && caps[0].inheritable==0);
        CHECK(sched_getscheduler(0)==SCHED_OTHER);
        CHECK(policy(2)==-1 && errno==EPERM);
        puts("SCARLET_REALTIME_EXEC_CEILING_OK");return 0;
    }
    CHECK(caps[0].effective==bit && caps[0].permitted==bit && caps[0].inheritable==bit);
    CHECK(caps[1].effective==0 && caps[1].permitted==0 && caps[1].inheritable==0);
    CHECK(prctl(PR_GET_KEEPCAPS,0,0,0,0)==0);
    CHECK(prctl(PR_GET_SECUREBITS,0,0,0,0)==SECBIT_KEEP_CAPS_LOCKED);
    CHECK(prctl(PR_SET_KEEPCAPS,1,0,0,0)==-1 && errno==EPERM);
    CHECK(prctl(PR_CAPBSET_READ,CAP_SYS_NICE,0,0,0)==1);
    CHECK(prctl(PR_CAPBSET_READ,CAP_SETPCAP,0,0,0)==0);
    CHECK(prctl(PR_CAP_AMBIENT,PR_CAP_AMBIENT_IS_SET,CAP_SYS_NICE,0,0)==1);
    CHECK(policy(2)==0 && sched_getscheduler(0)==SCHED_FIFO);
    struct sched_param parameter={0};CHECK(sched_getparam(0,&parameter)==0 && parameter.sched_priority==2);
    CHECK(policy(0)==0);
    caps[0].inheritable=0;CHECK(syscall(__NR_capset,&header,caps)==0);
    CHECK(prctl(PR_CAP_AMBIENT,PR_CAP_AMBIENT_IS_SET,CAP_SYS_NICE,0,0)==0);
    puts("SCARLET_REALTIME_AMBIENT_EXEC_OK");
    execl(program,program,"realtime-stripped",NULL);CHECK(0);
}
static int fifo_order(void) {
    int ready[2],done[2],go[2][2];CHECK(pipe(ready)==0 && pipe(done)==0 && pipe(go[0])==0 && pipe(go[1])==0);
    CHECK(policy(3)==0);
    pid_t children[2];
    for (int index=0;index<2;index++) {
        children[index]=fork();CHECK(children[index]>=0);
        if (!children[index]) {
            if (policy(2) || write(ready[1],"R",1)!=1) _exit(10);
            char start;if (read_exact(go[index][0],&start,1)) _exit(11);
            if (write(done[1],index?"B":"A",1)!=1) _exit(12);
            if (!index && (sched_yield() || write(done[1],"C",1)!=1)) _exit(13);
            _exit(0);
        }
    }
    char bytes[3];CHECK(read_exact(ready[0],bytes,2)==0);
    CHECK(write(go[0][1],"G",1)==1 && write(go[1][1],"G",1)==1);
    // Both FIFO peers are ready before this task returns to Fair. The first
    // writes A, explicitly yields to the second, then resumes to write C.
    CHECK(policy(0)==0 && read_exact(done[0],bytes,3)==0);
    CHECK(!memcmp(bytes,"ABC",3));
    CHECK(reap(children[0])==0 && reap(children[1])==0);
    close(ready[0]);close(ready[1]);close(done[0]);close(done[1]);
    for (int index=0;index<2;index++){close(go[index][0]);close(go[index][1]);}
    puts("SCARLET_REALTIME_FIFO_YIELD_OK");return 0;
}
int realtime_case(const char *program) {
    CHECK(sched_get_priority_min(SCHED_FIFO)==1 && sched_get_priority_max(SCHED_FIFO)==99);
    struct sched_param rr={.sched_priority=2};
    CHECK(sched_setscheduler(0,SCHED_RR,&rr)==-1 && errno==EOPNOTSUPP);
    CHECK(fifo_order()==0);
    pid_t child=fork();CHECK(child>=0);
    if (!child) {
        if (prctl(PR_GET_SECUREBITS,0,0,0,0)!=0 ||
            prctl(PR_SET_SECUREBITS,SECBIT_KEEP_CAPS|SECBIT_KEEP_CAPS_LOCKED,0,0,0) || setgid(1000) || setuid(1000)) _exit(20);
        struct __user_cap_header_struct header={_LINUX_CAPABILITY_VERSION_3,0};
        unsigned bit=1u<<CAP_SYS_NICE;
        struct __user_cap_data_struct caps[2]={{.effective=1u<<CAP_SETPCAP,.permitted=bit|(1u<<CAP_SETPCAP),.inheritable=bit},{0}};
        if (syscall(__NR_capset,&header,caps)) _exit(21);
        for (int cap=0;cap<=40;cap++) {
            if (prctl(PR_CAPBSET_READ,cap,0,0,0)<0 || (cap!=CAP_SYS_NICE && prctl(PR_CAPBSET_DROP,cap,0,0,0))) _exit(23);
        }
        caps[0].effective=0;caps[0].permitted=bit;
        if (syscall(__NR_capset,&header,caps) || prctl(PR_CAP_AMBIENT,PR_CAP_AMBIENT_RAISE,CAP_SYS_NICE,0,0)) _exit(24);
        execl(program,program,"realtime-ambient",NULL);_exit(22);
    }
    CHECK(reap(child)==0);
    puts("SCARLET_ANDROID_REALTIME_OK");return 0;
}
