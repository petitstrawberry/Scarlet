/* Convert netd's bounded bandwidth iptables transaction into native Scarlet
 * named meters. This is a management adapter, not a netfilter implementation:
 * unsupported filter rules are ignored, while quota2 rules affect real IPv4
 * receive and transmit admission in the kernel. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define NATIVE_CALL_BASE 0x53430000u
#define TRAFFIC_CONFIGURE_V1 922u
#define VERSION_V1 1u
#define OP_METER_SET 1u
#define OP_METER_REMOVE 2u
#define OP_LINK_SET 3u
#define OP_LINK_CLEAR 4u
#define OP_GLOBAL_SET 5u
#define OP_GLOBAL_CLEAR 6u
#define OP_LINKS_CLEAR_METER 7u
#define MAX_OPERATIONS 96

struct traffic_request {
    uint32_t version;
    uint32_t operation;
    uint64_t remaining;
    char subject[32];
    char meter[32];
};
_Static_assert(sizeof(struct traffic_request) == 80, "native traffic ABI changed");

struct operation {
    uint32_t operation;
    uint64_t remaining;
    char subject[32];
    char meter[32];
};

static long scarlet_syscall1(long number, long argument) {
    register long x8 __asm__("x8") = number;
    register long x0 __asm__("x0") = argument;
    __asm__ volatile("svc #0" : "+r"(x0) : "r"(x8) : "memory");
    return x0;
}

static int copy_value(char output[32], const char *value) {
    size_t length = strlen(value);
    if (length == 0 || length >= 32) return -1;
    memset(output, 0, 32);
    memcpy(output, value, length);
    return 0;
}

static const char *option_value(char **tokens, int count, const char *option) {
    for (int i = 1; i + 1 < count; ++i) {
        if (strcmp(tokens[i], option) == 0) return tokens[i + 1];
    }
    return NULL;
}

static int append(struct operation *operations, size_t *count, uint32_t operation,
                  uint64_t remaining, const char *subject, const char *meter) {
    if (*count >= MAX_OPERATIONS) return -1;
    struct operation *next = &operations[(*count)++];
    memset(next, 0, sizeof(*next));
    next->operation = operation;
    next->remaining = remaining;
    if (subject && copy_value(next->subject, subject) != 0) return -1;
    if (meter && copy_value(next->meter, meter) != 0) return -1;
    return 0;
}

static const char *meter_for_chain(const char *chain) {
    static const char shared[] = "shared";
    static const char costly[] = "bw_costly_";
    if (strcmp(chain, "bw_costly_shared") == 0) return shared;
    if (strncmp(chain, costly, sizeof(costly) - 1) == 0 && chain[sizeof(costly) - 1])
        return chain + sizeof(costly) - 1;
    return NULL;
}

static int parse_line(char *line, struct operation *operations, size_t *count) {
    char *tokens[64];
    int token_count = 0;
    char *save = NULL;
    for (char *token = strtok_r(line, " \t\r\n", &save); token && token_count < 64;
         token = strtok_r(NULL, " \t\r\n", &save)) {
        tokens[token_count++] = token;
    }
    if (token_count == 0 || tokens[0][0] == '#' || tokens[0][0] == ':') return 0;
    if (strcmp(tokens[0], "COMMIT") == 0) return 0;
    int insert = strcmp(tokens[0], "-I") == 0 || strcmp(tokens[0], "-A") == 0;
    int remove = strcmp(tokens[0], "-D") == 0;
    int flush = strcmp(tokens[0], "-F") == 0 || strcmp(tokens[0], "-X") == 0;
    if (!insert && !remove && !flush) return 0;
    const char *chain = token_count > 1 ? tokens[1] : NULL;
    if (!chain) return 0;

    const char *quota = option_value(tokens, token_count, "--quota");
    const char *name = option_value(tokens, token_count, "--name");
    if (strcmp(chain, "bw_global_alert") == 0 && name) {
        if (strcmp(name, "globalAlert") != 0) return -1;
        if (remove) return append(operations, count, OP_GLOBAL_CLEAR, 0, NULL, name);
        char *end = NULL;
        uint64_t bytes = quota ? strtoull(quota, &end, 10) : 0;
        if (!quota || !end || *end || quota[0] == '-' || bytes == 0) return -1;
        if (append(operations, count, OP_METER_SET, bytes, name, NULL) != 0) return -1;
        return append(operations, count, OP_GLOBAL_SET, 0, NULL, name);
    }
    if (name && quota && insert) {
        char *end = NULL;
        uint64_t bytes = strtoull(quota, &end, 10);
        if (!end || *end || quota[0] == '-' || bytes == 0) return -1;
        return append(operations, count, OP_METER_SET, bytes, name, NULL);
    }
    if (name && remove) return append(operations, count, OP_METER_REMOVE, 0, name, NULL);

    const char *iface = option_value(tokens, token_count, "-i");
    if (!iface) iface = option_value(tokens, token_count, "-o");
    if (!iface && flush) {
        const char *meter = meter_for_chain(chain);
        return meter ? append(operations, count, OP_LINKS_CLEAR_METER, 0, NULL, meter) : 0;
    }
    if (!iface) return 0;
    const char *meter = meter_for_chain(chain);
    if (!meter) return 0;
    return append(operations, count, remove ? OP_LINK_CLEAR : OP_LINK_SET, 0, iface, meter);
}

static int execute(const struct operation *operation) {
    struct traffic_request request;
    memset(&request, 0, sizeof(request));
    request.version = VERSION_V1;
    request.operation = operation->operation;
    request.remaining = operation->remaining;
    memcpy(request.subject, operation->subject, sizeof(request.subject));
    memcpy(request.meter, operation->meter, sizeof(request.meter));
    return scarlet_syscall1(NATIVE_CALL_BASE | TRAFFIC_CONFIGURE_V1,
                            (long)(uintptr_t)&request) == 0 ? 0 : -1;
}

static int commit(const struct operation *operations, size_t count) {
    /* Meter creation precedes attachment. Removal runs after link detachments,
     * so a failing operation cannot leave a policy naming an absent counter. */
    for (size_t i = 0; i < count; ++i)
        if (operations[i].operation == OP_METER_SET && execute(&operations[i]) != 0) return -1;
    for (size_t i = 0; i < count; ++i) {
        uint32_t op = operations[i].operation;
        if ((op == OP_LINK_SET || op == OP_GLOBAL_SET) && execute(&operations[i]) != 0) return -1;
    }
    for (size_t i = 0; i < count; ++i) {
        uint32_t op = operations[i].operation;
        if ((op == OP_LINK_CLEAR || op == OP_LINKS_CLEAR_METER || op == OP_GLOBAL_CLEAR) &&
            execute(&operations[i]) != 0) return -1;
    }
    for (size_t i = 0; i < count; ++i)
        if (operations[i].operation == OP_METER_REMOVE && execute(&operations[i]) != 0) return -1;
    return 0;
}

int main(int argc, char **argv) {
    (void)argc;
    const char *program = strrchr(argv[0], '/');
    program = program ? program + 1 : argv[0];
    int ignore_v6 = strncmp(program, "ip6", 3) == 0;
    char buffer[4096];
    char line[4096];
    size_t used = 0;
    int list_tether_stats = 0;
    struct operation operations[MAX_OPERATIONS];
    size_t count = 0;
    for (;;) {
        ssize_t received = read(STDIN_FILENO, buffer, sizeof(buffer));
        if (received == 0) break;
        if (received < 0) {
            if (errno == EINTR) continue;
            return 1;
        }
        for (ssize_t i = 0; i < received; ++i) {
            char byte = buffer[i];
            if (byte != '\n') {
                if (used == sizeof(line)) return 1;
                line[used++] = byte;
                continue;
            }
            line[used] = 0;
            if (strstr(line, "-L") && strstr(line, "tetherctrl_counters")) list_tether_stats = 1;
            if (!ignore_v6 && parse_line(line, operations, &count) != 0) return 1;
            if (strcmp(line, "#PING") == 0 && write(STDOUT_FILENO, "#PING\n", 6) != 6) return 1;
            if (strcmp(line, "COMMIT") == 0) {
                if (!ignore_v6 && commit(operations, count) != 0) return 1;
                if (list_tether_stats) {
                    static const char empty_chain[] =
                        "Chain tetherctrl_counters (0 references)\n"
                        "pkts bytes target prot opt in out source destination\n";
                    if (write(STDOUT_FILENO, empty_chain, sizeof(empty_chain) - 1) != sizeof(empty_chain) - 1)
                        return 1;
                    list_tether_stats = 0;
                }
                count = 0;
            }
            used = 0;
        }
    }
    if (used != 0) return 1;
    return count == 0 ? 0 : 1;
}
