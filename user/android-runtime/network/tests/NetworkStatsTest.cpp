#include "NetworkStatsPublic.h"
#include "NetworkStatsBackend.h"
#include <cerrno>
#include <climits>
#include <cstdio>
#include <cstring>

static int failure;
static NetworkStatsInterface records[] = {
    {"eth0", 3, 300, 4, 400}, {"wifi0", 5, 500, 6, 600},
};
extern "C" int networkstats_backend_visit(NetworkStatsVisitor visit, void* context) {
    for (auto& record : records) {
        int result = visit(&record, context);
        if (result) return result;
        if (failure) return failure;
    }
    return 0;
}
#define CHECK(condition) do { if (!(condition)) { \
    std::fprintf(stderr, "NetworkStatsTest:%d: %s\n", __LINE__, #condition); return 1; \
} } while (0)
int main() {
    using namespace android::bpf;
    Stats stats{};
    CHECK(bpfGetIfaceStats("eth0", &stats) == 0);
    CHECK(stats.rxBytes == 300 && stats.rxPackets == 3);
    CHECK(stats.txBytes == 400 && stats.txPackets == 4);
    CHECK(stats.tcpRxPackets == UINT64_MAX && stats.tcpTxPackets == UINT64_MAX);
    CHECK(bpfGetIfaceStats(nullptr, &stats) == 0);
    CHECK(stats.rxBytes == 800 && stats.rxPackets == 8);
    CHECK(stats.txBytes == 1000 && stats.txPackets == 10);
    Stats saved = stats;
    CHECK(bpfGetIfaceStats("missing", &stats) == -ENODEV);
    CHECK(!std::memcmp(&stats, &saved, sizeof(stats)));
    CHECK(bpfGetUidStats(10000, &stats) == -EOPNOTSUPP);
    CHECK(!std::memcmp(&stats, &saved, sizeof(stats)));
    failure = -EIO;
    CHECK(bpfGetIfaceStats(nullptr, &stats) == -EIO);
    CHECK(!std::memcmp(&stats, &saved, sizeof(stats)));
    std::vector<stats_line> lines;
    CHECK(parseBpfNetworkStatsDev(&lines) == -EIO && lines.empty());
    failure = 0;
    CHECK(parseBpfNetworkStatsDev(&lines) == 0 && lines.size() == 2);
    CHECK(lines[0].uid == UINT32_MAX && lines[0].set == UINT32_MAX && lines[0].tag == 0);
    CHECK(lines[0].rxBytes == 300 && lines[1].txBytes == 600);
    CHECK(parseBpfNetworkStatsDetail(&lines, {}, -1, -1) == -EOPNOTSUPP);
    CHECK(lines.size() == 2);
    lines.push_back(lines[0]);
    groupNetworkStats(&lines);
    CHECK(lines.size() == 2 && lines[0].rxBytes == 600);
    records[0].rx_bytes = INT64_MAX;
    CHECK(bpfGetIfaceStats(nullptr, &stats) == -EOVERFLOW);
    CHECK(!std::memcmp(&stats, &saved, sizeof(stats)));
    CHECK(combineUidTag(10000, 42) == ((uint64_t{10000} << 32) | 42));
    return 0;
}
