/* Native Unix datagram records exercised through unmodified Bionic. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dirent.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#define CHECK(x) do {if(!(x)){printf("SCARLET_DGRAM_FAIL line=%d errno=%d\n",__LINE__,errno);return 1;}}while(0)
static socklen_t named(struct sockaddr_un *address,const char *path) {memset(address,0,sizeof(*address));address->sun_family=AF_UNIX;strcpy(address->sun_path,path);return (socklen_t)(offsetof(struct sockaddr_un,sun_path)+strlen(path)+1);}
static int descriptors(void) {DIR *dir=opendir("/proc/self/fd");if(!dir)return -1;int n=0;struct dirent *entry;while((entry=readdir(dir)))if(entry->d_name[0]!='.')n++;closedir(dir);return n;}
static void *writable(void *argument) {struct pollfd p={.fd=(int)(long)argument,.events=POLLOUT};return (void*)(long)(poll(&p,1,1000)==1 && (p.revents&POLLOUT)?0:1);}
static int send_rights(int socket,const int *fds,size_t count) {
 char payload='p',control[CMSG_SPACE(sizeof(int)*2)]={0};struct iovec iov={&payload,1};struct msghdr message={.msg_iov=&iov,.msg_iovlen=1,.msg_control=control,.msg_controllen=CMSG_SPACE(count*sizeof(int))};
 struct cmsghdr *header=CMSG_FIRSTHDR(&message);header->cmsg_len=CMSG_LEN(count*sizeof(int));header->cmsg_level=SOL_SOCKET;header->cmsg_type=SCM_RIGHTS;memcpy(CMSG_DATA(header),fds,count*sizeof(int));return (int)sendmsg(socket,&message,MSG_NOSIGNAL);
}
int main(void) {
 signal(SIGPIPE,SIG_IGN);struct sockaddr_un address,source;const char *path="/dev/socket/scarlet-dgram-probe";socklen_t size=named(&address,path);
 int server=socket(AF_UNIX,SOCK_DGRAM|SOCK_NONBLOCK,0),client=socket(AF_UNIX,SOCK_DGRAM|SOCK_NONBLOCK,0);CHECK(server>=0 && client>=0 && bind(server,(struct sockaddr*)&address,size)==0);CHECK(listen(server,1)<0);CHECK(connect(client,(struct sockaddr*)&address,size)==0);
 CHECK(send(client,"first",5,0)==5 && send(client,"two",3,0)==3);char bytes[64]={0};CHECK(recv(server,bytes,2,MSG_PEEK|MSG_TRUNC)==5 && !memcmp(bytes,"fi",2));CHECK(recv(server,bytes,2,MSG_TRUNC)==5 && !memcmp(bytes,"fi",2));CHECK(recv(server,bytes,sizeof(bytes),0)==3 && !memcmp(bytes,"two",3));
 CHECK(send(client,NULL,0,0)==0);struct pollfd p={.fd=server,.events=POLLIN};CHECK(poll(&p,1,0)==1 && recv(server,bytes,sizeof(bytes),0)==0);CHECK(recv(server,bytes,sizeof(bytes),MSG_DONTWAIT)<0 && errno==EAGAIN);
 struct iovec output[2]={{"abc",3},{"def",3}};struct msghdr out={.msg_iov=output,.msg_iovlen=2,.msg_name=&address,.msg_namelen=size};CHECK(sendmsg(client,&out,0)==6);
 int one=1;CHECK(setsockopt(server,SOL_SOCKET,SO_PASSCRED,&one,sizeof(one))==0);char first[2],second[2],control[CMSG_SPACE(sizeof(struct ucred))];struct iovec input[2]={{first,2},{second,2}};struct msghdr in={.msg_iov=input,.msg_iovlen=2,.msg_control=control,.msg_controllen=sizeof(control)};
 CHECK(recvmsg(server,&in,MSG_TRUNC)==6 && (in.msg_flags&MSG_TRUNC) && !memcmp(first,"ab",2) && !memcmp(second,"cd",2));struct cmsghdr *header=CMSG_FIRSTHDR(&in);CHECK(header && header->cmsg_type==SCM_CREDENTIALS);struct ucred credential;memcpy(&credential,CMSG_DATA(header),sizeof(credential));CHECK(credential.pid==getpid() && credential.uid==getuid() && credential.gid==getgid());
 one=0;CHECK(setsockopt(server,SOL_SOCKET,SO_PASSCRED,&one,sizeof(one))==0);
 int pipefd[2];CHECK(pipe2(pipefd,O_CLOEXEC)==0 && send_rights(client,pipefd,2)==1);close(pipefd[0]);close(pipefd[1]);
 char rights[CMSG_SPACE(sizeof(int)*2)];struct iovec payload={bytes,sizeof(bytes)};struct msghdr transfer={.msg_iov=&payload,.msg_iovlen=1,.msg_control=rights,.msg_controllen=sizeof(rights)};
 CHECK(recvmsg(server,&transfer,MSG_PEEK|MSG_CMSG_CLOEXEC)==1);header=CMSG_FIRSTHDR(&transfer);CHECK(header && header->cmsg_type==SCM_RIGHTS);int received[2];memcpy(received,CMSG_DATA(header),sizeof(received));CHECK((fcntl(received[0],F_GETFD)&FD_CLOEXEC) && (fcntl(received[1],F_GETFD)&FD_CLOEXEC));close(received[0]);close(received[1]);
 transfer.msg_controllen=sizeof(rights);CHECK(recvmsg(server,&transfer,0)==1);header=CMSG_FIRSTHDR(&transfer);CHECK(header && header->cmsg_len==CMSG_LEN(sizeof(received)));memcpy(received,CMSG_DATA(header),sizeof(received));CHECK(fcntl(received[0],F_GETFD)==0 && fcntl(received[1],F_GETFD)==0);CHECK(write(received[1],"x",1)==1 && read(received[0],bytes,1)==1 && bytes[0]=='x');close(received[0]);close(received[1]);
 CHECK(pipe(pipefd)==0 && send_rights(client,pipefd,2)==1);close(pipefd[0]);close(pipefd[1]);transfer.msg_controllen=CMSG_LEN(sizeof(int));CHECK(recvmsg(server,&transfer,0)==1 && (transfer.msg_flags&MSG_CTRUNC));header=CMSG_FIRSTHDR(&transfer);CHECK(header);memcpy(received,CMSG_DATA(header),sizeof(int));CHECK(read(received[0],bytes,1)==0);close(received[0]);
 CHECK(pipe(pipefd)==0 && send_rights(client,pipefd,1)==1);close(pipefd[0]);int before=descriptors();transfer.msg_control=(void*)1;transfer.msg_controllen=sizeof(rights);CHECK(recvmsg(server,&transfer,0)<0 && errno==EFAULT && descriptors()==before);close(pipefd[1]);
 int count=0;while(send(client,"f",1,MSG_DONTWAIT)==1) {count++;CHECK(count<1024);}CHECK(errno==EAGAIN && count>0);p.fd=client;p.events=POLLOUT;p.revents=0;CHECK(poll(&p,1,0)==0);
 pthread_t worker;CHECK(pthread_create(&worker,NULL,writable,(void*)(long)client)==0);usleep(10000);CHECK(recv(server,bytes,sizeof(bytes),0)==1);void *result;CHECK(pthread_join(worker,&result)==0 && !result);CHECK(send(client,"f",1,0)==1);close(client);
 for(int i=0;i<count;i++)CHECK(recv(server,bytes,sizeof(bytes),0)==1);CHECK(recv(server,bytes,sizeof(bytes),MSG_DONTWAIT)<0 && errno==EAGAIN);
 client=socket(AF_UNIX,SOCK_DGRAM|SOCK_NONBLOCK,0);socklen_t source_size=named(&source,"/dev/socket/scarlet-dgram-source");CHECK(client>=0 && bind(client,(struct sockaddr*)&source,source_size)==0 && sendto(client,"named",5,0,(struct sockaddr*)&address,size)==5);
 struct sockaddr_un returned={0};socklen_t returned_size=sizeof(returned);CHECK(recvfrom(server,bytes,sizeof(bytes),0,(struct sockaddr*)&returned,&returned_size)==5 && returned_size==source_size && !strcmp(returned.sun_path,source.sun_path));
 CHECK(connect(client,(struct sockaddr*)&address,size)==0);struct sockaddr unspecified={.sa_family=AF_UNSPEC};CHECK(connect(client,&unspecified,sizeof(sa_family_t))==0);CHECK(send(client,"x",1,0)<0 && errno==EDESTADDRREQ);
 CHECK(connect(client,(struct sockaddr*)&address,size)==0);char *large=malloc(65537);CHECK(large && send(client,large,65537,0)<0 && errno==EMSGSIZE);free(large);
 CHECK(pipe(pipefd)==0 && send_rights(client,pipefd,1)==1);close(pipefd[0]);close(server);CHECK(write(pipefd[1],"x",1)<0 && errno==EPIPE);close(pipefd[1]);CHECK(send(client,"x",1,0)<0 && errno==ECONNREFUSED);close(client);CHECK(unlink(path)==0 && unlink(source.sun_path)==0);
 printf("SCARLET_DGRAM_OK named-routing message-boundaries zero-length peek truncation credentials rights rollback capacity poll close disconnect\n");return 0;
}
