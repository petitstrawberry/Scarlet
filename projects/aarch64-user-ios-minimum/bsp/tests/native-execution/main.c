#include "probe.h"

#include <stdio.h>

static void print_line(const char *line) {
    fprintf(stderr, "%s\n", line);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s path/to/ScarletFixture.dylib\n", argv[0]);
        return 2;
    }
    bool passed = probe_run(argv[1], print_line);
    print_line(passed ? "SCARLET_P0_PASS" : "SCARLET_P0_FAIL");
    return passed ? 0 : 1;
}
