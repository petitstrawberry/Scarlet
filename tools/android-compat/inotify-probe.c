/* Directory events through real VFS mutations, fork and descriptor lifetime. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/inotify.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#define CHECK(test) do {if(!(test)){perror(#test);return 1;}} while(0)
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);
    CHECK(mkdir("/inotify-probe",0700)==0);
    CHECK(mount("tmpfs","/inotify-probe","tmpfs",0,NULL)==0);
    int fd=inotify_init1(IN_NONBLOCK|IN_CLOEXEC);CHECK(fd>=0);
    int wd=inotify_add_watch(fd,"/inotify-probe",IN_CREATE|IN_DELETE|IN_MOVED_FROM|IN_MOVED_TO|IN_ONLYDIR);CHECK(wd>0);
    int ep=epoll_create1(EPOLL_CLOEXEC);CHECK(ep>=0);
    struct epoll_event ready={.events=EPOLLIN|EPOLLWAKEUP,.data.fd=fd};CHECK(epoll_ctl(ep,EPOLL_CTL_ADD,fd,&ready)==0);
    int file=open("/inotify-probe/a",O_RDWR|O_CREAT|O_EXCL,0600);CHECK(file>=0);CHECK(close(file)==0);
    char bytes[4096];errno=0;CHECK(read(fd,bytes,15)==-1 && errno==EINVAL);
    CHECK(epoll_wait(ep,&ready,1,0)==1 && ready.data.fd==fd);
    CHECK(rename("/inotify-probe/a","/inotify-probe/b")==0);CHECK(unlink("/inotify-probe/b")==0);
    pid_t child=fork();CHECK(child>=0);if(!child)_exit(mkdir("/inotify-probe/child",0700)?42:0);
    int status;CHECK(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
    ssize_t count=read(fd,bytes,sizeof(bytes));CHECK(count>0);
    unsigned events=0;uint32_t cookie=0;
    for(ssize_t at=0;at<count;) {
        struct inotify_event header;CHECK(count-at>=(ssize_t)sizeof(header));memcpy(&header,bytes+at,sizeof(header));
        CHECK(header.wd==wd && header.len<=count-at-sizeof(header));
        const char *name=bytes+at+sizeof(header);CHECK(header.len>0 && memchr(name,0,header.len));
        if(header.mask&IN_MOVED_FROM) {CHECK(!strcmp(name,"a") && header.cookie!=0);cookie=header.cookie;}
        if(header.mask&IN_MOVED_TO) CHECK(!strcmp(name,"b") && cookie==header.cookie);
        if(!strcmp(name,"child")) CHECK((header.mask&(IN_CREATE|IN_ISDIR))==(IN_CREATE|IN_ISDIR));
        events++;at+=(ssize_t)sizeof(header)+header.len;
    }
    CHECK(events==5);
    int duplicate=dup(fd);CHECK(duplicate>=0);CHECK(close(fd)==0);
    CHECK(inotify_rm_watch(duplicate,wd)==0);count=read(duplicate,bytes,sizeof(bytes));CHECK(count==16);
    struct inotify_event header;memcpy(&header,bytes,16);CHECK(header.wd==wd && header.mask==IN_IGNORED);
    int once=inotify_add_watch(duplicate,"/inotify-probe",IN_CREATE|IN_ONESHOT|IN_ONLYDIR);CHECK(once>wd);
    CHECK(mkdir("/inotify-probe/once",0700)==0);CHECK(mkdir("/inotify-probe/unwatched",0700)==0);
    count=read(duplicate,bytes,sizeof(bytes));CHECK(count>16);
    memcpy(&header,bytes,16);CHECK(header.wd==once && (header.mask&IN_CREATE));
    ssize_t at=16+header.len;CHECK(count-at==16);memcpy(&header,bytes+at,16);CHECK(header.mask==IN_IGNORED);
    errno=0;CHECK(read(duplicate,bytes,sizeof(bytes))==-1 && errno==EAGAIN);
    CHECK(close(duplicate)==0 && close(ep)==0);
    CHECK(umount2("/inotify-probe",MNT_DETACH)==0 && rmdir("/inotify-probe")==0);
    puts("SCARLET_INOTIFY_OK create rename-cookie delete fork short-buffer epoll dup oneshot ignored");
    return 0;
}
