/* Namespace-local Android diagnostic helper. This ELF is intentionally not
 * static: Scarlet selects the stock Android/Linux ABI through its dynamic
 * linker, so tgkill reaches the ART Signal Catcher thread rather than the
 * Scarlet-native process-control event path. */
#define _GNU_SOURCE
#include <errno.h>
#include <dirent.h>
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

static void read_wait_state(unsigned long pid) {
 const char *suffixes[]={"stack","syscall","wchan","stat","comm","status"};
 for(unsigned s=0;s<sizeof(suffixes)/sizeof(suffixes[0]);s++) {
  char path[96],value[8192];
  snprintf(path,sizeof(path),"/proc/%lu/%s",pid,suffixes[s]);
  int fd=open(path,O_RDONLY|O_CLOEXEC);
  if(fd<0)continue;
  ssize_t length=read(fd,value,sizeof(value)-1);close(fd);
  if(length<=0)continue;
  printf("SCARLET_INIT_JAVA_WAIT pid=%lu source=%s",pid,suffixes[s]);
  for(ssize_t i=0;i<length;i++)putchar(value[i]=='\n'?'|':value[i]);
  putchar('\n');
 }
}

static int read_small(unsigned long pid,const char *suffix,char *out,size_t capacity) {
 char path[96];snprintf(path,sizeof(path),"/proc/%lu/%s",pid,suffix);
 int fd=open(path,O_RDONLY|O_CLOEXEC);if(fd<0)return -1;
 ssize_t length=read(fd,out,capacity-1);close(fd);
 if(length<=0)return -1;out[length]=0;return 0;
}

static unsigned long proc_target(const char *needle) {
 for(unsigned long pid=1;pid<1024;pid++) {
  char status[4096];if(read_small(pid,"status",status,sizeof(status)))continue;
  if(strstr(status,needle))return pid;
 }
 return 0;
}

static void proc_children(unsigned long parent) {
 if(!parent)return;
 char needle[64];snprintf(needle,sizeof(needle),"PPid:\t%lu\n",parent);
 for(unsigned long pid=1;pid<1024;pid++) {
  char status[4096];if(read_small(pid,"status",status,sizeof(status)))continue;
  if(!strstr(status,needle))continue;
  char comm[64]={0};read_small(pid,"comm",comm,sizeof(comm));
  printf("SCARLET_INIT_JAVA_CHILD parent=%lu pid=%lu name=%.63s\n",parent,pid,comm);
  read_wait_state(pid);
 }
}

int main(int argc,char **argv) {
 setvbuf(stdout,NULL,_IONBF,0);
 int observation=argc>1?atoi(argv[1]):210;
 int proc_scan=getenv("SCARLET_INIT_PROC_SCAN")==NULL || getenv("SCARLET_INIT_PROC_SCAN")[0]!='0';
 int first_seen=-1,last_dump=-1;
 for(int tick=0;tick<observation*10;tick++) {
  if(tick>=200 && tick%10==0) {
   int scan_now=proc_scan && (tick==400 || tick==900 || tick==1400 || tick==2000);
   unsigned long proc_system=0, proc_systemui=0, proc_catcher=0;
   static struct task_info info[1024];
   long count=syscall(0x53430000L+25,info,1024);
   unsigned long launcher=0,signal_catcher=0;
   for(long i=0;i<count && i<1024;i++) {
    if(info[i].type!=1)continue;
    if(info[i].pid==info[i].tgid && strstr(info[i].name,"launcher3"))launcher=info[i].pid;
    if(scan_now && info[i].pid==info[i].tgid && strstr(info[i].name,"system_server"))proc_system=info[i].pid;
    if(scan_now && info[i].pid==info[i].tgid && strstr(info[i].name,"com.android.systemui"))proc_systemui=info[i].pid;
   }
   if(scan_now && !proc_system)proc_system=proc_target("system_server");
   if(scan_now && !proc_systemui)proc_systemui=proc_target("com.android.sys");
   if(scan_now && !proc_systemui)proc_systemui=proc_target("Uid:\t10010");
   for(long i=0;i<count && i<1024;i++) {
    if(launcher && info[i].tgid==launcher && strstr(info[i].name,"Signal Catcher"))signal_catcher=info[i].pid;
    if(proc_systemui && info[i].tgid==proc_systemui && strstr(info[i].name,"Signal Catcher"))proc_catcher=info[i].pid;
   }
   if(scan_now) {
    printf("SCARLET_INIT_JAVA_SCAN system=%lu systemui=%lu\n",proc_system,proc_systemui);
    if(proc_system)read_wait_state(proc_system);
    unsigned long proc_dex=proc_target("dex2oat64");
    if(proc_dex)read_wait_state(proc_dex);
    if(proc_systemui)read_wait_state(proc_systemui);
    unsigned long proc_installd=proc_target("installd");
    proc_children(proc_installd);
    printf("SCARLET_INIT_JAVA_PROC_TARGET system=%lu systemui=%lu dex2oat=%lu catcher=%lu\n",
           proc_system,proc_systemui,proc_dex,proc_catcher);
    if(proc_catcher) {
     errno=0;long status=syscall(SYS_tgkill,(pid_t)proc_systemui,(pid_t)proc_catcher,SIGQUIT);
     printf("SCARLET_INIT_JAVA_PROC_DUMP pid=%lu tid=%lu result=%ld errno=%d\n",
            proc_systemui,proc_catcher,status,errno);
    }
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
     if(!strstr(info[i].name,"system_server") && !strstr(info[i].name,"com.android.systemui"))continue;
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
