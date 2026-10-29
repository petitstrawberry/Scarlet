/* Real Bionic identity transitions, exec inheritance and socket snapshots. */
#define _GNU_SOURCE
#include <errno.h>
#include <grp.h>
#include <linux/capability.h>
#include <linux/seccomp.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/mount.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#define CHECK(x) do { if (!(x)) { printf("SCARLET_BIONIC_FAIL libc: credentials line=%d %s errno=%d\n", __LINE__, #x, errno); return 1; } } while (0)

int credentials_exec_case(void) {
    uid_t r,e,s; gid_t gr,ge,gs,groups[2];
    CHECK(getresuid(&r,&e,&s)==0 && r==10001 && e==r && s==r);
    CHECK(getresgid(&gr,&ge,&gs)==0 && gr==2000 && ge==gr && gs==gr);
    CHECK(getauxval(AT_UID)==r && getauxval(AT_EUID)==e && getauxval(AT_GID)==gr && getauxval(AT_EGID)==ge);
    CHECK(getgroups(2,groups)==2 && groups[0]==3 && groups[1]==7);
    CHECK(prctl(PR_GET_KEEPCAPS,0,0,0,0)==0);
    CHECK(setuid(0)==-1 && errno==EPERM);
    struct __user_cap_header_struct h={_LINUX_CAPABILITY_VERSION_3,0};
    struct __user_cap_data_struct d[2]={{0}};
    CHECK(syscall(__NR_capget,&h,d)==0 && d[0].effective==0 && d[0].permitted==0 && d[1].permitted==0);
    return 0;
}
static int child_identity(const char *program, int socket) {
    gid_t groups[]={7,3}, actual[2]; uid_t r,e,s;
    CHECK(syscall(__NR_getresuid,0,&e,&s)==-1 && errno==EFAULT);
    CHECK(setgroups(2,groups)==0 && getgroups(2,actual)==2 && actual[0]==3 && actual[1]==7);
    CHECK(getgroups(1,actual)==-1 && errno==EINVAL);
    CHECK(syscall(__NR_setgroups,2,1)==-1 && errno==EFAULT);
    CHECK(getgroups(2,actual)==2 && actual[0]==3 && actual[1]==7);
    CHECK(setresgid(2000,2000,2000)==0);
    CHECK(seteuid(10001)==0 && getuid()==0 && geteuid()==10001);
    CHECK(seteuid(0)==0 && geteuid()==0);
    CHECK(setuid(10001)==0 && getresuid(&r,&e,&s)==0 && r==10001 && e==r && s==r);
    CHECK(setuid(0)==-1 && errno==EPERM);
    CHECK(setgid(0)==-1 && errno==EPERM);
    CHECK(setgroups(0,NULL)==-1 && errno==EPERM);
    CHECK(unshare(CLONE_NEWNS)==-1 && errno==EPERM);
    CHECK(mount("rootfs","/",NULL,MS_PRIVATE,NULL)==-1 && errno==EPERM);
    struct ucred peer; socklen_t size=sizeof(peer);
    CHECK(getsockopt(socket,SOL_SOCKET,SO_PEERCRED,&peer,&size)==0 && peer.uid==0 && peer.gid==0);
    // Android libbase sends a valid empty SCM_RIGHTS even with no descriptors.
    union { struct cmsghdr align; unsigned char data[CMSG_SPACE(0)]; } control={0};
    char byte='I'; struct iovec iov={&byte,1};
    struct msghdr msg={.msg_iov=&iov,.msg_iovlen=1,.msg_control=control.data,.msg_controllen=sizeof(control)};
    struct cmsghdr *c=CMSG_FIRSTHDR(&msg);c->cmsg_level=SOL_SOCKET;c->cmsg_type=SCM_RIGHTS;c->cmsg_len=CMSG_LEN(0);
    CHECK(sendmsg(socket,&msg,MSG_NOSIGNAL)==1);
    struct stat inherited, a, b, separate;
    CHECK(fstat(socket,&inherited)==0 && S_ISSOCK(inherited.st_mode) && inherited.st_uid==0 && inherited.st_gid==0);
    int pair[2];CHECK(socketpair(AF_UNIX,SOCK_STREAM,0,pair)==0);
    int copy=dup(pair[0]);CHECK(copy>=0);
    CHECK(fstat(pair[0],&a)==0 && fstat(copy,&b)==0 && fstat(pair[1],&separate)==0);
    CHECK(S_ISSOCK(a.st_mode) && a.st_uid==10001 && a.st_gid==2000);
    CHECK(a.st_dev==b.st_dev && a.st_ino==b.st_ino && a.st_ino!=separate.st_ino);
    char path[64],target[64];snprintf(path,sizeof(path),"/proc/self/fd/%d",copy);
    ssize_t n=readlink(path,target,sizeof(target)-1);CHECK(n>0);target[n]=0;
    snprintf(path,sizeof(path),"socket:[%llu]",(unsigned long long)a.st_ino);CHECK(!strcmp(path,target));
    close(pair[0]);close(pair[1]);close(copy);close(socket);
    CHECK(prctl(PR_SET_KEEPCAPS,1,0,0,0)==0);
    execl(program,program,"identity-exec",NULL);
    CHECK(0);
}
static int drop_capabilities(void) {
    struct __user_cap_header_struct h={_LINUX_CAPABILITY_VERSION_3,0};
    struct __user_cap_data_struct d[2]={{0}};
    CHECK(prctl(PR_SET_KEEPCAPS,1,0,0,0)==0);
    CHECK(setresuid(10002,10002,10002)==0);
    CHECK(syscall(__NR_capget,&h,d)==0 && d[0].effective==0 && (d[0].permitted&(1u<<CAP_SETGID)));
    d[0].effective=d[0].permitted=1u<<CAP_SETGID;
    CHECK(syscall(__NR_capset,&h,d)==0 && setgid(777)==0 && getgid()==777);
    memset(d,0,sizeof(d));CHECK(syscall(__NR_capset,&h,d)==0);
    CHECK(setgid(0)==-1 && errno==EPERM);
    d[0].effective=d[0].permitted=1u<<CAP_SETGID;
    CHECK(syscall(__NR_capset,&h,d)==-1 && errno==EPERM);
    return 0;
}
int credentials_case(const char *program) {
    CHECK(prctl(PR_GET_DUMPABLE,0,0,0,0)==0);
    CHECK(prctl(PR_SET_DUMPABLE,0,0,0,0)==0);
    CHECK(prctl(PR_SET_DUMPABLE,1,0,0,0)==-1 && errno==EOPNOTSUPP);
    CHECK(prctl(0x7fffffff,0,0,0,0)==-1 && errno==EINVAL);
    CHECK(prctl(PR_CAPBSET_READ,64,0,0,0)==-1 && errno==EINVAL);
    CHECK(prctl(PR_SET_NO_NEW_PRIVS,0,0,0,0)==-1 && errno==EINVAL);
    CHECK(prctl(PR_SET_SECCOMP,99,0,0,0)==-1 && errno==EINVAL);
    int sockets[2], enabled=1, status;
    CHECK(socketpair(AF_UNIX,SOCK_STREAM,0,sockets)==0);
    CHECK(setsockopt(sockets[0],SOL_SOCKET,SO_PASSCRED,&enabled,sizeof(enabled))==0);
    pid_t child=fork();CHECK(child>=0);
    if(!child) {close(sockets[0]);_exit(child_identity(program,sockets[1]));}
    close(sockets[1]);CHECK(waitpid(child,&status,0)==child && status==0);
    // Receive after the sender has exited: credentials must come from the
    // queued snapshot, not a task lookup or the socket's creator.
    union { struct cmsghdr align; unsigned char data[CMSG_SPACE(sizeof(struct ucred))]; } control={0};
    char byte=0;struct iovec iov={&byte,1};
    struct msghdr msg={.msg_iov=&iov,.msg_iovlen=1,.msg_control=control.data,.msg_controllen=sizeof(control)};
    CHECK(recvmsg(sockets[0],&msg,0)==1 && byte=='I');
    struct cmsghdr *c=CMSG_FIRSTHDR(&msg);
    CHECK(c && c->cmsg_type==SCM_CREDENTIALS && c->cmsg_level==SOL_SOCKET);
    struct ucred cred;memcpy(&cred,CMSG_DATA(c),sizeof(cred));
    CHECK(cred.pid==child && cred.uid==10001 && cred.gid==2000);
    close(sockets[0]);CHECK(getuid()==0 && getgid()==0);
    child=fork();CHECK(child>=0);
    if(!child)_exit(drop_capabilities());
    CHECK(waitpid(child,&status,0)==child && status==0);
    child=fork();CHECK(child>=0);
    if(!child) {
        struct __user_cap_header_struct h={_LINUX_CAPABILITY_VERSION_3,0};
        struct __user_cap_data_struct d[2]={{0}};
        _exit(syscall(__NR_capset,&h,d)==0 && setuid(1)==-1 && errno==EPERM ? 0:1);
    }
    CHECK(waitpid(child,&status,0)==child && status==0);
    puts("SCARLET_IDENTITY_OK saved-id groups authority-drop fork exec auxv peercred queued-credentials");
    return 0;
}
