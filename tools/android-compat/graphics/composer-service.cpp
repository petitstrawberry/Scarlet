#define LOG_TAG "scarlet-composer"
#include <composer-hal/2.1/Composer.h>
#include <composer-passthrough/2.1/HwcHal.h>
#include <hwc2onfbadapter/HWC2OnFbAdapter.h>
#include <hardware/fb.h>
#include <hardware/gralloc.h>
#include <hidl/HidlTransportSupport.h>
#include <cstdio>

int main() {
    using namespace android::hardware::graphics::composer::V2_1;
    android::hardware::configureRpcThreadpool(4,true);
    const hw_module_t* module=nullptr;
    int result=hw_get_module(GRALLOC_HARDWARE_MODULE_ID,&module);
    if (result) {std::fprintf(stderr,"CPU composer gralloc load failed: %d\n",result);return 1;}
    framebuffer_device_t* framebuffer=nullptr;
    result=framebuffer_open(module,&framebuffer);
    if (result) {std::fprintf(stderr,"CPU composer framebuffer open failed: %d\n",result);return 1;}
    auto hwc=std::make_unique<passthrough::HwcHal>();
    // The stock framebuffer adapter explicitly reports unreliable present
    // fences. Keep that capability; no simulated fence or RT policy is added.
    if (!hwc->initWithDevice(new android::HWC2OnFbAdapter(framebuffer),false)) return 1;
    android::sp<IComposer> composer=hal::Composer::create(std::move(hwc)).release();
    auto status=composer->registerAsService("default");
    std::fprintf(stderr,"SCARLET_ANDROID_COMPOSER_REGISTER status=%d\n",status);
    if (status!=android::OK) return 1;
    android::hardware::joinRpcThreadpool();
    return 1;
}
