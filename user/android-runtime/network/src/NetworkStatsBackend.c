#include "NetworkStatsBackend.h"
#include <errno.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

#define NATIVE_BASE 0x53430000L
#define LIST_LINKS_V1 920
#define READ_LINK_STATISTICS_V1 923
#define MAX_LINKS 1024u

struct LinkInfo {
    uint64_t id, generation;
    char name[32];
    uint8_t mac[6], kind, state;
    uint32_t mtu, reserved;
};
struct LinkStatistics {
    uint32_t version, size;
    uint64_t id, generation;
    uint64_t rx_packets, rx_bytes, tx_packets, tx_bytes;
    uint32_t layer, reserved;
};
_Static_assert(sizeof(struct LinkInfo) == 64, "Native link ABI");
_Static_assert(sizeof(struct LinkStatistics) == 64, "Native statistics ABI");
_Static_assert(offsetof(struct LinkStatistics, rx_packets) == 24, "Native counters offset");

int networkstats_backend_visit(NetworkStatsVisitor visitor, void* context) {
    if (visitor == NULL) return -EINVAL;
    struct LinkInfo* links = NULL;
    size_t capacity = 16;
    long count = -1;
    int collected = 0;
    for (unsigned attempt = 0; attempt < 4; ++attempt) {
        struct LinkInfo* next = realloc(links, capacity * sizeof(*links));
        if (next == NULL) { free(links); return -ENOMEM; }
        links = next;
        errno = 0;
        count = syscall(NATIVE_BASE + LIST_LINKS_V1, links, capacity);
        if (count < 0) { int error = errno ? errno : EIO; free(links); return -error; }
        if ((size_t)count <= capacity) { collected = 1; break; }
        if ((size_t)count > MAX_LINKS) { free(links); return -E2BIG; }
        capacity = (size_t)count;
    }
    if (!collected) { free(links); return -EAGAIN; }
    for (long i = 0; i < count; ++i) {
        if (memchr(links[i].name, 0, sizeof(links[i].name)) == NULL || links[i].name[0] == 0) {
            free(links); return -EPROTO;
        }
        struct LinkStatistics stats = {
            .version = 1, .size = sizeof(stats), .id = links[i].id,
            .generation = links[i].generation,
        };
        errno = 0;
        if (syscall(NATIVE_BASE + READ_LINK_STATISTICS_V1, &stats, sizeof(stats)) < 0) {
            int error = errno ? errno : EIO; free(links); return -error;
        }
        if (stats.version != 1 || stats.size != sizeof(stats) || stats.id != links[i].id ||
            stats.generation != links[i].generation || stats.layer != 1 || stats.reserved != 0) {
            free(links); return -EPROTO;
        }
        struct NetworkStatsInterface record = {
            .rx_packets = stats.rx_packets, .rx_bytes = stats.rx_bytes,
            .tx_packets = stats.tx_packets, .tx_bytes = stats.tx_bytes,
        };
        memcpy(record.name, links[i].name, sizeof(record.name));
        int result = visitor(&record, context);
        if (result != 0) { free(links); return result; }
    }
    free(links);
    return 0;
}
