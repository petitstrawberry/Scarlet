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
int main(int argc,char **argv) {
 setvbuf(stdout,NULL,_IONBF,0);
 if(argc==2 && !strcmp(argv[1],"inside")) {
  printf("SCARLET_INIT_NAMESPACE_INSIDE pid=%d ppid=%d tid=%ld\n",getpid(),getppid(),syscall(SYS_gettid));
  if(getpid()!=1 || getppid()!=0 || syscall(SYS_gettid)!=1)return 3;
  char *const args[]={"/system/bin/init","second_stage",NULL};
  setenv("LD_PRELOAD","/system/lib64/libprobe-init-kmsg.so",1);
  execv(args[0],args);perror("stock init exec");return 4;
 }
 long scope=syscall(0x53430000L+1320);if(scope<0)return 11;
 pid_t child=fork();if(child<0)return 1;
 if(child==0) {
  // Native CreateNamespace before any seccomp installation. Exec rebuilds the Android ABI state.
  if(syscall(0x5343005c,3,"android-init-diagnostic")!=0){perror("namespace");_exit(2);}
  if(syscall(0x53430000L+1321,scope) || syscall(0x53430000L+1322,scope,0) || syscall(0x53430000L+102,scope))return 12;
  char *const args[]={argv[0],"inside",NULL};execv(args[0],args);_exit(5);
 }
 printf("SCARLET_INIT_NAMESPACE_OUTER child=%d\n",child);
 for(int tick=0;tick<200;tick++) {
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
