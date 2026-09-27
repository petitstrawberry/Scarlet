#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <unistd.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "SCARLET_CROSVM_FAIL: line %d: %s (errno %d)\n", __LINE__, #x, errno); return 1; } } while (0)

int main(void) {
    int fd = open("/guest/positional-io", O_CREAT | O_TRUNC | O_RDWR, 0600);
    CHECK(fd >= 0);
    unsigned char *source = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    unsigned char *dest = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(source != MAP_FAILED && dest != MAP_FAILED);
    for (size_t i = 0; i < 16384; ++i) source[i] = (i * 37 + 11) & 255;
    struct iovec out[] = {{source + 4090, 4110}, {NULL, 0}, {source + 17, 3073}};
    struct iovec in[] = {{dest + 4090, 4110}, {NULL, 0}, {dest + 17, 3073}};
    CHECK(lseek(fd, 5, SEEK_SET) == 5);
    CHECK(pwritev(fd, out, 3, 123) == 7183);
    CHECK(lseek(fd, 0, SEEK_CUR) == 5);
    CHECK(preadv(fd, in, 3, 123) == 7183);
    CHECK(lseek(fd, 0, SEEK_CUR) == 5);
    CHECK(memcmp(source + 4090, dest + 4090, 4110) == 0);
    CHECK(memcmp(source + 17, dest + 17, 3073) == 0);
    CHECK(preadv(fd, in, 3, 123 + 7183 - 7) == 7);
    CHECK(preadv(fd, in, 3, 123 + 7183) == 0);
    errno = 0; CHECK(preadv(fd, in, 3, -1) == -1 && errno == EINVAL);
    errno = 0; CHECK(syscall(SYS_preadv, fd, in, -1, 0, 0) == -1 && errno == EINVAL);
    int ro = open("/guest/positional-io", O_RDONLY);
    CHECK(ro >= 0);
    errno = 0; CHECK(pwritev(ro, out, 3, 0) == -1 && errno == EBADF);
    close(ro);
    int sockets[2];
    CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, sockets) == 0);
    struct iovec payload = {(void *)"hello", 5};
    union { struct cmsghdr align; char bytes[CMSG_SPACE(sizeof(int))]; } control = {0};
    struct msghdr msg = {.msg_iov = &payload, .msg_iovlen = 1, .msg_control = control.bytes, .msg_controllen = sizeof(control)};
    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    cmsg->cmsg_level = SOL_SOCKET; cmsg->cmsg_type = SCM_RIGHTS; cmsg->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(cmsg), &fd, sizeof(fd));
    CHECK(sendmsg(sockets[0], &msg, 0) == 5);
    CHECK(recv(sockets[1], NULL, 0, MSG_PEEK | MSG_TRUNC) == 5);
    char prefix[2];
    CHECK(recv(sockets[1], prefix, sizeof(prefix), MSG_PEEK | MSG_TRUNC) == 5);
    CHECK(memcmp(prefix, "he", 2) == 0);
    char received[5]; payload.iov_base = received;
    memset(control.bytes, 0, sizeof(control)); msg.msg_controllen = sizeof(control);
    CHECK(recvmsg(sockets[1], &msg, MSG_CMSG_CLOEXEC) == 5);
    CHECK(memcmp(received, "hello", 5) == 0);
    cmsg = CMSG_FIRSTHDR(&msg); CHECK(cmsg && cmsg->cmsg_type == SCM_RIGHTS);
    int passed_fd; memcpy(&passed_fd, CMSG_DATA(cmsg), sizeof(passed_fd));
    CHECK(fcntl(passed_fd, F_GETFD) & FD_CLOEXEC);
    CHECK(preadv(passed_fd, in, 3, 123) == 7183);
    close(passed_fd); close(sockets[0]); close(sockets[1]); close(fd);
    unlink("/guest/positional-io");
    puts("SCARLET_POSITIONAL_IO_OK");
    return 0;
}
