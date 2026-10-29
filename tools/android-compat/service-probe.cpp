// Real IServiceManager calls through unmodified libbinder and stock servicemanager.
#include <binder/Binder.h>
#include <binder/IServiceManager.h>
#include <binder/IPCThreadState.h>
#include <binder/Parcel.h>
#include <binder/ProcessState.h>
#include <utils/String16.h>
#include <android/log.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <poll.h>
#include <unistd.h>
#include <cerrno>
#include <cstdio>
#include <cstring>
using namespace android;
#define CHECK(x) do {if (!(x)) {dprintf(2,"SCARLET_SERVICES_FAIL pid=%d line=%d %s errno=%d\n",getpid(),__LINE__,#x,errno);return 1;}} while(0)
static const String16 service("scarlet.compat.service");
static const String16 denied_service("scarlet.compat.denied");
static const String16 descriptor("scarlet.compat.Service");
static bool exists(const char* path) {return access(path,F_OK)==0;}
static int mark(const char* path) {int fd=open(path,O_CREAT|O_WRONLY|O_EXCL|O_CLOEXEC,0600);CHECK(fd>=0);CHECK(close(fd)==0);return 0;}
static int await(const char* path) {for(int i=0;i<500;++i){if(exists(path))return 0;usleep(10000);}CHECK(false);}
class Service:public BBinder {
 public: const String16& getInterfaceDescriptor() const override {return descriptor;}
 protected:status_t onTransact(uint32_t code,const Parcel& data,Parcel* reply,uint32_t flags) override {
    if(code!=IBinder::FIRST_CALL_TRANSACTION)return BBinder::onTransact(code,data,reply,flags);
    if(!data.enforceInterface(descriptor))return PERMISSION_DENIED;
    int32_t value=0;if(data.readInt32(&value)!=OK || data.dataAvail()!=0)return BAD_VALUE;
    return reply->writeInt32(value+1);
 }
};
static int owner(const sp<IServiceManager>& sm) {
    CHECK(sm->checkService(String16("manager"))!=nullptr);
    auto object=sp<Service>::make();
    CHECK(sm->addService(service,object)==OK);
    dprintf(1,"SCARLET_SERVICES_REGISTERED\n");
    CHECK(mark("/tmp/service-ready")==0);
    int fd=-1;CHECK(IPCThreadState::self()->setupPolling(&fd)==OK);
    for(int i=0;i<200;++i) {
        if(exists("/tmp/service-client-done") && exists("/tmp/service-denied-done")) {
            dprintf(1,"SCARLET_SERVICES_OWNER_EXITING\n");return 0;
        }
        pollfd event={fd,POLLIN,0};int n=poll(&event,1,50);
        if(n<0 && errno==EINTR)continue;
        CHECK(n>=0 && !(event.revents&(POLLHUP|POLLERR|POLLNVAL)));
        if(n)CHECK(IPCThreadState::self()->handlePolledCommands()==OK);
    }
    CHECK(false);
}
static int client(const sp<IServiceManager>& sm) {
    CHECK(await("/tmp/service-ready")==0);
    auto object=sm->checkService(service);CHECK(object!=nullptr && object->remoteBinder()!=nullptr);
    Parcel data,reply;CHECK(data.writeInterfaceToken(descriptor)==OK && data.writeInt32(41)==OK);
    CHECK(object->transact(IBinder::FIRST_CALL_TRANSACTION,data,&reply)==OK);
    CHECK(reply.readInt32()==42 && reply.dataAvail()==0);
    bool found=false;for(const auto& name:sm->listServices())found |= name==service;
    CHECK(found);
    dprintf(1,"SCARLET_SERVICES_LOOKUP_CALL_LIST_OK\n");
    CHECK(mark("/tmp/service-client-done")==0);
    CHECK(await("/tmp/service-denied-done")==0);
    for(int i=0;i<500;++i) {
        if(sm->checkService(service)==nullptr) {
            CHECK(object->transact(IBinder::FIRST_CALL_TRANSACTION,data,&reply)==DEAD_OBJECT);
            dprintf(1,"SCARLET_SERVICES_OWNER_DEATH_OK\n");return 0;
        }
        usleep(10000);
    }
    CHECK(false);
}
static int denied(const sp<IServiceManager>& sm) {
    CHECK(await("/tmp/service-ready")==0);
    CHECK(sm->checkService(service)==nullptr);
    CHECK(sm->addService(denied_service,sp<Service>::make())==PERMISSION_DENIED);
    CHECK(sm->listServices().empty());
    dprintf(1,"SCARLET_SERVICES_DENIED_ADD_FIND_LIST\n");
    CHECK(mark("/tmp/service-denied-done")==0);return 0;
}
int main(int argc,char** argv) {
    CHECK(argc==2);
    __android_log_set_logger(__android_log_stderr_logger);
    Dl_info library={};CHECK(dladdr((void*)defaultServiceManager,&library) && strcmp(library.dli_fname,"/system/lib64/libbinder.so")==0);
    auto process=ProcessState::self();CHECK(process->setThreadPoolMaxThreadCount(0)==OK);
    auto sm=defaultServiceManager();CHECK(sm!=nullptr);
    dprintf(1,"SCARLET_SERVICES_PEER %s\n",argv[1]);
    if(!strcmp(argv[1],"owner"))return owner(sm);
    if(!strcmp(argv[1],"client"))return client(sm);
    if(!strcmp(argv[1],"denied"))return denied(sm);
    CHECK(false);
}
