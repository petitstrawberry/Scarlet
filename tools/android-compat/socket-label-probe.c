/* Real endpoint labels, thread/fork/exec lifetime and peer credentials. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>
#define CHECK(x) do {if(!(x)){printf("SCARLET_SOCKET_LABEL_FAIL line=%d errno=%d\n",__LINE__,errno);return 1;}}while(0)
#define INIT "u:r:init:s0"
#define ZYGOTE "u:r:zygote:s0"
static int select_label(const char *label) {int fd=open("/proc/thread-self/attr/sockcreate",O_WRONLY|O_CLOEXEC);if(fd<0)return -1;size_t n=label?strlen(label)+1:0;ssize_t result=write(fd,label?label:"",n);int saved=errno;close(fd);errno=saved;return result==(ssize_t)n?0:-1;}
static int empty_selection(void) {int fd=open("/proc/thread-self/attr/sockcreate",O_RDONLY|O_CLOEXEC);if(fd<0)return 0;char bytes[128];ssize_t n=read(fd,bytes,sizeof(bytes));close(fd);return n==0;}
static int peer_label(int fd,const char *expected) {char bytes[128];socklen_t length=sizeof(bytes);if(getsockopt(fd,SOL_SOCKET,SO_PEERSEC,bytes,&length))return 0;return length==strlen(expected)+1 && !memcmp(bytes,expected,length);}
static void *thread_selection(void *unused) {(void)unused;int pair[2];if(select_label(NULL) || socketpair(AF_UNIX,SOCK_STREAM,0,pair))return (void*)1;int ok=peer_label(pair[0],INIT);close(pair[0]);close(pair[1]);return (void*)(long)!ok;}
int main(int argc,char **argv) {
 if(argc==3 && !strcmp(argv[1],"exec")){CHECK(empty_selection());CHECK(peer_label(atoi(argv[2]),ZYGOTE));return 0;}
 CHECK(empty_selection() && select_label("u:r:servicemanager:s0")<0);
 CHECK(select_label(ZYGOTE)==0);int pair[2];CHECK(socketpair(AF_UNIX,SOCK_STREAM,0,pair)==0 && peer_label(pair[0],ZYGOTE));
 struct ucred credentials;socklen_t length=sizeof(credentials);CHECK(getsockopt(pair[0],SOL_SOCKET,SO_PEERCRED,&credentials,&length)==0 && credentials.pid==getpid() && credentials.uid==getuid());
 char small[2];length=sizeof(small);CHECK(getsockopt(pair[0],SOL_SOCKET,SO_PEERSEC,small,&length)<0 && errno==ERANGE && length==sizeof(ZYGOTE));
 pthread_t worker;CHECK(pthread_create(&worker,NULL,thread_selection,NULL)==0);void *result;CHECK(pthread_join(worker,&result)==0 && !result);
 int another[2];CHECK(socketpair(AF_UNIX,SOCK_STREAM,0,another)==0 && peer_label(another[0],ZYGOTE));close(another[0]);close(another[1]);
 pid_t child=fork();CHECK(child>=0);if(!child){char descriptor[32];snprintf(descriptor,sizeof(descriptor),"%d",pair[0]);execl(argv[0],argv[0],"exec",descriptor,NULL);_exit(4);}int status;pid_t waited;do{waited=waitpid(child,&status,0);}while(waited<0 && errno==EINTR);CHECK(waited==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
 int listener=socket(AF_UNIX,SOCK_STREAM,0);CHECK(listener>=0);CHECK(select_label(NULL)==0 && empty_selection());
 struct sockaddr_un address={.sun_family=AF_UNIX};strcpy(address.sun_path,"/dev/socket/scarlet-label-probe");
 CHECK(bind(listener,(struct sockaddr*)&address,sizeof(address))==0 && listen(listener,1)==0);
 child=fork();CHECK(child>=0);if(!child){int client=socket(AF_UNIX,SOCK_STREAM,0);if(client<0 || connect(client,(struct sockaddr*)&address,sizeof(address)) || !peer_label(client,ZYGOTE))_exit(5);char value='x';if(write(client,&value,1)!=1)_exit(6);close(client);_exit(0);}
 int accepted=accept(listener,NULL,NULL);CHECK(accepted>=0 && peer_label(accepted,INIT));char value;CHECK(read(accepted,&value,1)==1 && value=='x');do{waited=waitpid(child,&status,0);}while(waited<0 && errno==EINTR);CHECK(waited==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
 close(accepted);close(listener);close(pair[0]);close(pair[1]);CHECK(unlink(address.sun_path)==0);
 CHECK(socketpair(AF_UNIX,SOCK_STREAM,0,pair)==0 && peer_label(pair[0],INIT));close(pair[0]);close(pair[1]);
 printf("SCARLET_SOCKET_LABEL_OK authorization endpoint-identity peer-credentials accepted-socket thread fork exec\n");return 0;
}
