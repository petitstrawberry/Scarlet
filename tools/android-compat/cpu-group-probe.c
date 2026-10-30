/* Exercise native group constraints through Bionic on multiple real HVF CPUs. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#define NATIVE(n) (0x53430000L+(n))
#define CHECK(x) do {if(!(x)){printf("SCARLET_CPU_GROUP_FAIL line=%d errno=%d\n",__LINE__,errno);return 1;}}while(0)
static long create(long parent,const char *name) {return syscall(NATIVE(1330),parent,name);}
static int constrain(long group,unsigned long mask) {return (int)syscall(NATIVE(1335),group,mask?1:2,mask,0);}
static unsigned long allowed(void) {cpu_set_t mask;CPU_ZERO(&mask);if(sched_getaffinity(0,sizeof(mask),&mask))return 0;return (unsigned long)mask.__bits[0];}
static void *observe(void *argument) {unsigned long mask=(unsigned long)argument;for(int i=0;i<500;i++){int cpu=sched_getcpu();if(cpu<0 || !(mask&(1UL<<cpu)) || allowed()!=mask)return (void*)1;sched_yield();}return NULL;}
static int control(const char *path,const char *value) {int fd=open(path,O_WRONLY|O_CLOEXEC);if(fd<0)return -1;ssize_t count=write(fd,value,strlen(value));int saved=errno;close(fd);errno=saved;return count==(ssize_t)strlen(value)?0:-1;}
static int contains_pid(const char *path) {char text[512];int fd=open(path,O_RDONLY|O_CLOEXEC);if(fd<0)return 0;ssize_t n=read(fd,text,sizeof(text)-1);close(fd);if(n<=0)return 0;text[n]=0;char expected[32];snprintf(expected,sizeof(expected),"%d\n",getpid());return strstr(text,expected)!=NULL;}
int main(int argc,char **argv) {
 if(argc==3 && !strcmp(argv[1],"exec")) {unsigned long mask=strtoul(argv[2],NULL,0);CHECK(allowed()==mask && !observe((void*)mask));return 0;}
 unsigned long online=allowed();CHECK(__builtin_popcountl(online)>=2);
 long original=syscall(NATIVE(1331));CHECK(original>=0);long root=create(-1,NULL);CHECK(root>=0);
 unsigned long first=online&-online,rest=online&~first,second=rest&-rest;
 CHECK(constrain(root,first|second)==0);
 long group=create(root,"cpus"),child=create(group,"child");CHECK(group>=0 && child>=0);
 CHECK(constrain(group,second)==0 && constrain(child,first)<0);
 CHECK(syscall(NATIVE(1332),group,0)==0 && allowed()==second);
 CHECK(!observe((void*)second));
 cpu_set_t requested;CPU_ZERO(&requested);CPU_SET(__builtin_ctzl(first),&requested);
 CHECK(sched_setaffinity(0,sizeof(requested),&requested)<0 && allowed()==second);
 CPU_SET(__builtin_ctzl(second),&requested);CHECK(sched_setaffinity(0,sizeof(requested),&requested)==0 && allowed()==second);
 pthread_t worker;CHECK(pthread_create(&worker,NULL,observe,(void*)second)==0);void *result;CHECK(pthread_join(worker,&result)==0 && !result);
 CHECK(constrain(child,second)==0 && constrain(root,first)<0 && allowed()==second);
 CHECK(constrain(child,0)==0 && constrain(group,0)==0 && allowed()==(first|second));
 CHECK(constrain(root,first)==0 && allowed()==first && !observe((void*)first));
 pid_t pid=fork();CHECK(pid>=0);if(!pid){char text[32];snprintf(text,sizeof(text),"%lu",first);execl(argv[0],argv[0],"exec",text,NULL);_exit(3);}int status;pid_t waited;do{waited=waitpid(pid,&status,0);}while(waited<0 && errno==EINTR);CHECK(waited==pid && WIFEXITED(status) && WEXITSTATUS(status)==0);
 long placement=create(-1,NULL);CHECK(placement>=0 && constrain(placement,first)==0);
 CHECK(syscall(NATIVE(1332),placement,0)==0 && allowed()==first);
 uint64_t counts[4];CHECK(syscall(NATIVE(1333),group,counts)==0 && counts[1]==1);
 CHECK(syscall(NATIVE(1333),placement,counts)==0 && counts[1]==1);
 CHECK(syscall(NATIVE(1336),placement)==0 && allowed()==first);
 CHECK(syscall(NATIVE(1336),root)==0 && allowed()==(first|second));
 CHECK(syscall(NATIVE(1332),original,0)==0);
 CHECK(mkdir("/dev/cpuset",0755)==0 || errno==EEXIST);
 CHECK(mount("none","/sys/fs/cgroup","cgroup2",MS_NODEV|MS_NOEXEC|MS_NOSUID,NULL)==0);
 CHECK(mount("none","/dev/cpuset","cpuset",MS_NODEV|MS_NOEXEC|MS_NOSUID,NULL)==0);
 CHECK(mkdir("/sys/fs/cgroup/probe",0755)==0 && mkdir("/dev/cpuset/probe",0755)==0);
 CHECK(control("/sys/fs/cgroup/probe/cgroup.procs","0")==0);
 CHECK(control("/dev/cpuset/probe/cgroup.procs","0")<0);
 char cpulist[32];snprintf(cpulist,sizeof(cpulist),"%u",(unsigned)__builtin_ctzl(second));
 CHECK(control("/dev/cpuset/probe/cpus",cpulist)==0 && control("/dev/cpuset/probe/mems","0")==0);
 CHECK(control("/dev/cpuset/probe/cgroup.procs","0")==0 && allowed()==second && !observe((void*)second));
 CHECK(contains_pid("/sys/fs/cgroup/probe/cgroup.procs") && contains_pid("/dev/cpuset/probe/tasks"));
 CHECK(control("/dev/cpuset/probe/mems","1")<0 && control("/dev/cpuset/probe/cpus","63")<0 && allowed()==second);
 CHECK(control("/dev/cpuset/tasks","0")==0 && allowed()==(first|second));
 CHECK(contains_pid("/sys/fs/cgroup/probe/cgroup.procs"));
 CHECK(control("/sys/fs/cgroup/cgroup.procs","0")==0);
 CHECK(rmdir("/sys/fs/cgroup/probe")==0 && rmdir("/dev/cpuset/probe")==0);
 printf("SCARLET_CPU_GROUP_OK actual-cpus hierarchy affinity thread fork exec rollback independent-hierarchies cpuset-and-cgroup2\n");return 0;
}
