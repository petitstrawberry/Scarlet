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
#include <dirent.h>
int main(int argc,char **argv) {
 setvbuf(stdout,NULL,_IONBF,0);
 if(argc==2 && !strcmp(argv[1],"inside")) {
  printf("SCARLET_INIT_NAMESPACE_INSIDE pid=%d ppid=%d tid=%ld\n",getpid(),getppid(),syscall(SYS_gettid));
  if(getpid()!=1 || getppid()!=0 || syscall(SYS_gettid)!=1)return 3;
  // Discard only this disposable fixture's property snapshot; stock init owns initialization.
  DIR *properties=opendir("/dev/__properties__");if(!properties)return 9;
  struct dirent *entry;while((entry=readdir(properties))) {
   if(!strcmp(entry->d_name,".")||!strcmp(entry->d_name,".."))continue;
   char path[512];snprintf(path,sizeof(path),"/dev/__properties__/%s",entry->d_name);
   if(unlink(path)){perror("remove fixture property");return 10;}
  }
  closedir(properties);puts("SCARLET_INIT_PROPERTY_SNAPSHOT_REMOVED");
  char *const args[]={"/system/bin/init","second_stage",NULL};
  setenv("LD_PRELOAD","/system/lib64/libprobe-init-kmsg.so",1);
  execv(args[0],args);perror("stock init exec");return 4;
 }
 pid_t child=fork();if(child<0)return 1;
 if(child==0) {
  // Native CreateNamespace before any seccomp installation. Exec rebuilds the Android ABI state.
  if(syscall(0x5343005c,3,"android-init-diagnostic")!=0){perror("namespace");_exit(2);}
  char *const args[]={argv[0],"inside",NULL};execv(args[0],args);_exit(5);
 }
 printf("SCARLET_INIT_NAMESPACE_OUTER child=%d\n",child);
 for(int tick=0;tick<200;tick++) {
  int status;pid_t got=waitpid(child,&status,WNOHANG);
  if(got==child){printf("SCARLET_INIT_NAMESPACE_REAP %d %d\n",got,status);return 0;}
  if(got<0){perror("outer wait");return 6;}
  usleep(100000);
 }
 if(kill(child,SIGKILL)){perror("owned init stop");return 7;}
 int status;if(waitpid(child,&status,0)!=child)return 8;
 printf("SCARLET_INIT_NAMESPACE_STOP_REAP %d %d\n",child,status);return 0;
}
