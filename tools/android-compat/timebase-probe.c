/* Bionic must receive an accounting unit through Linux process-entry metadata. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/auxv.h>
#include <time.h>
#include <unistd.h>

int main(void) {
    unsigned long auxiliary=getauxval(AT_CLKTCK);
    long configured=sysconf(_SC_CLK_TCK);
    if(!auxiliary || configured<=0 || (unsigned long)configured!=auxiliary) {
        printf("SCARLET_TIMEBASE_MISSING aux=%lu sysconf=%ld\n",auxiliary,configured);
        return 2;
    }
    struct timespec value;
    if(clock_gettime(CLOCK_MONOTONIC,&value) || value.tv_nsec<0 || value.tv_nsec>=1000000000L)return 3;
    printf("SCARLET_TIMEBASE_OK accounting_hz=%ld monotonic_ns=%lld\n",configured,(long long)value.tv_sec*1000000000LL+value.tv_nsec);
    return 0;
}
