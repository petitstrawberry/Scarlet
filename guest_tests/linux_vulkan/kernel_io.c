#define _GNU_SOURCE
#include <unistd.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <sys/un.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sched.h>
#include <fcntl.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define REQUIRE(test) do { if (!(test)) { fprintf(stderr,"line %u: %s, errno=%d\n",__LINE__,#test,errno); return 1; } } while (0)
static int current_cpu(void) {
 cpu_set_t allowed; CPU_ZERO(&allowed);
 REQUIRE(sched_getaffinity(0,sizeof(allowed),&allowed)==0);
 REQUIRE(syscall(SYS_getcpu,NULL,NULL,(void *)1)==0);
 for (int i=0; i<100; i++) {
  unsigned int cpu=~0u,node=~0u;
  REQUIRE(syscall(SYS_getcpu,&cpu,&node,NULL)==0);
  REQUIRE(cpu<CPU_SETSIZE && CPU_ISSET(cpu,&allowed) && node==0);
 }
 REQUIRE(syscall(SYS_getcpu,(void *)1,NULL,NULL)==-1 && errno==EFAULT);
 REQUIRE(syscall(SYS_getcpu,NULL,(void *)1,NULL)==-1 && errno==EFAULT);
 puts("PASS: getcpu reports an allowed CPU, NUMA node, optional outputs and EFAULT"); return 0;
}
static int set_lock(int fd, short kind, off_t start, off_t len) {
 struct flock lock={.l_type=kind,.l_whence=SEEK_SET,.l_start=start,.l_len=len};
 return fcntl(fd,F_SETLK,&lock);
}
static int record_locks(void) {
 char path[]="/var/tmp/vulkan-lock-XXXXXX";
 int fd=mkstemp(path); REQUIRE(fd>=0); unlink(path);
 REQUIRE(ftruncate(fd,200)==0);
 REQUIRE(set_lock(fd,F_WRLCK,0,100)==0);
 REQUIRE(set_lock(fd,F_UNLCK,10,10)==0);
 REQUIRE(set_lock(fd,F_RDLCK,20,10)==0);
 pid_t parent=getpid(), child=fork(); REQUIRE(child>=0);
 if (!child) {
  struct flock query={.l_type=F_WRLCK,.l_whence=SEEK_SET,.l_start=0,.l_len=1};
  REQUIRE(fcntl(fd,F_GETLK,&query)==0);
  REQUIRE(query.l_type==F_WRLCK && query.l_pid==parent && query.l_start==0 && query.l_len==10);
  REQUIRE(set_lock(fd,F_WRLCK,100,1)==0);
  REQUIRE(set_lock(fd,F_WRLCK,99,1)==-1 && errno==EAGAIN);
  REQUIRE(set_lock(fd,F_WRLCK,10,10)==0);
  REQUIRE(set_lock(fd,F_WRLCK,9,1)==-1 && errno==EAGAIN);
  REQUIRE(set_lock(fd,F_RDLCK,20,10)==0);
  REQUIRE(set_lock(fd,F_WRLCK,20,1)==-1 && errno==EAGAIN);
  query=(struct flock){.l_type=F_WRLCK,.l_whence=SEEK_SET,.l_start=40,.l_len=-10};
  REQUIRE(fcntl(fd,F_GETLK,&query)==0 && query.l_type==F_WRLCK && query.l_start==30);
  REQUIRE(lseek(fd,95,SEEK_SET)==95);
  query=(struct flock){.l_type=F_WRLCK,.l_whence=SEEK_CUR,.l_start=4,.l_len=1};
  REQUIRE(fcntl(fd,F_GETLK,&query)==0 && query.l_type==F_WRLCK);
  query=(struct flock){.l_type=F_WRLCK,.l_whence=SEEK_END,.l_start=-100,.l_len=1};
  REQUIRE(fcntl(fd,F_GETLK,&query)==0 && query.l_type==F_UNLCK);
  _exit(0);
 }
 int status; REQUIRE(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
 REQUIRE(set_lock(fd,F_UNLCK,0,0)==0);
 close(fd); puts("PASS: POSIX byte ranges, split/unlock, conversions, GETLK and fork ownership"); return 0;
}
static int native_descriptor(void) {
 long handle=syscall(0x53430000UL+620,4096,3); REQUIRE(handle>=0);
 int fd=syscall(0x53440000UL,handle,O_CLOEXEC); REQUIRE(fd>=0);
 REQUIRE(fcntl(fd,F_GETFD)==FD_CLOEXEC);
 REQUIRE(syscall(0x53440000UL,handle,1)==-1 && errno==EINVAL);
 REQUIRE(syscall(0x53440000UL,-1,0)==-1 && errno==EBADF);
 REQUIRE(syscall(0x53430000UL+102,handle)==0);
 unsigned char *pixels=mmap(NULL,4096,PROT_READ|PROT_WRITE,MAP_SHARED,fd,0);
 REQUIRE(pixels!=MAP_FAILED); pixels[37]=0x79;
 int sockets[2]; REQUIRE(socketpair(AF_UNIX,SOCK_STREAM,0,sockets)==0);
 char byte='x'; struct iovec vec={&byte,1};
 union {struct cmsghdr aligned; char bytes[CMSG_SPACE(sizeof(int))];} control;
 memset(&control,0,sizeof(control));
 struct msghdr message={.msg_iov=&vec,.msg_iovlen=1,.msg_control=&control,.msg_controllen=sizeof(control)};
 struct cmsghdr *cmsg=CMSG_FIRSTHDR(&message);
 cmsg->cmsg_level=SOL_SOCKET; cmsg->cmsg_type=SCM_RIGHTS; cmsg->cmsg_len=CMSG_LEN(sizeof(int));
 memcpy(CMSG_DATA(cmsg),&fd,sizeof(fd));
 REQUIRE(sendmsg(sockets[0],&message,0)==1);
 close(fd); memset(&control,0,sizeof(control)); message.msg_controllen=sizeof(control);
 REQUIRE(recvmsg(sockets[1],&message,MSG_CMSG_CLOEXEC)==1);
 cmsg=CMSG_FIRSTHDR(&message); REQUIRE(cmsg && cmsg->cmsg_level==SOL_SOCKET && cmsg->cmsg_type==SCM_RIGHTS);
 int received; memcpy(&received,CMSG_DATA(cmsg),sizeof(received));
 REQUIRE(fcntl(received,F_GETFD)==FD_CLOEXEC);
 unsigned char *copy=mmap(NULL,4096,PROT_READ|PROT_WRITE,MAP_SHARED,received,0);
 REQUIRE(copy!=MAP_FAILED && copy[37]==0x79); copy[38]=0x41; REQUIRE(pixels[38]==0x41);
 munmap(copy,4096);munmap(pixels,4096);close(received);close(sockets[0]);close(sockets[1]);
 puts("PASS: native capability duplication, CLOEXEC, SCM_RIGHTS and shared image lifetime"); return 0;
}
static int relative_sockets(void) {
 char directory[]="/var/tmp/vulkan-sockets-XXXXXX";
 REQUIRE(mkdtemp(directory)!=NULL);
 REQUIRE(chdir(directory)==0 && mkdir("a",0700)==0 && mkdir("b",0700)==0);
 struct sockaddr_un address={.sun_family=AF_UNIX}; strcpy(address.sun_path,"socket");
 int a=socket(AF_UNIX,SOCK_STREAM,0), b=socket(AF_UNIX,SOCK_STREAM,0);
 REQUIRE(a>=0 && b>=0 && chdir("a")==0);
 REQUIRE(bind(a,(struct sockaddr *)&address,sizeof(address))==0 && listen(a,2)==0);
 REQUIRE(chdir("../b")==0);
 REQUIRE(bind(b,(struct sockaddr *)&address,sizeof(address))==0 && listen(b,2)==0);
 int duplicate=socket(AF_UNIX,SOCK_STREAM,0); REQUIRE(duplicate>=0);
 REQUIRE(bind(duplicate,(struct sockaddr *)&address,sizeof(address))==-1 && errno==EADDRINUSE);
 close(duplicate);
 int client=socket(AF_UNIX,SOCK_STREAM,0); REQUIRE(client>=0);
 strcpy(address.sun_path,"../a/socket");
 REQUIRE(connect(client,(struct sockaddr *)&address,sizeof(address))==0);
 int accepted=accept(a,NULL,NULL); REQUIRE(accepted>=0);
 REQUIRE(write(client,"A",1)==1);
 char byte=0; REQUIRE(read(accepted,&byte,1)==1 && byte=='A');
 close(client);close(accepted);
 REQUIRE(unlink("socket")==0);
 int replacement=socket(AF_UNIX,SOCK_STREAM,0); REQUIRE(replacement>=0);
 strcpy(address.sun_path,"socket");
 REQUIRE(bind(replacement,(struct sockaddr *)&address,sizeof(address))==0);
 close(b); // Old socket teardown must preserve the replacement registration.
 REQUIRE(listen(replacement,2)==0);
 client=socket(AF_UNIX,SOCK_STREAM,0); REQUIRE(client>=0);
 REQUIRE(connect(client,(struct sockaddr *)&address,sizeof(address))==0);
 accepted=accept(replacement,NULL,NULL); REQUIRE(accepted>=0);
 close(client);close(accepted);close(replacement);close(a);
 REQUIRE(unlink("socket")==0 && unlink("../a/socket")==0);
 REQUIRE(chdir("..")==0 && rmdir("a")==0 && rmdir("b")==0);
 REQUIRE(chdir("/var/tmp")==0 && rmdir(directory)==0);
 puts("PASS: relative Unix socket names, independent Wine prefixes and replacement ownership"); return 0;
}
int main(void) {
 setvbuf(stdout,NULL,_IONBF,0);
 return current_cpu() || record_locks() || native_descriptor() || relative_sockets();
}
