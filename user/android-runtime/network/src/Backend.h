#pragma once

#include <stdint.h>

/* Native operations used by the provider. These symbols remain private. */
int netd_updatable_backend_query_traffic(uint64_t* supported_operations);
int netd_updatable_backend_open_cgroup(const char* path, int* fd);

