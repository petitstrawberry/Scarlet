#define LOG_TAG "scarlet-allocator"
#include <allocator-passthrough/2.0/GrallocLoader.h>
#include <hidl/HidlTransportSupport.h>

int main() {
    using android::hardware::graphics::allocator::V2_0::IAllocator;
    using android::hardware::graphics::allocator::V2_0::passthrough::GrallocLoader;
    android::hardware::configureRpcThreadpool(4, true);
    android::sp<IAllocator> allocator = GrallocLoader::load();
    if (!allocator || allocator->registerAsService("default") != android::OK) return 1;
    android::hardware::joinRpcThreadpool();
    return 1;
}
