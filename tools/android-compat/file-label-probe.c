/* Native inode labels exercised through stock Bionic's Android ABI. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/xattr.h>
#include <unistd.h>
#define CHECK(x) do {if(!(x)){printf("SCARLET_FILE_LABEL_FAIL line=%d errno=%d\n",__LINE__,errno);return 1;}}while(0)
static const char *base="u:object_r:unlabeled:s0",*data="u:object_r:system_data_file:s0",*chosen="u:object_r:build_prop:s0";
static int context(const char *path,const char *expected) {char label[256];ssize_t n=getxattr(path,"security.selinux",label,sizeof(label));return n==(ssize_t)strlen(expected)+1 && !memcmp(label,expected,(size_t)n);}
static int select_label(const char *label) {int fd=open("/proc/thread-self/attr/fscreate",O_WRONLY|O_CLOEXEC);if(fd<0)return -1;size_t n=label?strlen(label)+1:0;int result=write(fd,label,n)==(ssize_t)n?0:-1;int saved=errno;close(fd);errno=saved;return result;}
static int exercise(const char *root) {
 char path[256],alias[256],renamed[256];snprintf(path,sizeof(path),"%s/file",root);snprintf(alias,sizeof(alias),"%s/alias",root);snprintf(renamed,sizeof(renamed),"%s/renamed",root);
 CHECK(mkdir(root,0700)==0);int fd=open(path,O_CREAT|O_EXCL|O_RDWR,0600);CHECK(fd>=0 && context(path,base));
 CHECK(setxattr(path,"security.selinux",data,strlen(data)+1,0)==0 && context(path,data));
 struct stat owned;CHECK(chown(path,0x12345,0x23456)==0 && fstat(fd,&owned)==0 && owned.st_uid==0x12345 && owned.st_gid==0x23456 && chown(path,0,0)==0);
 const char *forbidden="u:object_r:servicemanager_exec:s0";CHECK(setxattr(path,"security.selinux",forbidden,strlen(forbidden)+1,0)==-1 && errno==EACCES && context(path,data));
 CHECK(link(path,alias)==0 && rename(path,renamed)==0 && context(alias,data));
 CHECK(unlink(alias)==0 && unlink(renamed)==0);char retained[256];CHECK(fgetxattr(fd,"security.selinux",retained,sizeof(retained))==(ssize_t)strlen(data)+1 && !strcmp(retained,data));
 CHECK(close(fd)==0);fd=open(path,O_CREAT|O_EXCL|O_RDWR,0600);CHECK(fd>=0 && context(path,base) && close(fd)==0);
 CHECK(select_label("u:object_r:property_socket:s0")==0);int denied=open(alias,O_CREAT|O_EXCL|O_RDWR,0600);CHECK(denied==-1 && errno==EACCES && access(alias,F_OK)==-1 && errno==ENOENT);
 CHECK(select_label(chosen)==0);fd=open(alias,O_CREAT|O_EXCL|O_RDWR,0600);CHECK(fd>=0 && context(alias,chosen) && close(fd)==0);
 CHECK(select_label(NULL)==0);
 CHECK(unlink(path)==0);
 CHECK(unlink(alias)==0);
 CHECK(rmdir(root)==0);return 0;
}
int main(int argc,char **argv) {
 if(argc==2 && !strcmp(argv[1],"check-clean")) {char label[256];int fd=open("/proc/self/attr/fscreate",O_RDONLY);CHECK(fd>=0 && read(fd,label,sizeof(label))==0 && close(fd)==0);return 0;}
 CHECK(exercise("/tmp/native-labels")==0);CHECK(exercise("/data/local/tmp/native-labels")==0);
 CHECK(select_label(chosen)==0);CHECK(execl("/definitely-missing-label-probe","missing",NULL)==-1 && errno==ENOENT);pid_t child=fork();CHECK(child>=0);if(!child){execl(argv[0],argv[0],"check-clean",NULL);_exit(70);}int status;CHECK(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
 char label[256];int fd=open("/proc/self/attr/fscreate",O_RDONLY);CHECK(fd>=0 && read(fd,label,sizeof(label))==(ssize_t)strlen(chosen)+1 && !strcmp(label,chosen) && close(fd)==0);CHECK(select_label(NULL)==0);
 printf("SCARLET_FILE_LABEL_OK tmpfs ext2 owner32 hardlink rename open-unlink denied-relabel create-kind exec-clear\n");return 0;
}
