/* Stock Bionic alarm timerfds, epoll delivery and privilege enforcement. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/epoll.h>
#include <sys/syscall.h>
#include <sys/timerfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#define CHECK(value) do { if (!(value)) { perror(#value); return 1; } } while (0)
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);
    int clocks[]={CLOCK_REALTIME_ALARM,CLOCK_BOOTTIME_ALARM};
    for (unsigned i=0;i<2;i++) {
        int fd=timerfd_create(clocks[i],TFD_NONBLOCK|TFD_CLOEXEC);CHECK(fd>=0);
        int ep=epoll_create1(EPOLL_CLOEXEC);CHECK(ep>=0);
        struct epoll_event ev={.events=EPOLLIN|EPOLLWAKEUP,.data.fd=fd};
        CHECK(epoll_ctl(ep,EPOLL_CTL_ADD,fd,&ev)==0);
        struct itimerspec when={0};when.it_value.tv_nsec=20000000;
        CHECK(timerfd_settime(fd,0,&when,NULL)==0);
        CHECK(epoll_wait(ep,&ev,1,2000)==1 && ev.data.fd==fd);
        uint64_t count=0;CHECK(read(fd,&count,sizeof(count))==sizeof(count) && count==1);
        CHECK(close(fd)==0 && close(ep)==0);
    }
    int monitor=timerfd_create(CLOCK_REALTIME,TFD_NONBLOCK);CHECK(monitor>=0);
    struct itimerspec disarm={0};CHECK(timerfd_settime(monitor,TFD_TIMER_ABSTIME|TFD_TIMER_CANCEL_ON_SET,&disarm,NULL)==0);
    CHECK(close(monitor)==0);
    int inherited=timerfd_create(CLOCK_BOOTTIME_ALARM,TFD_CLOEXEC);CHECK(inherited>=0);
    pid_t child=fork();CHECK(child>=0);
    if (!child) {
        if(setuid(10001))_exit(40);
        errno=0;int fd=timerfd_create(CLOCK_BOOTTIME_ALARM,0);
        if(fd!=-1 || errno!=EPERM)_exit(41);
        errno=0;if(timerfd_settime(inherited,0,&disarm,NULL)!=-1 || errno!=EPERM)_exit(42);
        _exit(0);
    }
    int status;CHECK(waitpid(child,&status,0)==child && WIFEXITED(status) && !WEXITSTATUS(status));
    CHECK(close(inherited)==0);
    puts("SCARLET_WAKE_TIMER_OK realtime-alarm boottime-alarm epoll privilege-denied cancel-monitor");
    return 0;
}
