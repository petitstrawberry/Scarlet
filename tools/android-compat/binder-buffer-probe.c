/* Reuse the existing bounded Binder pump, reference handshakes and deadlines. */
#include "binder-object-probe.c"
#include <stddef.h>
#include <linux/memfd.h>
#include <sys/stat.h>
#include <sys/vfs.h>

static int binderfs_case(void) {
    struct stat alias,node,opened;
    CHECK(lstat("/dev/hwbinder",&alias)==0 && S_ISLNK(alias.st_mode));
    CHECK(stat("/dev/hwbinder",&alias)==0 && stat("/dev/binderfs/hwbinder",&node)==0);
    CHECK(S_ISCHR(node.st_mode) && alias.st_ino==node.st_ino && alias.st_dev==node.st_dev);
    CHECK(access("/dev/hwbinder",R_OK|W_OK)==0);
    int fd=open("/dev/hwbinder",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
    CHECK(fstat(fd,&opened)==0 && opened.st_ino==node.st_ino && opened.st_dev==node.st_dev && opened.st_rdev==node.st_rdev);
    CHECK(fchmod(fd,0600)==0 && stat("/dev/binderfs/hwbinder",&node)==0 && (node.st_mode&0777)==0600);
    CHECK(chmod("/dev/hwbinder",0666)==0 && fstat(fd,&opened)==0 && (opened.st_mode&0777)==0666);
    struct statfs filesystem;CHECK(fstatfs(fd,&filesystem)==0 && filesystem.f_type==0x6c6f6f70);
    struct binder_version version={0};CHECK(ioctl(fd,BINDER_VERSION,&version)==0 && version.protocol_version==8);
    CHECK(close(fd)==0);puts("SCARLET_ANDROID_BINDERFS_OK");return 0;
}

enum { BUFFER_EXCHANGE=0x801, BUFFER_STOP=0x802 };
struct root_buffer { uintptr_t child; int fd; uint32_t marker; };
struct embedded_packet {
    struct binder_buffer_object root,child;
    struct binder_fd_array_object array;
    struct binder_fd_object direct;
};
static struct binder_transaction_data_sg embedded(struct embedded_packet *packet,
        struct root_buffer *root,int fd,const char *text,binder_size_t offsets[4],uint32_t handle) {
    *root=(struct root_buffer){.child=(uintptr_t)text,.fd=fd,.marker=0xaabbccdd};
    *packet=(struct embedded_packet){
        .root={.hdr.type=BINDER_TYPE_PTR,.buffer=(uintptr_t)root,.length=sizeof(*root)},
        .child={.hdr.type=BINDER_TYPE_PTR,.flags=BINDER_BUFFER_FLAG_HAS_PARENT,
                .buffer=(uintptr_t)text,.length=8,.parent=0,.parent_offset=0},
        .array={.hdr.type=BINDER_TYPE_FDA,.num_fds=1,.parent=0,.parent_offset=offsetof(struct root_buffer,fd)},
        .direct={.hdr.type=BINDER_TYPE_FD,.fd=fd},
    };
    offsets[0]=offsetof(struct embedded_packet,root);offsets[1]=offsetof(struct embedded_packet,child);
    offsets[2]=offsetof(struct embedded_packet,array);offsets[3]=offsetof(struct embedded_packet,direct);
    return (struct binder_transaction_data_sg){.transaction_data={
        .target.handle=handle,.code=BUFFER_EXCHANGE,.flags=TF_ACCEPT_FDS,.data_size=sizeof(*packet),
        .offsets_size=4*sizeof(*offsets),.data.ptr={.buffer=(uintptr_t)packet,.offsets=(uintptr_t)offsets}},
        .buffers_size=sizeof(*root)+8};
}
static int read_embedded(struct object_peer *peer,const struct binder_transaction_data *tx,
                          const char *text,int *automatic,int *ordinary) {
    CHECK(tx->data_size==sizeof(struct embedded_packet) && tx->offsets_size==4*sizeof(binder_size_t));
    CHECK(in_pool(peer,tx->data.ptr.buffer,tx->data_size));
    struct embedded_packet packet;memcpy(&packet,(void *)(uintptr_t)tx->data.ptr.buffer,sizeof(packet));
    CHECK(packet.root.hdr.type==BINDER_TYPE_PTR && packet.child.hdr.type==BINDER_TYPE_PTR);
    CHECK(in_pool(peer,packet.root.buffer,sizeof(struct root_buffer)));
    CHECK(in_pool(peer,packet.child.buffer,8));
    struct root_buffer root;memcpy(&root,(void *)(uintptr_t)packet.root.buffer,sizeof(root));
    CHECK(root.child==packet.child.buffer && root.marker==0xaabbccdd);
    CHECK(!memcmp((void *)root.child,text,8));
    CHECK(root.fd>=0 && packet.direct.hdr.type==BINDER_TYPE_FD);
    CHECK((fcntl(root.fd,F_GETFD)&FD_CLOEXEC) && (fcntl(packet.direct.fd,F_GETFD)&FD_CLOEXEC));
    CHECK(root.fd!=(int)packet.direct.fd);
    *automatic=root.fd;*ordinary=packet.direct.fd;return 0;
}
static int buffer_server(int ready) {
    role="buffer-server";
    struct object_peer peer={.fd=-1,.pool=MAP_FAILED};
    CHECK(peer_open_path(&peer,(1u<<CONTEXT)|(1u<<SERVICE),"/dev/hwbinder")==0);
    struct flat_binder_object context=local_object(CONTEXT,1);context.flags=FLAT_BINDER_FLAG_ACCEPTS_FDS;
    CHECK(ioctl(peer.fd,BINDER_SET_CONTEXT_MGR_EXT,&context)==0);
    CHECK(write(ready,"R",1)==1);close(ready);
    struct binder_transaction_data tx;
    CHECK(next_event(&peer,BR_TRANSACTION,&tx)==0 && tx.code==EXPORT);
    CHECK(free_received(&peer,&tx)==0);
    struct packet export;packet_init(&export,1);
    struct flat_binder_object service=local_object(SERVICE,1);
    service.flags=FLAT_BINDER_FLAG_ACCEPTS_FDS|FLAT_BINDER_FLAG_INHERIT_RT;
    packet_object(&export,0,service);
    CHECK(send_packet(&peer,BC_REPLY,0,EXPORT,&export)==0);
    CHECK(next_event(&peer,BR_TRANSACTION,&tx)==0 && tx.code==BUFFER_EXCHANGE);
    int automatic,ordinary;
    CHECK(read_embedded(&peer,&tx,"REQUEST!",&automatic,&ordinary)==0);
    char bytes[4];CHECK(pread(ordinary,bytes,4,0)==4 && !memcmp(bytes,"DATA",4));
    int retained=dup(automatic);CHECK(retained>=0);
    CHECK(pwrite(retained,"DONE",4,0)==4);
    CHECK(free_received(&peer,&tx)==0);
    CHECK(fcntl(automatic,F_GETFD)==-1 && errno==EBADF);
    CHECK(pread(ordinary,bytes,4,0)==4 && !memcmp(bytes,"DONE",4));
    close(ordinary);
    struct embedded_packet reply;struct root_buffer root;binder_size_t offsets[4];
    struct binder_transaction_data_sg outgoing=embedded(&reply,&root,retained,"REPLIED!",offsets,0);
    CHECK(command(peer.fd,BC_REPLY_SG,&outgoing,sizeof(outgoing))==0);
    close(retained);
    CHECK(next_event(&peer,BR_TRANSACTION,&tx)==0 && tx.code==BUFFER_STOP);
    CHECK(free_received(&peer,&tx)==0 && send_scalar(&peer,BC_REPLY,0,BUFFER_STOP)==0);
    CHECK(peer_close(&peer)==0);return 0;
}
static int buffer_client(void) {
    struct object_peer peer={.fd=-1,.pool=MAP_FAILED};
    CHECK(peer_open_path(&peer,0,"/dev/hwbinder")==0);
    CHECK(send_scalar(&peer,BC_TRANSACTION,0,EXPORT)==0);
    struct binder_transaction_data tx;struct flat_binder_object service;
    CHECK(next_event(&peer,BR_REPLY,&tx)==0 && tx.code==EXPORT);
    CHECK(read_packet(&peer,&tx,1,&service)==0 && is_remote(&service,1)==0);
    CHECK(retain(&peer,service.handle,1)==0 && free_received(&peer,&tx)==0);
    int fd=memfd_create("hidl-fd-transfer",MFD_CLOEXEC);CHECK(fd>=0);
    CHECK(ftruncate(fd,4096)==0 && pwrite(fd,"DATA",4,0)==4);
    struct embedded_packet packet;struct root_buffer root;binder_size_t offsets[4];
    struct binder_transaction_data_sg outgoing=embedded(&packet,&root,fd,"REQUEST!",offsets,service.handle);
    struct binder_transaction_data_sg bad=outgoing;
    bad.buffers_size=1;
    CHECK(command(peer.fd,BC_TRANSACTION_SG,&bad,sizeof(bad))==-1 && errno==E2BIG);
    packet.child.parent=3;
    CHECK(command(peer.fd,BC_TRANSACTION_SG,&outgoing,sizeof(outgoing))==-1 && errno==EINVAL);
    packet.child.parent=0;packet.array.parent_offset=0;
    CHECK(command(peer.fd,BC_TRANSACTION_SG,&outgoing,sizeof(outgoing))==-1 && errno==EINVAL);
    packet.array.parent_offset=offsetof(struct root_buffer,fd);packet.direct.fd=-1;
    CHECK(command(peer.fd,BC_TRANSACTION_SG,&outgoing,sizeof(outgoing))==-1 && errno==EBADF);
    packet.direct.fd=fd;
    CHECK(command(peer.fd,BC_TRANSACTION_SG,&outgoing,sizeof(outgoing))==0);
    close(fd);
    CHECK(next_event(&peer,BR_REPLY,&tx)==0 && tx.code==BUFFER_EXCHANGE);
    int automatic,ordinary;CHECK(read_embedded(&peer,&tx,"REPLIED!",&automatic,&ordinary)==0);
    char bytes[4];CHECK(pread(automatic,bytes,4,0)==4 && !memcmp(bytes,"DONE",4));
    int retained=dup(automatic);CHECK(retained>=0);
    CHECK(free_received(&peer,&tx)==0);
    CHECK(fcntl(automatic,F_GETFD)==-1 && errno==EBADF);
    CHECK(pread(ordinary,bytes,4,0)==4 && !memcmp(bytes,"DONE",4));
    CHECK(pread(retained,bytes,4,0)==4 && !memcmp(bytes,"DONE",4));
    close(ordinary);close(retained);
    CHECK(release(&peer,service.handle,1)==0);
    CHECK(call_scalar(&peer,0,BUFFER_STOP)==0);
    CHECK(peer_close(&peer)==0);return 0;
}
int main(void) {
    CHECK(binderfs_case()==0);
    int ready[2];if(pipe(ready))return 2;
    pid_t child=fork();if(child<0)return 3;
    if (!child) {close(ready[0]);_exit(buffer_server(ready[1]) ? 4 : 0);}
    close(ready[1]);char marker;
    int result=read(ready[0],&marker,1)==1 && marker=='R' ? buffer_client() : -1;
    close(ready[0]);if(result)kill(child,SIGKILL);
    int status;if(waitpid(child,&status,0)!=child || !WIFEXITED(status) || WEXITSTATUS(status))result=-1;
    if(result)return 5;
    puts("SCARLET_ANDROID_BINDER_BUFFERS_OK");return 0;
}
