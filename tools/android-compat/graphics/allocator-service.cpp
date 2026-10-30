#define LOG_TAG "scarlet-allocator"
#include <allocator-passthrough/2.0/GrallocLoader.h>
#include <hidl/HidlTransportSupport.h>
#include <cstdio>

int main() {
    using android::hardware::graphics::allocator::V2_0::IAllocator;
    using android::hardware::graphics::allocator::V2_0::passthrough::GrallocLoader;
    android::hardware::configureRpcThreadpool(4, true);
    android::sp<IAllocator> allocator = GrallocLoader::load();
    if (!allocator) {std::fprintf(stderr,"CPU allocator module load failed\n");return 1;}
    auto status=allocator->registerAsService("default");
    std::fprintf(stderr,"SCARLET_ANDROID_ALLOCATOR_REGISTER status=%d\n",status);
    if (status!=android::OK) return 1;
    android::hardware::joinRpcThreadpool();
    return 1;
}
