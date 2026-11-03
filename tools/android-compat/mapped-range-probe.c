/* msync must validate VMAs, not faulted-in pages or the page after the range. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
#include <unistd.h>
#define CHECK(x) do {if(!(x)){perror(#x);return 1;}} while(0)
int main(void) {
    size_t page=(size_t)sysconf(_SC_PAGESIZE);
    char *area=mmap(NULL,4*page,PROT_NONE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    CHECK(area!=MAP_FAILED);
    CHECK(munmap(area+3*page,page)==0);
    CHECK(msync(area,3*page,MS_SYNC)==0);
    CHECK(mprotect(area,3*page,PROT_READ|PROT_WRITE)==0);
    CHECK(msync(area,3*page,MS_SYNC)==0); /* still lazy */
    area[0]=42;
    CHECK(msync(area,1,MS_SYNC)==0);
    CHECK(msync(area,3*page-1,MS_SYNC)==0);
    errno=0;CHECK(msync(area,3*page+1,MS_SYNC)==-1 && errno==ENOMEM);
    CHECK(munmap(area+page,page)==0);
    errno=0;CHECK(msync(area,3*page,MS_SYNC)==-1 && errno==ENOMEM);
    CHECK(msync(area,1,MS_SYNC)==0);
    CHECK(msync(area+2*page,page,MS_SYNC)==0);
    CHECK(munmap(area,page)==0);
    CHECK(munmap(area+2*page,page)==0);
    puts("SCARLET_MAPPED_RANGE_OK");
    return 0;
}
