#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>
#include "zygote-config.h"
static int listener(const char *role, struct sockaddr_un *address, socklen_t *length) {
    memset(address,0,sizeof(*address));address->sun_family=AF_UNIX;
    int count=snprintf(address->sun_path,sizeof(address->sun_path),"/dev/socket/%s",role);
    *length=offsetof(struct sockaddr_un,sun_path)+count+1;
    int fd=socket(AF_UNIX,SOCK_STREAM,0);
    if(fd<0 || bind(fd,(struct sockaddr*)address,*length)<0 || listen(fd,5)<0) return -1;
    return fd;
}
static int read_all(int fd,void *buffer,size_t length) {
    while(length) {struct pollfd p={fd,POLLIN,0};if(poll(&p,1,20000)<=0)return -1;
        ssize_t n=read(fd,buffer,length);if(n<=0)return -1;buffer=(char*)buffer+n;length-=n;}
    return 0;
}
static int query(struct sockaddr_un *address,socklen_t length,const char *command,char *result,size_t capacity) {
    int fd=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0);int outcome=-1;
    if(fd<0 || connect(fd,(struct sockaddr*)address,length)<0)goto done;
    if(write(fd,command,strlen(command))!=(ssize_t)strlen(command))goto done;
    uint32_t size;if(read_all(fd,&size,4))goto done;size=ntohl(size);
    if(size==0 || size>=capacity || read_all(fd,result,size))goto done;
    result[size]=0;outcome=0;
done: if(fd>=0)close(fd);return outcome;
}
static int check_text(const char *path,const char *expected,size_t length) {
    char bytes[128];int fd=open(path,O_RDONLY|O_CLOEXEC);if(fd<0)return -1;
    ssize_t count=read(fd,bytes,sizeof(bytes));close(fd);
    return count==(ssize_t)length && !memcmp(bytes,expected,length)?0:-1;
}
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);
    struct sockaddr_un server,pool;socklen_t server_size,pool_size;
    int server_fd=listener("zygote",&server,&server_size),pool_fd=listener("usap_pool_primary",&pool,&pool_size);
    if(server_fd<0 || pool_fd<0) {printf("SCARLET_ZYGOTE_PROBE_FAIL sockets errno=%d\n",errno);return 1;}
    pid_t child=fork();if(child<0)return 1;
    if(child==0) {
        char fd[32];snprintf(fd,sizeof(fd),"%d",server_fd);setenv("ANDROID_SOCKET_zygote",fd,1);
        snprintf(fd,sizeof(fd),"%d",pool_fd);setenv("ANDROID_SOCKET_usap_pool_primary",fd,1);
        for(const char **entry=zygote_environment;*entry;entry++) {char *copy=strdup(*entry);char *eq=strchr(copy,'=');*eq=0;setenv(copy,eq+1,1);free(copy);}
        execv(zygote_argv[0],(char *const*)zygote_argv);perror("zygote exec");_exit(1);
    }
    close(server_fd);close(pool_fd);printf("SCARLET_ZYGOTE_PROBE_SPAWN %d\n",child);
    char answer[128];int passed=1;
    if(query(&server,server_size,"1\n--query-abi-list\n",answer,sizeof(answer)) || strcmp(answer,"arm64-v8a")) {
        printf("SCARLET_ZYGOTE_PROBE_FAIL abi errno=%d\n",errno);passed=0;
    } else {
        printf("SCARLET_ZYGOTE_PROBE_ABI_OK %s\n",answer);
        if(query(&server,server_size,"1\n--get-pid\n",answer,sizeof(answer)) || atoi(answer)!=child) {
            printf("SCARLET_ZYGOTE_PROBE_FAIL pid errno=%d\n",errno);passed=0;
        } else printf("SCARLET_ZYGOTE_PROBE_PID_OK %s\n",answer);
    }
    char label_path[64];snprintf(label_path,sizeof(label_path),"/proc/%d/attr/current",child);
    if(check_text("/sys/fs/selinux/enforce","1\n",2) ||
       check_text(label_path,"u:r:zygote:s0",sizeof("u:r:zygote:s0"))) {
        puts("SCARLET_ZYGOTE_PROBE_FAIL shield");passed=0;
    } else puts("SCARLET_ZYGOTE_PROBE_SHIELD_OK");
    int status;
    if(kill(child,SIGKILL) || waitpid(child,&status,0)!=child || !WIFSIGNALED(status) || WTERMSIG(status)!=SIGKILL) {
        puts("SCARLET_ZYGOTE_PROBE_FAIL cleanup");return 1;
    }
    printf("SCARLET_ZYGOTE_PROBE_REAP %d\n",status);
    if(passed)puts("SCARLET_ZYGOTE_PROBE_OK");return passed?0:1;
}
