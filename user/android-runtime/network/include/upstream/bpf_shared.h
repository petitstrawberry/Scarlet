/*
 * Copyright (C) 2018 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#pragma once

#include <linux/if.h>
#include <linux/if_ether.h>
#include <linux/in.h>
#include <linux/in6.h>

#ifdef __cplusplus
#include <string_view>
#include "XtBpfProgLocations.h"
#endif

// This header file is shared by eBPF kernel programs (C) and netd (C++) and
// some of the maps are also accessed directly from Java mainline module code.
//
// Hence: explicitly pad all relevant structures and assert that their size
// is the sum of the sizes of their fields.
#define STRUCT_SIZE(name, size) _Static_assert(sizeof(name) == (size), "Incorrect struct size.")

typedef struct {
    uint32_t uid;
    uint32_t tag;
} UidTagValue;
STRUCT_SIZE(UidTagValue, 2 * 4);  // 8

typedef struct {
    uint32_t uid;
    uint32_t tag;
    uint32_t counterSet;
    uint32_t ifaceIndex;
} StatsKey;
STRUCT_SIZE(StatsKey, 4 * 4);  // 16

typedef struct {
    uint64_t rxPackets;
    uint64_t rxBytes;
    uint64_t txPackets;
    uint64_t txBytes;
} StatsValue;
STRUCT_SIZE(StatsValue, 4 * 8);  // 32

typedef struct {
    char name[IFNAMSIZ];
} IfaceValue;
STRUCT_SIZE(IfaceValue, 16);

typedef struct {
    uint64_t rxBytes;
    uint64_t rxPackets;
    uint64_t txBytes;
    uint64_t txPackets;
    uint64_t tcpRxPackets;
    uint64_t tcpTxPackets;
} Stats;

// Since we cannot garbage collect the stats map since device boot, we need to make these maps as
// large as possible. The maximum size of number of map entries we can have is depend on the rlimit
// of MEM_LOCK granted to netd. The memory space needed by each map can be calculated by the
// following fomula:
//      elem_size = 40 + roundup(key_size, 8) + roundup(value_size, 8)
//      cost = roundup_pow_of_two(max_entries) * 16 + elem_size * max_entries +
//              elem_size * number_of_CPU
// And the cost of each map currently used is(assume the device have 8 CPUs):
// cookie_tag_map:      key:  8 bytes, value:  8 bytes, cost:  822592 bytes    =   823Kbytes
// uid_counter_set_map: key:  4 bytes, value:  1 bytes, cost:  145216 bytes    =   145Kbytes
// app_uid_stats_map:   key:  4 bytes, value: 32 bytes, cost: 1062784 bytes    =  1063Kbytes
// uid_stats_map:       key: 16 bytes, value: 32 bytes, cost: 1142848 bytes    =  1143Kbytes
// tag_stats_map:       key: 16 bytes, value: 32 bytes, cost: 1142848 bytes    =  1143Kbytes
// iface_index_name_map:key:  4 bytes, value: 16 bytes, cost:   80896 bytes    =    81Kbytes
// iface_stats_map:     key:  4 bytes, value: 32 bytes, cost:   97024 bytes    =    97Kbytes
// dozable_uid_map:     key:  4 bytes, value:  1 bytes, cost:  145216 bytes    =   145Kbytes
// standby_uid_map:     key:  4 bytes, value:  1 bytes, cost:  145216 bytes    =   145Kbytes
// powersave_uid_map:   key:  4 bytes, value:  1 bytes, cost:  145216 bytes    =   145Kbytes
// total:                                                                         4930Kbytes
// It takes maximum 4.9MB kernel memory space if all maps are full, which requires any devices
// running this module to have a memlock rlimit to be larger then 5MB. In the old qtaguid module,
// we don't have a total limit for data entries but only have limitation of tags each uid can have.
// (default is 1024 in kernel);

// 'static' - otherwise these constants end up in .rodata in the resulting .o post compilation
static const int COOKIE_UID_MAP_SIZE = 10000;
static const int UID_COUNTERSET_MAP_SIZE = 2000;
static const int APP_STATS_MAP_SIZE = 10000;
static const int STATS_MAP_SIZE = 5000;
static const int IFACE_INDEX_NAME_MAP_SIZE = 1000;
static const int IFACE_STATS_MAP_SIZE = 1000;
static const int CONFIGURATION_MAP_SIZE = 2;
static const int UID_OWNER_MAP_SIZE = 2000;

#ifdef __cplusplus

#define BPF_NETD_PATH "/sys/fs/bpf/netd_shared/"

#define BPF_EGRESS_PROG_PATH BPF_NETD_PATH "prog_netd_cgroupskb_egress_stats"
#define BPF_INGRESS_PROG_PATH BPF_NETD_PATH "prog_netd_cgroupskb_ingress_stats"

#define ASSERT_STRING_EQUAL(s1, s2) \
    static_assert(std::string_view(s1) == std::string_view(s2), "mismatch vs Android T netd")

/* -=-=-=-=- WARNING -=-=-=-=-
 *
 * These 4 xt_bpf program paths are actually defined by:
 *   //system/netd/include/mainline/XtBpfProgLocations.h
 * which is intentionally a non-automerged location.
 *
 * They are *UNCHANGEABLE* due to being hard coded in Android T's netd binary
 * as such we have compile time asserts that things match.
 * (which will be validated during build on mainline-prod branch against old system/netd)
 *
 * If you break this, netd on T will fail to start with your tethering mainline module.
 */
ASSERT_STRING_EQUAL(XT_BPF_INGRESS_PROG_PATH,   BPF_NETD_PATH "prog_netd_skfilter_ingress_xtbpf");
ASSERT_STRING_EQUAL(XT_BPF_EGRESS_PROG_PATH,    BPF_NETD_PATH "prog_netd_skfilter_egress_xtbpf");
ASSERT_STRING_EQUAL(XT_BPF_ALLOWLIST_PROG_PATH, BPF_NETD_PATH "prog_netd_skfilter_allowlist_xtbpf");
ASSERT_STRING_EQUAL(XT_BPF_DENYLIST_PROG_PATH,  BPF_NETD_PATH "prog_netd_skfilter_denylist_xtbpf");

#define CGROUP_SOCKET_PROG_PATH BPF_NETD_PATH "prog_netd_cgroupsock_inet_create"

#define TC_BPF_INGRESS_ACCOUNT_PROG_NAME "prog_netd_schedact_ingress_account"
#define TC_BPF_INGRESS_ACCOUNT_PROG_PATH BPF_NETD_PATH TC_BPF_INGRESS_ACCOUNT_PROG_NAME

#define COOKIE_TAG_MAP_PATH BPF_NETD_PATH "map_netd_cookie_tag_map"
#define UID_COUNTERSET_MAP_PATH BPF_NETD_PATH "map_netd_uid_counterset_map"
#define APP_UID_STATS_MAP_PATH BPF_NETD_PATH "map_netd_app_uid_stats_map"
#define STATS_MAP_A_PATH BPF_NETD_PATH "map_netd_stats_map_A"
#define STATS_MAP_B_PATH BPF_NETD_PATH "map_netd_stats_map_B"
#define IFACE_INDEX_NAME_MAP_PATH BPF_NETD_PATH "map_netd_iface_index_name_map"
#define IFACE_STATS_MAP_PATH BPF_NETD_PATH "map_netd_iface_stats_map"
#define CONFIGURATION_MAP_PATH BPF_NETD_PATH "map_netd_configuration_map"
#define UID_OWNER_MAP_PATH BPF_NETD_PATH "map_netd_uid_owner_map"
#define UID_PERMISSION_MAP_PATH BPF_NETD_PATH "map_netd_uid_permission_map"

#endif // __cplusplus

enum UidOwnerMatchType {
    NO_MATCH = 0,
    HAPPY_BOX_MATCH = (1 << 0),
    PENALTY_BOX_MATCH = (1 << 1),
    DOZABLE_MATCH = (1 << 2),
    STANDBY_MATCH = (1 << 3),
    POWERSAVE_MATCH = (1 << 4),
    RESTRICTED_MATCH = (1 << 5),
    LOW_POWER_STANDBY_MATCH = (1 << 6),
    IIF_MATCH = (1 << 7),
    LOCKDOWN_VPN_MATCH = (1 << 8),
    OEM_DENY_1_MATCH = (1 << 9),
    OEM_DENY_2_MATCH = (1 << 10),
    OEM_DENY_3_MATCH = (1 << 11),
};

enum BpfPermissionMatch {
    BPF_PERMISSION_INTERNET = 1 << 2,
    BPF_PERMISSION_UPDATE_DEVICE_STATS = 1 << 3,
};
// In production we use two identical stats maps to record per uid stats and
// do swap and clean based on the configuration specified here. The statsMapType
// value in configuration map specified which map is currently in use.
enum StatsMapType {
    SELECT_MAP_A,
    SELECT_MAP_B,
};

// TODO: change the configuration object from a bitmask to an object with clearer
// semantics, like a struct.
typedef uint32_t BpfConfig;
static const BpfConfig DEFAULT_CONFIG = 0;

typedef struct {
    // Allowed interface index. Only applicable if IIF_MATCH is set in the rule bitmask above.
    uint32_t iif;
    // A bitmask of enum values in UidOwnerMatchType.
    uint32_t rule;
} UidOwnerValue;
STRUCT_SIZE(UidOwnerValue, 2 * 4);  // 8

// Entry in the configuration map that stores which UID rules are enabled.
#define UID_RULES_CONFIGURATION_KEY 0
// Entry in the configuration map that stores which stats map is currently in use.
#define CURRENT_STATS_MAP_CONFIGURATION_KEY 1

typedef struct {
    uint32_t iif;            // The input interface index
    struct in6_addr pfx96;   // The source /96 nat64 prefix, bottom 32 bits must be 0
    struct in6_addr local6;  // The full 128-bits of the destination IPv6 address
} ClatIngress6Key;
STRUCT_SIZE(ClatIngress6Key, 4 + 2 * 16);  // 36

typedef struct {
    uint32_t oif;           // The output interface to redirect to (0 means don't redirect)
    struct in_addr local4;  // The destination IPv4 address
} ClatIngress6Value;
STRUCT_SIZE(ClatIngress6Value, 4 + 4);  // 8

typedef struct {
    uint32_t iif;           // The input interface index
    struct in_addr local4;  // The source IPv4 address
} ClatEgress4Key;
STRUCT_SIZE(ClatEgress4Key, 4 + 4);  // 8

typedef struct {
    uint32_t oif;            // The output interface to redirect to
    struct in6_addr local6;  // The full 128-bits of the source IPv6 address
    struct in6_addr pfx96;   // The destination /96 nat64 prefix, bottom 32 bits must be 0
    bool oifIsEthernet;      // Whether the output interface requires ethernet header
    uint8_t pad[3];
} ClatEgress4Value;
STRUCT_SIZE(ClatEgress4Value, 4 + 2 * 16 + 1 + 3);  // 40

#undef STRUCT_SIZE
