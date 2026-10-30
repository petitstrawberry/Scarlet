#define LOG_TAG "scarlet-mapper"
#include <mapper-passthrough/2.0/GrallocLoader.h>

extern "C" android::hardware::graphics::mapper::V2_0::IMapper*
HIDL_FETCH_IMapper(const char*) {
    return android::hardware::graphics::mapper::V2_0::passthrough::GrallocLoader::load();
}
