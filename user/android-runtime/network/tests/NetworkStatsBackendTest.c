#include "NetworkStatsBackend.h"
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int networkstats_backend_visit_at(const char*, NetworkStatsVisitor, void*);
static char root[] = "/tmp/scarlet-networkstats-XXXXXX";
static const char* names[] = {"rx_packets", "rx_bytes", "tx_packets", "tx_bytes"};
static void cleanup(void) {
    char path[256];
    for (size_t i=0;i<4;++i) {
        snprintf(path,sizeof(path),"%s/eth0/statistics/%s",root,names[i]); unlink(path);
    }
    snprintf(path,sizeof(path),"%s/eth0/statistics",root); rmdir(path);
    snprintf(path,sizeof(path),"%s/eth0",root); rmdir(path);
    rmdir(root);
}
static int write_value(const char* name, const char* value) {
    char path[256]; snprintf(path,sizeof(path),"%s/eth0/statistics/%s",root,name);
    FILE* file=fopen(path,"w"); if (!file) return -1;
    int result=fputs(value,file); if(fclose(file))return -1; return result<0?-1:0;
}
struct Capture { unsigned count; struct NetworkStatsInterface last; int error; };
static int collect(const struct NetworkStatsInterface* record, void* context) {
    struct Capture* capture=context; ++capture->count; capture->last=*record; return capture->error;
}
#define CHECK(test) do { if (!(test)) { fprintf(stderr,"NetworkStatsBackendTest:%d: %s\n",__LINE__,#test); return 1; } } while(0)
int main(void) {
    CHECK(mkdtemp(root)!=NULL); CHECK(atexit(cleanup)==0);
    struct Capture capture={0};
    CHECK(networkstats_backend_visit_at(root,collect,&capture)==0 && capture.count==0);
    CHECK(networkstats_backend_visit_at(root,NULL,&capture)==-EINVAL);
    char path[256]; snprintf(path,sizeof(path),"%s/eth0",root); CHECK(mkdir(path,0700)==0);
    snprintf(path,sizeof(path),"%s/eth0/statistics",root); CHECK(mkdir(path,0700)==0);
    CHECK(write_value("rx_packets","11\n")==0); CHECK(write_value("rx_bytes","1100\n")==0);
    CHECK(write_value("tx_packets","12\n")==0); CHECK(write_value("tx_bytes","1200\n")==0);
    CHECK(networkstats_backend_visit_at(root,collect,&capture)==0 && capture.count==1);
    CHECK(!strcmp(capture.last.name,"eth0"));
    CHECK(capture.last.rx_packets==11 && capture.last.rx_bytes==1100);
    CHECK(capture.last.tx_packets==12 && capture.last.tx_bytes==1200);
    capture.count=0; CHECK(write_value("rx_bytes","18446744073709551615\n")==0);
    CHECK(networkstats_backend_visit_at(root,collect,&capture)==0 && capture.last.rx_bytes==UINT64_MAX);
    for (size_t i=0;i<3;++i) {
        const char* values[]={"18446744073709551616\n","-1\n","12 extra\n"};
        int errors[]={EOVERFLOW,EPROTO,EPROTO};
        capture.count=0; CHECK(write_value("rx_bytes",values[i])==0);
        CHECK(networkstats_backend_visit_at(root,collect,&capture)==-errors[i]); CHECK(capture.count==0);
    }
    CHECK(write_value("rx_bytes","1100\n")==0); capture.error=-ECANCELED;
    CHECK(networkstats_backend_visit_at(root,collect,&capture)==-ECANCELED);
    capture.error=0; capture.count=0;
    snprintf(path,sizeof(path),"%s/eth0/statistics/tx_bytes",root); CHECK(unlink(path)==0);
    CHECK(networkstats_backend_visit_at(root,collect,&capture)==-ENOENT && capture.count==0);
    return 0;
}
