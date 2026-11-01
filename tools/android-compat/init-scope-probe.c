/* Bounded diagnostic supervisor. Init exit and observation timeout are distinct. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <errno.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/stat.h>
#include <sys/system_properties.h>
#include <time.h>
struct task_info {
 unsigned long pid,ppid;unsigned char state,type,cpu,reserved;int exit_status;unsigned long tgid;
 char name[64];unsigned long long cpu_ns;unsigned util,min_util,capacity;unsigned char preference,padding[3];
 unsigned long long migrations;int nice;unsigned weight;unsigned long long vruntime,deadline;
};
_Static_assert(sizeof(struct task_info)==152,"native task info layout");
static void snapshot(void) {
 struct task_info info[128];long count=syscall(0x53430000L+25,info,128);
 for(long i=0;i<count && i<128;i++)if(info[i].type==1)
  printf("SCARLET_INIT_TASK pid=%lu ppid=%lu tgid=%lu state=%u exit=%d cpu_ns=%llu name=%.64s\n",info[i].pid,info[i].ppid,info[i].tgid,info[i].state,info[i].exit_status,info[i].cpu_ns,info[i].name);
}
static int log_socket(void) {
 int fd=socket(AF_UNIX,SOCK_DGRAM|SOCK_CLOEXEC,0);if(fd<0)return -1;
 struct sockaddr_un address={.sun_family=AF_UNIX};strcpy(address.sun_path,"/dev/socket/logdw");
 if(bind(fd,(struct sockaddr*)&address,sizeof(address))){close(fd);return -1;}return fd;
}
static void drain_logs(int fd) {
 if(fd<0)return;
 if(getenv("SCARLET_INIT_QUIET"))return;
 for(int count=0;count<128;count++) {unsigned char message[8192];ssize_t n=recv(fd,message,sizeof(message),MSG_DONTWAIT);
  if(n<0)break;
  printf("SCARLET_INIT_LOGD bytes=%zd ",n);
  for(ssize_t i=0;i<n;i++)if(message[i]<32 || message[i]>=127)message[i]='|';
  fwrite(message,1,(size_t)n,stdout);
  putchar('\n');
 }
}
int main(int argc,char **argv) {
 setvbuf(stdout,NULL,_IONBF,0);
 int observation=20;const char *requested=getenv("SCARLET_INIT_OBSERVE_SECONDS");
 if(requested) {char *end=NULL;long value=strtol(requested,&end,10);if(!*requested || *end || value<20 || value>480)return 16;observation=(int)value;}
 if(argc==2 && !strcmp(argv[1],"inside")) {
  printf("SCARLET_INIT_NAMESPACE_INSIDE pid=%d ppid=%d tid=%ld\n",getpid(),getppid(),syscall(SYS_gettid));
  if(getpid()!=1 || getppid()!=0 || syscall(SYS_gettid)!=1)return 3;
  char *const args[]={"/system/bin/init","second_stage",NULL};
  setenv("LD_PRELOAD","libscarlet-android-shim.so:libprobe-init-kmsg.so",1);
  execv(args[0],args);perror("stock init exec");return 4;
 }
 long scope=syscall(0x53430000L+1320);if(scope<0)return 11;
 int logfd=log_socket();if(logfd<0)perror("diagnostic logdw bind");
 pid_t child=fork();if(child<0)return 1;
 if(child==0) {
  if(logfd>=0)close(logfd);
  // Native CreateNamespace before any seccomp installation. Exec rebuilds the Android ABI state.
  if(syscall(0x5343005c,3,"android-init-diagnostic")!=0){perror("namespace");_exit(2);}
  if(syscall(0x53430000L+1321,scope) || syscall(0x53430000L+1322,scope,0) || syscall(0x53430000L+102,scope))return 12;
  // The namespace supervisor outside Android cannot address namespace-local
  // PIDs. Keep this diagnostic helper inside the namespace and let init retain
  // PID 1 while the helper observes application metadata and sends SIGQUIT.
  pid_t signaler=getenv("SCARLET_INIT_JAVA_DUMP")?fork():-1;
  if(signaler==0) {
   // Let stock init publish linker configuration before loading the dynamic
   // helper; a dynamic Android ELF selects the Linux ABI that implements tgkill.
   sleep(10);
   char value[16];snprintf(value,sizeof(value),"%d",observation);
   char *const helper[]={"/system/bin/scarlet-signal-helper",value,NULL};
   execv(helper[0],helper);perror("signal helper exec");_exit(6);
  }
  char *const args[]={argv[0],"inside",NULL};execv(args[0],args);_exit(5);
 }
 printf("SCARLET_INIT_NAMESPACE_OUTER child=%d\n",child);
 struct timespec started,now;if(clock_gettime(CLOCK_MONOTONIC,&started))return 15;
 for(int tick=0;tick<observation*10;tick++) {
  if(clock_gettime(CLOCK_MONOTONIC,&now))return 15;
  if(now.tv_sec-started.tv_sec>observation || (now.tv_sec-started.tv_sec==observation && now.tv_nsec>=started.tv_nsec))break;
  drain_logs(logfd);
  int status;pid_t got=waitpid(child,&status,WNOHANG);
  if(got==child){
   printf("SCARLET_INIT_NAMESPACE_REAP %d %d\n",got,status);
   if(syscall(0x53430000L+1322,scope,0) || syscall(0x53430000L+1323,scope))return 13;
   unsigned state[4]={0};
   for(int tick=0;tick<200;tick++) {
    if(syscall(0x53430000L+1324,scope,state))return 14;
    if(state[0]==1 && state[1]==0)break;
    usleep(10000);
   }
   if(state[0]!=1 || state[1]!=0){printf("SCARLET_INIT_SCOPE_PENDING closing=%u live=%u members=%u\n",state[0],state[1],state[2]);return 14;}
   printf("SCARLET_INIT_SCOPE_CLOSED live=%u members=%u\n",state[1],state[2]);
   return 0;
  }
  if(got<0){perror("outer wait");return 6;}
  if(tick==40 && argc==2 && !strcmp(argv[1],"start-zygote")) {int result=__system_property_set("ctl.start","zygote");printf("SCARLET_INIT_CONTROL_REQUEST service=zygote result=%d\n",result);}
  if(tick==40 && getenv("SCARLET_INIT_START_INSTALLER") && !strcmp(getenv("SCARLET_INIT_START_INSTALLER"),"1")) {
   int result=__system_property_set("ctl.start","installd");printf("SCARLET_INIT_CONTROL_REQUEST service=installd result=%d\n",result);
  }
  if(tick==40 && getenv("SCARLET_INIT_START_NETD") && !strcmp(getenv("SCARLET_INIT_START_NETD"),"1")) {
   int bpfflag=__system_property_set("bpf.progs_loaded","1");
   char bpfvalue[PROP_VALUE_MAX]={0};__system_property_get("bpf.progs_loaded",bpfvalue);
   printf("SCARLET_INIT_BPF_LOADER_FLAG set=%d value=%s\n",bpfflag,bpfvalue);
   int result=__system_property_set("ctl.start","netd");printf("SCARLET_INIT_CONTROL_REQUEST service=netd result=%d\n",result);
  }
  if(tick==40 || tick==80 || tick==200 || tick==240 || tick==280)snapshot();
  usleep(100000);
 }
 if(syscall(0x53430000L+1322,scope,0) || syscall(0x53430000L+1323,scope)){perror("owned scope stop");return 7;}
 int status;if(waitpid(child,&status,0)!=child)return 8;
 printf("SCARLET_INIT_NAMESPACE_STOP_REAP %d %d\n",child,status);
 unsigned state[4]={0};
 for(int tick=0;tick<200;tick++) {
  if(syscall(0x53430000L+1324,scope,state))return 14;
  if(state[0]==1 && state[1]==0)break;
  usleep(10000);
 }
 if(state[0]!=1 || state[1]!=0)return 14;
 printf("SCARLET_INIT_SCOPE_CLOSED live=%u members=%u\n",state[1],state[2]);
 return 0;
}
