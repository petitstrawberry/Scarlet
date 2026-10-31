/* Syscall output must respect private COW and discontiguous user pages. */
#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#define CHECK(x) do {if(!(x)){perror(#x);return 1;}} while(0)
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);size_t page=(size_t)sysconf(_SC_PAGESIZE);
    unsigned char *bytes=mmap(NULL,2*page,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);CHECK(bytes!=MAP_FAILED);
    memset(bytes,0xa5,2*page);pid_t child=fork();CHECK(child>=0);
    if(!child)_exit(syscall(__NR_clock_gettime,CLOCK_MONOTONIC,bytes+128)?42:0);
    int status;CHECK(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
    for(size_t i=0;i<2*page;i++)if(bytes[i]!=0xa5) {
        printf("SCARLET_COPYOUT_ISOLATION_LEAK clock_gettime offset=%zu\n",i);return 1;
    }
    CHECK(syscall(__NR_clock_gettime,CLOCK_MONOTONIC,bytes+page-8)==0);
    struct timespec value;memcpy(&value,bytes+page-8,sizeof(value));
    CHECK(value.tv_sec>=0 && value.tv_nsec>=0 && value.tv_nsec<1000000000);
    CHECK(munmap(bytes,2*page)==0);
    puts("SCARLET_COPYOUT_OK clock_gettime cow cross-page");return 0;
}
