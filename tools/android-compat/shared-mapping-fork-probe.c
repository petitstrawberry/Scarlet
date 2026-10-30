/* File offsets and lazy shared pages must survive fork and a leading unmap. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHECK(value) do { if (!(value)) { perror(#value); return 1; } } while (0)
static int child_matches(const unsigned char *mapping,const unsigned char *expected,size_t size) {
    pid_t child=fork();if(child<0)return -1;
    if(!child)_exit(memcmp(mapping,expected,size)?41:0);
    int status;if(waitpid(child,&status,0)!=child)return -1;
    return WIFEXITED(status) && WEXITSTATUS(status)==0?0:-1;
}
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);
    size_t page=(size_t)sysconf(_SC_PAGESIZE),length=3*page;
    int fd=open("/system/framework/framework-res.apk",O_RDONLY|O_CLOEXEC);
    CHECK(fd>=0);
    unsigned char *expected=malloc(length);CHECK(expected);
    CHECK(pread(fd,expected,length,(off_t)page)==(ssize_t)length);
    unsigned char *mapping=mmap(NULL,length,PROT_READ,MAP_SHARED,fd,(off_t)page);
    CHECK(mapping!=MAP_FAILED);close(fd);
    CHECK(*(volatile unsigned char*)mapping==expected[0]);
    if(child_matches(mapping,expected,length) || memcmp(mapping,expected,length)) {
        puts("SCARLET_SHARED_FORK_FAIL nonzero-offset");return 2;
    }
    CHECK(munmap(mapping,page)==0);
    if(child_matches(mapping+page,expected+page,length-page) || memcmp(mapping+page,expected+page,length-page)) {
        puts("SCARLET_SHARED_FORK_FAIL leading-unmap");return 3;
    }
    CHECK(munmap(mapping+page,length-page)==0);free(expected);
    puts("SCARLET_SHARED_FORK_OK nonzero-offset lazy-pages descriptor-close parent-child leading-unmap");
    return 0;
}
