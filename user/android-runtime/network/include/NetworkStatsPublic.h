/*
 * Copyright (C) 2018 The Android Open Source Project
 * Licensed under the Apache License, Version 2.0.
 * Derived from the pinned libnetworkstats contract in SOURCE_PINS.json.
 */
#pragma once
#include <stdint.h>
#include <sys/types.h>
#include <string>
#include <vector>

struct Stats {
    uint64_t rxBytes, rxPackets, txBytes, txPackets, tcpRxPackets, tcpTxPackets;
};

#pragma GCC visibility push(default)
namespace android::bpf {
struct stats_line {
    char iface[32];
    uint32_t uid, set, tag;
    int64_t rxBytes, rxPackets, txBytes, txPackets;
    stats_line() = default;
    stats_line(const stats_line&) = default;
    stats_line& operator=(const stats_line&);
    stats_line& operator+=(const stats_line&);
};
bool operator==(const stats_line&, const stats_line&);
bool operator<(const stats_line&, const stats_line&);
int bpfGetUidStats(uid_t, Stats*);
int bpfGetIfaceStats(const char*, Stats*);
int parseBpfNetworkStatsDev(std::vector<stats_line>*);
int parseBpfNetworkStatsDetail(std::vector<stats_line>*, const std::vector<std::string>&, int, int);
void groupNetworkStats(std::vector<stats_line>*);
uint64_t combineUidTag(uid_t, uint32_t);
}
#pragma GCC visibility pop
