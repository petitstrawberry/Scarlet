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
#ifdef SCARLET_ZYGOTE_CHILD
static int run_child(struct sockaddr_un *server,socklen_t server_size,int results) {
    const char *request="12\n--runtime-args\n--setuid=10001\n--setgid=10001\n--setgroups=3003\n--runtime-flags=0\n--target-sdk-version=32\n--seinfo=default:targetSdkVersion=32\n--nice-name=scarlet-zygote-child\n--instruction-set=arm64\n--app-data-dir=/data/local/tmp\n--package-name=org.scarlet.zygote\nScarletZygoteChild\n";
    int control=-1, peer=-1, outcome=-1;pid_t child=0;
    unsigned char response[5];uint32_t number;
    control=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0);
    if(results<0 || control<0 || connect(control,(struct sockaddr*)server,server_size) ||
       write(control,request,strlen(request))!=(ssize_t)strlen(request) || read_all(control,response,5)) goto done;
    memcpy(&number,response,4);child=(pid_t)ntohl(number);
    if(child<=0 || response[4]!=0) goto done;
    struct pollfd ready={results,POLLIN,0};if(poll(&ready,1,20000)<=0)goto done;
    peer=accept4(results,NULL,NULL,SOCK_CLOEXEC);if(peer<0)goto done;
    struct ucred credentials;socklen_t size=sizeof(credentials);
    if(getsockopt(peer,SOL_SOCKET,SO_PEERCRED,&credentials,&size) || size!=sizeof(credentials) ||
       credentials.pid!=child || credentials.uid!=10001 || credentials.gid!=10001)goto done;
    uint32_t proof[4];char label[128],path[64];
    if(read_all(peer,proof,sizeof(proof)))goto done;
    for(int i=0;i<4;i++)proof[i]=ntohl(proof[i]);
    if(proof[0]!=(uint32_t)child || proof[1]!=10001 || proof[2]!=10001 || proof[3]>=sizeof(label))goto done;
    if(read_all(peer,label,proof[3]))goto done;label[proof[3]]=0;
    if(strcmp(label,"u:r:untrusted_app:s0:c1,c256,c512,c768"))goto done;
    snprintf(path,sizeof(path),"/proc/%d/attr/current",child);
    if(check_text(path,label,strlen(label)+1))goto done;
    printf("SCARLET_ZYGOTE_PROBE_CHILD_IDENTITY_OK %d 10001 10001\n",child);
    printf("SCARLET_ZYGOTE_PROBE_CHILD_LABEL_OK %s\n",label);
    const unsigned char ack=1;if(write(peer,&ack,1)!=1)goto done;
    ready=(struct pollfd){peer,POLLIN,0};if(poll(&ready,1,20000)<=0)goto done;
    unsigned char extra;if(read(peer,&extra,1)!=0)goto done;
    puts("SCARLET_ZYGOTE_PROBE_CHILD_EOF");outcome=0;
done:
    if(peer>=0)close(peer);if(control>=0)close(control);
    if(outcome && child>0)kill(child,SIGKILL);
    return outcome;
}
#endif
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
#ifdef SCARLET_ZYGOTE_CHILD
    // Create after fork: this supervisor endpoint must not enter Zygote's FD table.
    struct sockaddr_un result_address;socklen_t result_size;
    int result_fd=listener("scarlet_zygote_result",&result_address,&result_size);
#endif
    if(query(&server,server_size,"1\n--query-abi-list\n",answer,sizeof(answer)) || strcmp(answer,"arm64-v8a")) {
        printf("SCARLET_ZYGOTE_PROBE_FAIL abi errno=%d\n",errno);passed=0;
    } else {
        printf("SCARLET_ZYGOTE_PROBE_ABI_OK %s\n",answer);
        if(query(&server,server_size,"1\n--get-pid\n",answer,sizeof(answer)) || atoi(answer)!=child) {
            printf("SCARLET_ZYGOTE_PROBE_FAIL pid errno=%d\n",errno);passed=0;
        } else printf("SCARLET_ZYGOTE_PROBE_PID_OK %s\n",answer);
    }
#ifdef SCARLET_ZYGOTE_CHILD
    if(!passed || run_child(&server,server_size,result_fd)) {
        printf("SCARLET_ZYGOTE_PROBE_FAIL child errno=%d\n",errno);passed=0;
    }
    if(result_fd>=0)close(result_fd);
#endif
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
