/* Management-side compatibility endpoint for netd's rule loader.
 *
 * Scarlet does not yet expose Linux netfilter. Accepting the stock restore
 * protocol lets netd initialize and publish its Binder service without
 * pretending that packet filtering or traffic accounting is available. */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    char buffer[4096];
    char line[16];
    size_t used = 0;
    for (;;) {
        ssize_t count = read(STDIN_FILENO, buffer, sizeof(buffer));
        if (count == 0) return 0;
        if (count < 0) {
            if (errno == EINTR) continue;
            return 1;
        }
        for (ssize_t i = 0; i < count; ++i) {
            if (used < sizeof(line)) line[used++] = buffer[i];
            else used = sizeof(line) + 1;
            if (buffer[i] == '\n') {
                if (used == 6 && memcmp(line, "#PING\n", 6) == 0) {
                    if (write(STDOUT_FILENO, line, 6) != 6) return 1;
                }
                used = 0;
            }
        }
    }
}
