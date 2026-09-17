#include <stdio.h>
#include "sws_client.h"
int main(void) {
    SwsDisplay display;
    int result = sws_get_display(&display);
    if (result < 0) {
        fprintf(stderr, "SWS display query failed: %d\n", result);
        return 1;
    }
    printf("SWS display: %ux%u, backend=%u epoch=%u capabilities=%llu\n",
           display.width, display.height, display.compositor_backend,
           display.compositor_epoch, (unsigned long long)display.capabilities);
    return 0;
}
