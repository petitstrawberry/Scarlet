/* Check mmap placement and process inheritance against the native VM state. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#define CHECK(x) do {if(!(x)){printf("SCARLET_LAYOUT_FAIL line=%d errno=%d\n",__LINE__,errno);return 1;}}while(0)
struct layout {uint32_t native_bits,low_bits;uint64_t mmap_base;};
static int query(struct layout *layout) {return (int)syscall(0x53430000L+1351,layout);}
static int transfer(int fd,void *data,size_t size,int output) {size_t offset=0;while(offset<size){ssize_t n=output?write(fd,(char*)data+offset,size-offset):read(fd,(char*)data+offset,size-offset);if(n<0 && errno==EINTR)continue;if(n<=0)return -1;offset+=(size_t)n;}return 0;}
int main(int argc,char **argv) {
 struct layout initial;CHECK(query(&initial)==0);
 if(argc==3 && !strcmp(argv[1],"report")) {CHECK(transfer(atoi(argv[2]),&initial,sizeof(initial),1)==0);return 0;}
 CHECK(initial.native_bits>=24 && initial.native_bits<=33 && initial.low_bits==16 && !(initial.mmap_base&4095));
 void *mapping=mmap(NULL,4096,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);CHECK(mapping!=MAP_FAILED && (uintptr_t)mapping>=initial.mmap_base);CHECK(munmap(mapping,4096)==0);
 void *fixed=(void*)(uintptr_t)0x30000000;mapping=mmap(fixed,4096,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED_NOREPLACE,-1,0);CHECK(mapping==fixed && munmap(mapping,4096)==0);
 int varied=0;
 for(int iteration=0;iteration<4;iteration++) {
  int channel[2];CHECK(pipe(channel)==0);pid_t child=fork();CHECK(child>=0);
  if(!child) {close(channel[0]);struct layout inherited;if(query(&inherited) || memcmp(&inherited,&initial,sizeof(initial)))_exit(2);char descriptor[32];snprintf(descriptor,sizeof(descriptor),"%d",channel[1]);execl(argv[0],argv[0],"report",descriptor,NULL);_exit(3);}
  close(channel[1]);struct layout next;CHECK(transfer(channel[0],&next,sizeof(next),0)==0 && close(channel[0])==0);int status;pid_t waited;do{waited=waitpid(child,&status,0);}while(waited<0 && errno==EINTR);CHECK(waited==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
  CHECK(next.native_bits==initial.native_bits && next.low_bits==initial.low_bits && !(next.mmap_base&4095));varied|=next.mmap_base!=initial.mmap_base;
 }
 CHECK(varied);printf("SCARLET_LAYOUT_OK native-policy mmap fixed-address fork-preserve exec-resample\n");return 0;
}
