#pragma once
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct NetworkStatsInterface {
    char name[32];
    uint64_t rx_packets, rx_bytes, tx_packets, tx_bytes;
};
typedef int (*NetworkStatsVisitor)(const struct NetworkStatsInterface*, void*);
/* Link-layer device counters; no UID/tag or TCP-only attribution is implied. */
int networkstats_backend_visit(NetworkStatsVisitor visitor, void* context);
#ifdef __cplusplus
}
#endif
