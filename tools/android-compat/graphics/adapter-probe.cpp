#define LOG_TAG "scarlet-adapter-probe"
#include <allocator-passthrough/2.0/GrallocLoader.h>
#include <android/hardware/graphics/mapper/2.0/IMapper.h>
#include <dlfcn.h>
#include <cstdio>
#include <cstring>

using android::hardware::graphics::mapper::V2_0::IMapper;
using android::hardware::graphics::mapper::V2_0::Error;
using android::hardware::graphics::common::V1_0::PixelFormat;
using android::hardware::graphics::common::V1_0::BufferUsage;

#define CHECK(c) do { if (!(c)) { std::fprintf(stderr,"ADAPTER_FAIL line=%d: %s\n",__LINE__,#c); return 2; } } while (0)

int main() {
    void* library=dlopen("android.hardware.graphics.mapper@2.0-impl.scarlet.so",RTLD_NOW|RTLD_LOCAL|RTLD_NODELETE);
    if (!library) std::fprintf(stderr,"mapper load: %s\n",dlerror());
    CHECK(library);
    auto create=reinterpret_cast<IMapper*(*)(const char*)>(dlsym(library,"HIDL_FETCH_IMapper"));
    CHECK(create);
    android::sp<IMapper> mapper=create("default");
    CHECK(mapper);
    android::sp<android::hardware::graphics::allocator::V2_0::IAllocator> allocator=
        android::hardware::graphics::allocator::V2_0::passthrough::GrallocLoader::load();
    CHECK(allocator);
    constexpr uint64_t usage=uint64_t(BufferUsage::CPU_READ_OFTEN)|uint64_t(BufferUsage::CPU_WRITE_OFTEN);
    IMapper::BufferDescriptorInfo info={64,32,1,PixelFormat::RGBA_8888,usage};
    android::hardware::graphics::mapper::V2_0::BufferDescriptor descriptor;
    bool descriptor_ok=false;
    CHECK(mapper->createDescriptor(info,[&](Error error,const auto& value) {
        descriptor_ok=error==Error::NONE;descriptor=value;
    }).isOk() && descriptor_ok);
    void* imported=nullptr;bool allocation_ok=false;
    CHECK(allocator->allocate(descriptor,1,[&](Error error,uint32_t stride,const auto& buffers) {
        if (error!=Error::NONE || stride<64 || buffers.size()!=1) return;
        allocation_ok=mapper->importBuffer(buffers[0],[&](Error result,void* buffer) {
            if (result==Error::NONE) imported=buffer;
        }).isOk() && imported;
    }).isOk() && allocation_ok);
    // The stock allocator has now released its copy. The mapper must own the
    // imported descriptor and mapping for the rest of this operation.
    bool lock_ok=false;void* pixels=nullptr;
    CHECK(mapper->lock(imported,usage,{0,0,64,32},{},[&](Error error,void* address) {
        lock_ok=error==Error::NONE;pixels=address;
    }).isOk() && lock_ok && pixels);
    std::memset(pixels,0x5a,64*4);
    bool unlock_ok=false;
    CHECK(mapper->unlock(imported,[&](Error error,const auto&) {unlock_ok=error==Error::NONE;}).isOk() && unlock_ok);
    lock_ok=false;pixels=nullptr;
    CHECK(mapper->lock(imported,uint64_t(BufferUsage::CPU_READ_OFTEN),{0,0,64,32},{},[&](Error error,void* address) {
        lock_ok=error==Error::NONE;pixels=address;
    }).isOk() && lock_ok && pixels);
    for (int i=0;i<64*4;i++) CHECK(static_cast<unsigned char*>(pixels)[i]==0x5a);
    unlock_ok=false;
    CHECK(mapper->unlock(imported,[&](Error error,const auto&) {unlock_ok=error==Error::NONE;}).isOk() && unlock_ok);
    CHECK(mapper->freeBuffer(imported).withDefault(Error::BAD_BUFFER)==Error::NONE);
    mapper.clear();allocator.clear();dlclose(library);
    std::puts("SCARLET_ANDROID_GRAPHICS_ADAPTERS_OK");
    return 0;
}
