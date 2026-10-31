/* Start the stock audio HAL after AudioFlinger has had time to publish. */
#include <sys/system_properties.h>
#include <unistd.h>

int main(void) {
    usleep(1500000);
    return __system_property_set("ctl.start", "vendor.audio-hal") == 0 ? 0 : 1;
}
