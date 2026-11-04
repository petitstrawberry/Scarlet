/* Init supervisor. A zero observation interval keeps an interactive session alive. */
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
static void capacity_snapshot(void) {
 struct {unsigned size,version;unsigned long long users,kernels,stack_bytes,active_users,active_kernels,pending_users,pending_kernels,slots,used_slots;} info={0};
 _Static_assert(sizeof(info)==80,"native task capacity layout");
 if(syscall(0x53430000L+55,0,&info,sizeof(info))==0 && info.size==sizeof(info) && info.version==1)
  printf("SCARLET_INIT_CAPACITY max_user=%llu max_kernel=%llu stack_bytes=%llu active_user=%llu active_kernel=%llu pending_user=%llu pending_kernel=%llu stack_slots=%llu used_stack_slots=%llu\n",info.users,info.kernels,info.stack_bytes,info.active_users,info.active_kernels,info.pending_users,info.pending_kernels,info.slots,info.used_slots);
}
static void snapshot(void) {
 struct timespec now;
 if(!clock_gettime(CLOCK_MONOTONIC,&now))
  printf("SCARLET_INIT_TASK_SAMPLE time_ns=%llu\n",(unsigned long long)now.tv_sec*1000000000ULL+(unsigned long long)now.tv_nsec);
 struct task_info info[128];long count=syscall(0x53430000L+25,info,128);
 for(long i=0;i<count && i<128;i++)if(info[i].type==1)
  printf("SCARLET_INIT_TASK pid=%lu ppid=%lu tgid=%lu state=%u exit=%d cpu_ns=%llu name=%.64s\n",info[i].pid,info[i].ppid,info[i].tgid,info[i].state,info[i].exit_status,info[i].cpu_ns,info[i].name);
}
static int log_socket(void) {
 int fd=socket(AF_UNIX,SOCK_DGRAM|SOCK_CLOEXEC,0);if(fd<0)return -1;
 struct sockaddr_un address={.sun_family=AF_UNIX};strcpy(address.sun_path,"/dev/socket/logdw");
 if(bind(fd,(struct sockaddr*)&address,sizeof(address))){close(fd);return -1;}return fd;
}
static int important_log(const unsigned char *message,size_t length) {
 // The same socket carries Android logger packets and the preload's plain
 // text diagnostics. Quiet mode must retain crashes and command results.
 const char *prefixes[]={"SCARLET_INIT_JAVA_FATAL ","SCARLET_INIT_ABORT_MESSAGE ",
                         "SCARLET_ANDROID_STDIO ","SCARLET_INIT_LOG priority=6 ",
                         "SCARLET_INIT_LOG priority=7 "};
 for(unsigned i=0;i<sizeof(prefixes)/sizeof(prefixes[0]);i++) {
  size_t n=strlen(prefixes[i]);
  if(length>=n && !memcmp(message,prefixes[i],n))return 1;
 }
 // Packed logdw header: log ID (1), TID (2), seconds (4), nanoseconds (4).
 // Events/stats/security buffers contain binary records, not a priority byte.
 if(length<=12 || !(message[0]==0 || message[0]==1 || message[0]==3 || message[0]==4))return 0;
 if(message[11]>=6 && message[11]<=7)return 1;
 if(message[11]!=4 && message[11]!=5)return 0;
 // Keep boot/input/activity evidence without the full framework info stream.
 const char *tag=(const char*)message+12;size_t tag_length=strnlen(tag,length-12);
 if(tag_length==length-12)return 0;
 if(message[11]==5)return tag_length==sizeof("libc")-1 && !memcmp(tag,"libc",sizeof("libc")-1);
 const char *boot_tags[]={"UserController","StorageManagerService","FallbackHome","WindowManager","SystemServer"};
 for(unsigned i=0;i<sizeof(boot_tags)/sizeof(boot_tags[0]);i++)
  if(tag_length==strlen(boot_tags[i]) && !memcmp(tag,boot_tags[i],tag_length))return 1;
 const char *body=tag+tag_length+1;size_t body_length=length-13-tag_length;
 if(tag_length==sizeof("InputReader")-1 && !memcmp(tag,"InputReader",sizeof("InputReader")-1))return 1;
 if(tag_length==sizeof("ActivityTaskManager")-1 && !memcmp(tag,"ActivityTaskManager",sizeof("ActivityTaskManager")-1))
  return memmem(body,body_length,"START u0",8)!=NULL || memmem(body,body_length,"Displayed ",10)!=NULL;
 if(tag_length==sizeof("ActivityManager")-1 && !memcmp(tag,"ActivityManager",sizeof("ActivityManager")-1))
  return memmem(body,body_length,"Start proc ",11)!=NULL || memmem(body,body_length,"BOOT_COMPLETED",14)!=NULL;
 return 0;
}
static void drain_logs(int fd) {
 if(fd<0)return;
 int quiet=getenv("SCARLET_INIT_QUIET")!=NULL;
 for(int count=0;count<128;count++) {unsigned char message[8192];ssize_t n=recv(fd,message,sizeof(message),MSG_DONTWAIT);
  if(n<0)break;
  if(quiet && !important_log(message,(size_t)n))continue;
  printf("SCARLET_INIT_LOGD bytes=%zd ",n);
  for(ssize_t i=0;i<n;i++)if(message[i]<32 || message[i]>=127)message[i]='|';
  fwrite(message,1,(size_t)n,stdout);
  putchar('\n');
 }
}
int main(int argc,char **argv) {
 setvbuf(stdout,NULL,_IONBF,0);
 int observation=20;const char *requested=getenv("SCARLET_INIT_OBSERVE_SECONDS");
 if(requested) {char *end=NULL;long value=strtol(requested,&end,10);if(!*requested || *end || value<0 || (value>0 && value<20) || value>1800)return 16;observation=(int)value;}
 if(!observation && getenv("SCARLET_INIT_JAVA_DUMP"))return 16;
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
 for(unsigned long long tick=0;!observation || tick<(unsigned)observation*10;tick++) {
  if(clock_gettime(CLOCK_MONOTONIC,&now))return 15;
  if(observation && (now.tv_sec-started.tv_sec>observation || (now.tv_sec-started.tv_sec==observation && now.tv_nsec>=started.tv_nsec)))break;
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
  if(tick==40 || tick==80 || tick==200 || tick==240 || tick==280)snapshot();
  if(tick%300==0)capacity_snapshot();
  usleep(100000);
 }
 capacity_snapshot();
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
