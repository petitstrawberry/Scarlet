/* Monotonic native disclosure restrictions, including real kernel-log access. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#define NATIVE 0x53430000L
#define CHECK(x) do { if (!(x)) { printf("SCARLET_INFORMATION_FAIL line=%d errno=%d\n",__LINE__,errno); return 1; } } while(0)
int information_exec_case(void) {
    CHECK(syscall(NATIVE + 1319) == 1);
    int fd = open("/proc/sys/kernel/kptr_restrict", O_RDWR); CHECK(fd >= 0);
    char value[2]; CHECK(read(fd,value,2)==2 && !memcmp(value,"2\n",2));
    CHECK(write(fd,"0",1)==-1 && errno==EACCES); CHECK(write(fd,"2",1)==1); CHECK(close(fd)==0);
    printf("SCARLET_INFORMATION_EXEC_OK\n"); return 0;
}
static int child(const char *program) {
    CHECK(syscall(NATIVE + 1319)==0);
    int log = open("/dev/kmsg",O_RDONLY|O_NONBLOCK); CHECK(log>=0);
    CHECK(syscall(NATIVE + 1318,1)==0 && syscall(NATIVE + 1319)==1);
    char byte; CHECK(read(log,&byte,1)==-1 && errno==EACCES); CHECK(close(log)==0);
    CHECK(syscall(NATIVE + 1318,0)==0 && syscall(NATIVE + 1319)==1);
    CHECK(syscall(NATIVE + 1318,2)==-1 && syscall(NATIVE + 1319)==1);
    CHECK(information_exec_case()==0);
    pid_t pid=fork(); CHECK(pid>=0);
    if(!pid){execl(program,program,"information-exec",NULL);_exit(90);}
    int status; CHECK(waitpid(pid,&status,0)==pid && status==0);
    printf("SCARLET_INFORMATION_OK native monotonic kptr kernel-log fork exec\n");return 0;
}
int information_case(const char *program) {
    pid_t pid=fork();CHECK(pid>=0);if(!pid)_exit(child(program));
    int status;CHECK(waitpid(pid,&status,0)==pid && status==0);
    CHECK(syscall(NATIVE + 1319)==0);return 0;
}
