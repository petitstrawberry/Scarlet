/* Redirect userspace log writes only; raw kernel log reads remain constrained. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <android/log.h>
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <dirent.h>
#include <stdlib.h>
#include <unistd.h>
static int console=-1;
static int zygote_diagnostic;
static struct stat null_identity;
static int have_null;
static void report(const char *format,...) {
 char message[4096];va_list args;va_start(args,format);int n=vsnprintf(message,sizeof(message),format,args);va_end(args);
 if(n<0)return;size_t length=(size_t)n<sizeof(message)?(size_t)n:sizeof(message)-1;
 if(!zygote_diagnostic){if(console>=0)write(console,message,length);return;}
 // No diagnostic descriptor survives between messages or into a Zygote fork.
 int fd=socket(AF_UNIX,SOCK_DGRAM|SOCK_CLOEXEC|SOCK_NONBLOCK,0);if(fd<0)return;
 struct sockaddr_un address={.sun_family=AF_UNIX};strcpy(address.sun_path,"/dev/socket/logdw");
 if(!connect(fd,(struct sockaddr*)&address,sizeof(address)))send(fd,message,length,MSG_DONTWAIT);
 close(fd);
}
static void diagnostic_log(const struct __android_log_message *message) {
 report("SCARLET_INIT_LOG priority=%d tag=%s %s\n",message->priority,message->tag?message->tag:"",message->message?message->message:"");
}
__attribute__((constructor)) static void capture_console(void) {
 const char *program=getprogname();zygote_diagnostic=program && !strcmp(program,"app_process64");
 if(zygote_diagnostic) {
  // Restore stock /dev/null stdio and close only inherited diagnostic consoles.
  int nullfd=open("/dev/null",O_RDWR|O_CLOEXEC);
  if(nullfd>=0){for(int fd=0;fd<3;fd++)dup2(nullfd,fd);if(nullfd>2)close(nullfd);}
  DIR *directory=opendir("/proc/self/fd");
  if(directory){struct dirent *entry;while((entry=readdir(directory))) {
   char *end;long fd=strtol(entry->d_name,&end,10);if(*end || fd<3 || fd==dirfd(directory))continue;
   char path[64],target[128];snprintf(path,sizeof(path),"/proc/self/fd/%ld",fd);
   ssize_t n=readlink(path,target,sizeof(target)-1);if(n<0)continue;target[n]=0;
   if(!strcmp(target,"/dev/tty0"))close((int)fd);
  }closedir(directory);}
 }else {console=fcntl(STDERR_FILENO,F_DUPFD_CLOEXEC,64);have_null=stat("/dev/null",&null_identity)==0;}
 void (*set_logger)(void (*)(const struct __android_log_message*))=dlsym(RTLD_DEFAULT,"__android_log_set_logger");
 if(set_logger)set_logger(diagnostic_log);
 report("SCARLET_INIT_LOGGER_READY uid=%u euid=%u\n",getuid(),geteuid());
}
static int log_open(const char *path,int flags) {
 if(console>=0 && path && !strcmp(path,"/dev/kmsg") && (flags&O_ACCMODE)!=O_RDONLY)
  return fcntl(console,(flags&O_CLOEXEC)?F_DUPFD_CLOEXEC:F_DUPFD,0);
 return -1;
}
int open(const char *path,int flags,...) {
 int fd=log_open(path,flags);if(fd>=0)return fd;
 mode_t mode=0;if((flags&O_CREAT)||((flags&O_TMPFILE)==O_TMPFILE)){va_list args;va_start(args,flags);mode=va_arg(args,int);va_end(args);}
 int (*real)(const char*,int,...)=dlsym(RTLD_NEXT,"open");return real(path,flags,mode);
}
int open64(const char *path,int flags,...) {
 mode_t mode=0;if((flags&O_CREAT)||((flags&O_TMPFILE)==O_TMPFILE)){va_list args;va_start(args,flags);mode=va_arg(args,int);va_end(args);}
 return open(path,flags,mode);
}
int openat(int dir,const char *path,int flags,...) {
 int fd=log_open(path,flags);if(fd>=0)return fd;
 mode_t mode=0;if((flags&O_CREAT)||((flags&O_TMPFILE)==O_TMPFILE)){va_list args;va_start(args,flags);mode=va_arg(args,int);va_end(args);}
 int (*real)(int,const char*,int,...)=dlsym(RTLD_NEXT,"openat");return real(dir,path,flags,mode);
}
int __open_2(const char *path,int flags){return open(path,flags);}
int __openat_2(int dir,const char *path,int flags){return openat(dir,path,flags);}

/* Keep child diagnostic output visible when init installs /dev/null stdio. */
static int diagnostic_output(int oldfd,int newfd) {
 if(console<0 || oldfd==newfd || (newfd!=STDOUT_FILENO && newfd!=STDERR_FILENO))return oldfd;
 struct stat identity;if(have_null && !fstat(oldfd,&identity) && identity.st_dev==null_identity.st_dev && identity.st_ino==null_identity.st_ino && (identity.st_mode&S_IFMT)==(null_identity.st_mode&S_IFMT))return console;
 char path[64],target[128];snprintf(path,sizeof(path),"/proc/self/fd/%d",oldfd);
 ssize_t length=readlink(path,target,sizeof(target)-1);
 if(length<0){if(console>=0)dprintf(console,"SCARLET_INIT_STDIO_FD fd=%d target=%d readlink-errno=%d\n",oldfd,newfd,errno);return oldfd;}target[length]=0;
 if(console>=0)dprintf(console,"SCARLET_INIT_STDIO_FD fd=%d target=%d path=%s\n",oldfd,newfd,target);
 return !strcmp(target,"/dev/null")?console:oldfd;
}
int dup2(int oldfd,int newfd) {
 oldfd=diagnostic_output(oldfd,newfd);
 int (*real)(int,int)=dlsym(RTLD_NEXT,"dup2");return real(oldfd,newfd);
}
int dup3(int oldfd,int newfd,int flags) {
 oldfd=diagnostic_output(oldfd,newfd);
 int (*real)(int,int,int)=dlsym(RTLD_NEXT,"dup3");return real(oldfd,newfd,flags);
}

/* Report failed real exec calls on the retained diagnostic console. */
int execv(const char *path,char *const argv[]) {
 int (*real)(const char*,char *const[])=dlsym(RTLD_NEXT,"execv");
 if(console>=0)dprintf(console,"SCARLET_INIT_EXEC_TRY path=%s uid=%u gid=%u\n",path,getuid(),getgid());
 int result=real(path,argv);int saved=errno;
 if(console>=0)dprintf(console,"SCARLET_INIT_EXEC_FAIL path=%s errno=%d\n",path,saved);
 errno=saved;return result;
}

/* Preserve the real abort path while exposing its message before termination. */
void android_set_abort_message(const char *message) {
 report("SCARLET_INIT_ABORT_MESSAGE %s\n",message?message:"");
 void (*real)(const char*)=dlsym(RTLD_NEXT,"android_set_abort_message");real(message);
}
