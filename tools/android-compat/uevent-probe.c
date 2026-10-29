/* Exercise the same authenticated kernel-multicast receive contract as ueventd. */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/netlink.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)){printf("SCARLET_UEVENT_FAIL line=%d errno=%d\n",__LINE__,errno);return 1;} }while(0)
static int channel(void) {
 int fd=socket(AF_NETLINK,SOCK_DGRAM|SOCK_NONBLOCK|SOCK_CLOEXEC,NETLINK_KOBJECT_UEVENT);
 if(fd<0)return -1;
 int size=1024*1024,one=1,actual=0;socklen_t length=sizeof(actual);
 if(setsockopt(fd,SOL_SOCKET,SO_RCVBUF,&size,sizeof(size)) || getsockopt(fd,SOL_SOCKET,SO_RCVBUF,&actual,&length) || actual!=2*size || setsockopt(fd,SOL_SOCKET,SO_PASSCRED,&one,sizeof(one)))return -1;
 struct sockaddr_nl address={.nl_family=AF_NETLINK,.nl_groups=~0u};
 if(bind(fd,(struct sockaddr*)&address,sizeof(address)))return -1;return fd;
}
static int receive(int fd,int flags,char *buffer,size_t capacity,size_t *full) {
 struct sockaddr_nl source={0};char control[CMSG_SPACE(sizeof(struct ucred))]={0};
 struct iovec vector={buffer,capacity};struct msghdr message={.msg_name=&source,.msg_namelen=sizeof(source),.msg_iov=&vector,.msg_iovlen=1,.msg_control=control,.msg_controllen=sizeof(control)};
 ssize_t size=recvmsg(fd,&message,flags);if(size<0)return -1;
 struct cmsghdr *header=CMSG_FIRSTHDR(&message);if(!header || header->cmsg_type!=SCM_CREDENTIALS || header->cmsg_level!=SOL_SOCKET || header->cmsg_len!=CMSG_LEN(sizeof(struct ucred)))return -2;
 struct ucred credentials;memcpy(&credentials,CMSG_DATA(header),sizeof(credentials));
 if(credentials.pid!=0 || credentials.uid!=0 || credentials.gid!=0 || source.nl_family!=AF_NETLINK || source.nl_pid!=0 || source.nl_groups!=1)return -3;
 if(full)*full=(size_t)size;return message.msg_flags;
}
int main(void) {
 int first=channel(),second=channel();CHECK(first>=0 && second>=0);
 struct pollfd poller={.fd=first,.events=POLLIN};CHECK(poll(&poller,1,0)==0);
 DIR *directory=opendir("/sys/devices/scarlet");CHECK(directory!=NULL);
 char path[256]={0};struct dirent *entry;while((entry=readdir(directory))) {
  if(entry->d_name[0]=='.')continue;
  int size=snprintf(path,sizeof(path),"/sys/devices/scarlet/%s/uevent",entry->d_name);CHECK(size>0 && size<(int)sizeof(path));break;
 }
 CHECK(closedir(directory)==0 && path[0]);int replay=open(path,O_WRONLY|O_CLOEXEC);CHECK(replay>=0 && write(replay,"add\n",4)==4 && close(replay)==0);
 CHECK(poll(&poller,1,1000)==1 && (poller.revents&POLLIN));
 char tiny[8],packet[2048],other[2048];size_t full=0;
 CHECK(receive(first,MSG_PEEK|MSG_TRUNC,tiny,sizeof(tiny),&full)>=0 && full>sizeof(tiny));
 CHECK(receive(first,0,packet,sizeof(packet),&full)==0 && full<sizeof(packet));
 size_t other_size=0;CHECK(receive(second,0,other,sizeof(other),&other_size)==0 && full==other_size && !memcmp(packet,other,full));
 CHECK(!memcmp(packet,"add@/devices/scarlet/",20));
 CHECK(receive(first,0,packet,sizeof(packet),NULL)==-1 && errno==EAGAIN);
 poller.revents=0;CHECK(poll(&poller,1,20)==0);
 CHECK(send(first,"spoof",5,0)==-1);
 CHECK(close(first)==0 && close(second)==0);
 printf("SCARLET_UEVENT_OK native-replay multicast-credentials independent-queues peek trunc poll no-injection\n");return 0;
}
