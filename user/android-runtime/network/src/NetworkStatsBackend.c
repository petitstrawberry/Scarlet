#include "NetworkStatsBackend.h"
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

#define MAX_LINKS 1024u

static int read_counter(int directory, const char* name, uint64_t* out) {
    int fd = openat(directory, name, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return -errno;
    char buffer[32];
    size_t length = 0;
    int result = 0;
    for (;;) {
        ssize_t count = read(fd, buffer + length, sizeof(buffer) - length);
        if (count < 0) {
            if (errno == EINTR) continue;
            result = -errno; break;
        }
        if (count == 0) break;
        length += (size_t)count;
        if (length == sizeof(buffer)) { result = -EOVERFLOW; break; }
    }
    close(fd);
    if (result) return result;
    if (length && buffer[length - 1] == '\n') --length;
    if (!length) return -EPROTO;
    uint64_t value = 0;
    for (size_t i = 0; i < length; ++i) {
        if (buffer[i] < '0' || buffer[i] > '9') return -EPROTO;
        unsigned digit = (unsigned)(buffer[i] - '0');
        if (value > (UINT64_MAX - digit) / 10) return -EOVERFLOW;
        value = value * 10 + digit;
    }
    *out = value;
    return 0;
}

/* The directory parameter also permits real filesystem fixtures in host tests. */
int networkstats_backend_visit_at(const char* path, NetworkStatsVisitor visitor, void* context) {
    if (!path || !visitor) return -EINVAL;
    DIR* directory = opendir(path);
    if (!directory) return -errno;
    int result = 0;
    unsigned count = 0;
    for (;;) {
        errno = 0;
        struct dirent* entry = readdir(directory);
        if (!entry) { if (errno) result = -errno; break; }
        if (!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, "..")) continue;
        if (++count > MAX_LINKS) { result = -E2BIG; break; }
        if (!entry->d_name[0] || strnlen(entry->d_name, 32) == 32) { result = -EPROTO; break; }
        struct NetworkStatsInterface record = {0};
        memcpy(record.name, entry->d_name, strlen(entry->d_name));
        int interface = openat(dirfd(directory), entry->d_name, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        if (interface < 0) { result = -errno; break; }
        int stats = openat(interface, "statistics", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        int error = errno;
        close(interface);
        if (stats < 0) { result = -error; break; }
        result = read_counter(stats, "rx_packets", &record.rx_packets);
        if (!result) result = read_counter(stats, "rx_bytes", &record.rx_bytes);
        if (!result) result = read_counter(stats, "tx_packets", &record.tx_packets);
        if (!result) result = read_counter(stats, "tx_bytes", &record.tx_bytes);
        close(stats);
        if (result) break;
        result = visitor(&record, context);
        if (result) break;
    }
    closedir(directory);
    return result;
}

int networkstats_backend_visit(NetworkStatsVisitor visitor, void* context) {
    return networkstats_backend_visit_at("/sys/class/net", visitor, context);
}
