/* Redirect userspace log writes only; raw kernel log reads remain constrained. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
static int console=-1;
__attribute__((constructor)) static void capture_console(void) {console=fcntl(STDERR_FILENO,F_DUPFD_CLOEXEC,64);}
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
 char path[64],target[128];snprintf(path,sizeof(path),"/proc/self/fd/%d",oldfd);
 ssize_t length=readlink(path,target,sizeof(target)-1);
 if(length<0)return oldfd;target[length]=0;
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
