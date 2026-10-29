#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <pthread.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#include <ucontext.h>
#define CHECK(x) do { if (!(x)) { printf("SCARLET_SECCOMP_FAIL line=%d %s errno=%d\n",__LINE__,#x,errno); return 1; } } while(0)
static int filter(unsigned nr, unsigned action) {
    struct sock_filter instructions[] = {
        BPF_STMT(BPF_LD|BPF_W|BPF_ABS,offsetof(struct seccomp_data,arch)),
        BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,AUDIT_ARCH_AARCH64,1,0),
        BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_KILL_PROCESS),
        BPF_STMT(BPF_LD|BPF_W|BPF_ABS,offsetof(struct seccomp_data,nr)),
        BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,nr,0,1),
        BPF_STMT(BPF_RET|BPF_K,action),
        BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program={.len=sizeof(instructions)/sizeof(instructions[0]),.filter=instructions};
    int result=prctl(PR_SET_SECCOMP,SECCOMP_MODE_FILTER,&program,0,0);
    // A stored user pointer would turn a denial into ALLOW (or freed stack data).
    ((volatile struct sock_filter *)instructions)[5].k=SECCOMP_RET_ALLOW;
    return result;
}
int seccomp_exec_case(void) {
    CHECK(prctl(PR_GET_NO_NEW_PRIVS,0,0,0,0)==1);
    CHECK(prctl(PR_GET_SECCOMP,0,0,0,0)==SECCOMP_MODE_FILTER);
    CHECK(syscall(__NR_getuid)==-1 && errno==EACCES);
    return 0;
}
static int identity_child(const char *program) {
    CHECK(setresuid(10001,10001,10001)==0);
    CHECK(filter(__NR_getuid,SECCOMP_RET_ERRNO|EACCES)==-1 && errno==EACCES);
    CHECK(prctl(PR_GET_SECCOMP,0,0,0,0)==0);
    CHECK(prctl(PR_SET_NO_NEW_PRIVS,1,0,0,0)==0);
    CHECK(prctl(PR_SET_NO_NEW_PRIVS,0,0,0,0)==-1 && errno==EINVAL);
    CHECK(prctl(PR_SET_NO_NEW_PRIVS,1,1,0,0)==-1 && errno==EINVAL);
    CHECK(prctl(PR_SET_SECCOMP,SECCOMP_MODE_FILTER,NULL,0,0)==-1 && errno==EFAULT);
    struct sock_filter bad[]={BPF_STMT(BPF_JMP|BPF_JA,UINT32_MAX),BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_ALLOW)};
    struct sock_fprog p={.len=2,.filter=bad};
    CHECK(syscall(__NR_seccomp,SECCOMP_SET_MODE_FILTER,0,&p)==-1 && errno==EINVAL);
    CHECK(prctl(PR_GET_SECCOMP,0,0,0,0)==0);
    CHECK(syscall(__NR_seccomp,SECCOMP_SET_MODE_FILTER,1,&p)==-1 && errno==EINVAL);
    unsigned action=SECCOMP_RET_TRAP;
    CHECK(syscall(__NR_seccomp,SECCOMP_GET_ACTION_AVAIL,0,&action)==0);
    action=0x12340000;
    CHECK(syscall(__NR_seccomp,SECCOMP_GET_ACTION_AVAIL,0,&action)==-1 && errno==EOPNOTSUPP);
    CHECK(filter(__NR_getuid,SECCOMP_RET_ERRNO|EACCES)==0);
    CHECK(syscall(__NR_getuid)==-1 && errno==EACCES);
    CHECK(syscall(__NR_getgid)>=0);
    pid_t pid=fork(); CHECK(pid>=0);
    if (!pid) { execl(program,program,"seccomp-exec",NULL); _exit(80); }
    int status; CHECK(waitpid(pid,&status,0)==pid && WIFEXITED(status) && WEXITSTATUS(status)==0);
    // A later ALLOW cannot weaken the first program. Equal-ranked newest data wins.
    CHECK(filter(__NR_getuid,SECCOMP_RET_ALLOW)==0);
    CHECK(syscall(__NR_getuid)==-1 && errno==EACCES);
    CHECK(filter(__NR_getuid,SECCOMP_RET_ERRNO|EPERM)==0);
    CHECK(syscall(__NR_getuid)==-1 && errno==EPERM);
    return 0;
}
static volatile sig_atomic_t trapped;
static void trap_handler(int sig,siginfo_t *info,void *context) {
    if (sig!=SIGSYS || info->si_code!=SYS_SECCOMP || info->si_errno!=37 ||
        info->si_syscall!=__NR_getppid || info->si_arch!=AUDIT_ARCH_AARCH64 || !info->si_call_addr ||
        (uintptr_t)info->si_call_addr!=((ucontext_t *)context)->uc_mcontext.pc) _exit(81);
    ((ucontext_t *)context)->uc_mcontext.regs[0]=777;
    trapped++;
}
static int trap_child(void) {
    struct sigaction action={.sa_sigaction=trap_handler,.sa_flags=SA_SIGINFO};
    CHECK(sigemptyset(&action.sa_mask)==0 && sigaction(SIGSYS,&action,NULL)==0);
    CHECK(filter(__NR_getppid,SECCOMP_RET_TRAP|37)==0);
    CHECK(syscall(__NR_getppid)==777 && trapped==1);
    return 0;
}
static int side_effect_child(void) {
    int fd[2];CHECK(pipe2(fd,O_NONBLOCK)==0);
    CHECK(filter(__NR_write,SECCOMP_RET_ERRNO|EACCES)==0);
    // Do not print diagnostics after write is denied; exit status is checked by parent.
    if (write(fd[1],"x",1)!=-1 || errno!=EACCES) return 82;
    char c;if (read(fd[0],&c,1)!=-1 || errno!=EAGAIN) return 83;
    return 0;
}
static void *killed_worker(void *unused) {
    (void)unused;
    if (filter(__NR_getppid,SECCOMP_RET_KILL_THREAD)) return (void *)1;
    syscall(__NR_getppid);
    return (void *)2;
}
static int thread_child(void) {
    pthread_t thread; CHECK(pthread_create(&thread,NULL,killed_worker,NULL)==0);
    CHECK(pthread_join(thread,NULL)==0);
    CHECK(syscall(__NR_getppid)>0); // Filters are per task without TSYNC.
    return 0;
}
int seccomp_case(const char *program) {
    for (int test=0;test<7;test++) {
        pid_t pid=fork();CHECK(pid>=0);
        if (!pid) {
            switch(test) {
                case 0:_exit(identity_child(program));
                case 1:_exit(trap_child());
                case 2:_exit(side_effect_child());
                case 3:_exit(thread_child());
                case 4:
                    if(filter(__NR_getppid,SECCOMP_RET_KILL_PROCESS))_exit(84);
                    syscall(__NR_getppid);_exit(85);
                case 5:
                    if(filter(__NR_getppid,0x12340000))_exit(86);
                    syscall(__NR_getppid);_exit(87);
                default:
                    if(filter(__NR_getppid,SECCOMP_RET_ALLOW))_exit(88);
                    syscall(0x53430007);_exit(89); // Native Getpid escape must be fatal.
            }
        }
        int status;CHECK(waitpid(pid,&status,0)==pid);
        if (test<4) { CHECK(WIFEXITED(status) && WEXITSTATUS(status)==0); }
        else { CHECK(WIFSIGNALED(status) && WTERMSIG(status)==SIGSYS); }
    }
    puts("SCARLET_SECCOMP_OK verify no-side-effect trap kill thread fork exec stacking native-escape");
    return 0;
}
