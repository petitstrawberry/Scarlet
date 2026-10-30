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
#include <unistd.h>
static int console=-1;
static struct stat null_identity;
static int have_null;
static void diagnostic_log(const struct __android_log_message *message) {
 if(console>=0)dprintf(console,"SCARLET_INIT_LOG priority=%d tag=%s %s\n",message->priority,message->tag?message->tag:"",message->message?message->message:"");
}
__attribute__((constructor)) static void capture_console(void) {
 console=fcntl(STDERR_FILENO,F_DUPFD_CLOEXEC,64);have_null=stat("/dev/null",&null_identity)==0;
 void (*set_logger)(void (*)(const struct __android_log_message*))=dlsym(RTLD_DEFAULT,"__android_log_set_logger");
 if(set_logger)set_logger(diagnostic_log);
 if(console>=0)dprintf(console,"SCARLET_INIT_LOGGER_READY uid=%u euid=%u\n",getuid(),geteuid());
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
