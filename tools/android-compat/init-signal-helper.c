/* Namespace-local Android diagnostic helper. This ELF is intentionally not
 * static: Scarlet selects the stock Android/Linux ABI through its dynamic
 * linker, so tgkill reaches the ART Signal Catcher thread rather than the
 * Scarlet-native process-control event path. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

struct task_info {
 unsigned long pid,ppid;unsigned char state,type,cpu,reserved;int exit_status;unsigned long tgid;
 char name[64];unsigned long long cpu_ns;unsigned util,min_util,capacity;unsigned char preference,padding[3];
 unsigned long long migrations;int nice;unsigned weight;unsigned long long vruntime,deadline;
};
_Static_assert(sizeof(struct task_info)==152,"native task info layout");

static int read_trace(void) {
 const char *paths[]={"/data/anr/traces.txt","/data/misc/trace/traces.txt","/data/local/tmp/traces.txt"};
 for(unsigned p=0;p<sizeof(paths)/sizeof(paths[0]);p++) {
  int fd=open(paths[p],O_RDONLY|O_CLOEXEC);
  if(fd<0)continue;
  char trace[65536];ssize_t length=read(fd,trace,sizeof(trace)-1);close(fd);
  if(length<=0)continue;
  printf("SCARLET_INIT_JAVA_TRACE path=%s bytes=%zd\n",paths[p],length);
  fwrite(trace,1,(size_t)length,stdout);putchar('\n');
  return 1;
 }
 return 0;
}

int main(int argc,char **argv) {
 setvbuf(stdout,NULL,_IONBF,0);
 int observation=argc>1?atoi(argv[1]):210;
 int first_seen=-1,last_dump=-1;
 for(int tick=0;tick<observation*10;tick++) {
  if(tick>=200 && tick%10==0) {
   static struct task_info info[1024];
   long count=syscall(0x53430000L+25,info,1024);
   unsigned long launcher=0,signal_catcher=0;
   for(long i=0;i<count && i<1024;i++) {
    if(info[i].type!=1)continue;
    if(info[i].pid==info[i].tgid && strstr(info[i].name,"launcher3"))launcher=info[i].pid;
   }
   for(long i=0;i<count && i<1024;i++) {
    if(launcher && info[i].tgid==launcher && strstr(info[i].name,"Signal Catcher"))signal_catcher=info[i].pid;
   }
   if(tick%600==0) {
    for(long i=0;i<count && i<1024;i++) {
     if(info[i].pid!=info[i].tgid || !info[i].pid)continue;
     char path[80],label[256];
     snprintf(path,sizeof(path),"/proc/%lu/attr/current",info[i].pid);
     int fd=open(path,O_RDONLY|O_CLOEXEC);
     if(fd<0)continue;
     ssize_t length=read(fd,label,sizeof(label)-1);
     close(fd);
     if(length<=0)continue;
     label[length]=0;
     if(!strstr(label,"u:r:system_server:s0"))continue;
     for(long j=0;j<count && j<1024;j++) {
      if(info[j].tgid==info[i].tgid && strstr(info[j].name,"Signal Catcher")) {
       errno=0;long status=syscall(SYS_tgkill,(pid_t)info[i].pid,(pid_t)info[j].pid,SIGQUIT);
       printf("SCARLET_INIT_JAVA_SYSTEM_DUMP pid=%lu tid=%lu result=%ld errno=%d\n",info[i].pid,info[j].pid,status,errno);
      }
     }
    }
   }
   printf("SCARLET_INIT_JAVA_TARGET tick=%d tasks=%ld launcher=%lu signal_catcher=%lu\n",
          tick,count,launcher,signal_catcher);
    if(launcher && first_seen<0)first_seen=tick;
    if(launcher && signal_catcher && tick-first_seen>=200 && (last_dump<0 || tick-last_dump>=200)) {
    last_dump=tick;
    errno=0;long result=syscall(SYS_tgkill,(pid_t)launcher,(pid_t)signal_catcher,SIGQUIT);
    printf("SCARLET_INIT_JAVA_DUMP pid=%lu tid=%lu result=%ld errno=%d\n",
           launcher,signal_catcher,result,errno);
    for(int attempt=0;attempt<8;attempt++) {
     sleep(1);
     if(read_trace())break;
    }
    read_trace();
    if(tick-first_seen>=600)_exit(0);
   }
  }
  usleep(100000);
 }
 return 0;
}
