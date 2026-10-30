/* The proc projection must agree with native-backed Linux sysinfo units. */
#define _GNU_SOURCE
#include <stdio.h>
#include <string.h>
#include <sys/sysinfo.h>

int main(void) {
    struct sysinfo info;
    if(sysinfo(&info))return 1;
    FILE *file=fopen("/proc/meminfo","re");if(!file)return 2;
    unsigned long long total=0,free=0;char line[256];
    while(fgets(line,sizeof(line),file)) {
        (void)sscanf(line,"MemTotal: %llu kB",&total);
        (void)sscanf(line,"MemFree: %llu kB",&free);
    }
    fclose(file);
    unsigned long long native_total=(unsigned long long)info.totalram*info.mem_unit/1024;
    if(!total || total!=native_total || free>total)return 3;
    printf("SCARLET_MEMINFO_OK total_kib=%llu free_kib=%llu\n",total,free);
    return 0;
}
