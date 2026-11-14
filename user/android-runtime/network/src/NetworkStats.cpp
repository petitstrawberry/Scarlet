/*
 * Copyright (C) 2017 The Android Open Source Project
 * Licensed under the Apache License, Version 2.0.
 * Scarlet backend for the pinned AOSP libnetworkstats public contract.
 */
#include "NetworkStatsPublic.h"
#include "NetworkStatsBackend.h"
#include <algorithm>
#include <cerrno>
#include <climits>
#include <cstddef>
#include <cstring>

static_assert(sizeof(Stats) == 48);
static_assert(sizeof(android::bpf::stats_line) == 80);
static_assert(offsetof(android::bpf::stats_line, rxBytes) == 48);

namespace android::bpf {
namespace {
struct Sum { const char* name; Stats stats{}; bool found = false; };
int sum_interface(const NetworkStatsInterface* in, void* context) {
    auto& sum = *static_cast<Sum*>(context);
    if (sum.name && strcmp(sum.name, in->name)) return 0;
    sum.found = true;
    for (const auto& pair : {std::pair{&sum.stats.rxBytes, in->rx_bytes},
                            std::pair{&sum.stats.rxPackets, in->rx_packets},
                            std::pair{&sum.stats.txBytes, in->tx_bytes},
                            std::pair{&sum.stats.txPackets, in->tx_packets}}) {
        if (pair.second > static_cast<uint64_t>(INT64_MAX) - *pair.first) return -EOVERFLOW;
        *pair.first += pair.second;
    }
    return 0;
}
int append_interface(const NetworkStatsInterface* in, void* context) {
    if (in->rx_bytes > INT64_MAX || in->rx_packets > INT64_MAX ||
        in->tx_bytes > INT64_MAX || in->tx_packets > INT64_MAX) return -EOVERFLOW;
    stats_line line{};
    memcpy(line.iface, in->name, sizeof(line.iface));
    line.uid = UINT32_MAX; line.set = UINT32_MAX; line.tag = 0;
    line.rxBytes = in->rx_bytes; line.rxPackets = in->rx_packets;
    line.txBytes = in->tx_bytes; line.txPackets = in->tx_packets;
    static_cast<std::vector<stats_line>*>(context)->push_back(line);
    return 0;
}
}

int bpfGetUidStats(uid_t, Stats*) { return -EOPNOTSUPP; }
int parseBpfNetworkStatsDetail(std::vector<stats_line>*, const std::vector<std::string>&, int, int) {
    return -EOPNOTSUPP;
}
int bpfGetIfaceStats(const char* iface, Stats* out) {
    if (!out || (iface && (!*iface || strnlen(iface, 32) == 32))) return -EINVAL;
    Sum sum{iface};
    int result = networkstats_backend_visit(sum_interface, &sum);
    if (result) return result;
    if (iface && !sum.found) return -ENODEV;
    sum.stats.tcpRxPackets = sum.stats.tcpTxPackets = UINT64_MAX;
    *out = sum.stats;
    return 0;
}
int parseBpfNetworkStatsDev(std::vector<stats_line>* lines) {
    if (!lines) return -EINVAL;
    std::vector<stats_line> pending;
    int result = networkstats_backend_visit(append_interface, &pending);
    if (result) return result;
    lines->insert(lines->end(), pending.begin(), pending.end());
    groupNetworkStats(lines);
    return 0;
}
uint64_t combineUidTag(uid_t uid, uint32_t tag) { return (uint64_t{uid} << 32) | tag; }
bool operator==(const stats_line& a, const stats_line& b) {
    return a.uid == b.uid && a.tag == b.tag && a.set == b.set &&
           !strncmp(a.iface, b.iface, sizeof(a.iface));
}
bool operator<(const stats_line& a, const stats_line& b) {
    int iface = strncmp(a.iface, b.iface, sizeof(a.iface));
    if (iface) return iface < 0;
    if (a.uid != b.uid) return a.uid < b.uid;
    if (a.tag != b.tag) return a.tag < b.tag;
    return a.set < b.set;
}
stats_line& stats_line::operator=(const stats_line& other) {
    if (this == &other) return *this;
    memcpy(iface, other.iface, sizeof(iface));
    uid = other.uid; set = other.set; tag = other.tag;
    rxBytes = other.rxBytes; rxPackets = other.rxPackets;
    txBytes = other.txBytes; txPackets = other.txPackets;
    return *this;
}
stats_line& stats_line::operator+=(const stats_line& other) {
    rxBytes += other.rxBytes; rxPackets += other.rxPackets;
    txBytes += other.txBytes; txPackets += other.txPackets;
    return *this;
}
void groupNetworkStats(std::vector<stats_line>* lines) {
    if (!lines || lines->size() < 2) return;
    std::sort(lines->begin(), lines->end());
    size_t out = 0;
    for (size_t i = 1; i < lines->size(); ++i) {
        if ((*lines)[out] == (*lines)[i]) (*lines)[out] += (*lines)[i];
        else { ++out; if (out != i) (*lines)[out] = (*lines)[i]; }
    }
    lines->resize(out + 1);
}
}
