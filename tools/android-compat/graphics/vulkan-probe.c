/* Execute a real transfer command through the Android Vulkan hardware module. */
#include <hardware/hwvulkan.h>
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define CHECK(c) do { if (!(c)) {fprintf(stderr,"VULKAN_FAIL line=%d: %s\n",__LINE__,#c);return 2;} } while (0)
#define LOAD_I(name) PFN_vk##name name=(PFN_vk##name)gipa(instance,"vk" #name);CHECK(name)
#define LOAD_D(name) PFN_vk##name name=(PFN_vk##name)gdpa(device,"vk" #name);CHECK(name)

int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);
    void* library=dlopen("/vendor/lib64/hw/vulkan.pastel.so",RTLD_NOW|RTLD_LOCAL);
    if (!library) fprintf(stderr,"software Vulkan load: %s\n",dlerror());
    CHECK(library);
    hw_module_t* module=dlsym(library,HAL_MODULE_INFO_SYM_AS_STR);
    CHECK(module && module->id && !strcmp(module->id,HWVULKAN_HARDWARE_MODULE_ID));
    hw_device_t* opened=NULL;CHECK(module->methods->open(module,HWVULKAN_DEVICE_0,&opened)==0 && opened);
    hwvulkan_device_t* hal=(hwvulkan_device_t*)opened;
    PFN_vkGetInstanceProcAddr gipa=hal->GetInstanceProcAddr;CHECK(gipa);
    VkApplicationInfo application={.sType=VK_STRUCTURE_TYPE_APPLICATION_INFO,.pApplicationName="Scarlet software Vulkan probe",.apiVersion=VK_API_VERSION_1_1};
    VkInstanceCreateInfo create={.sType=VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,.pApplicationInfo=&application};
    VkInstance instance=VK_NULL_HANDLE;
    CHECK(hal->CreateInstance(&create,NULL,&instance)==VK_SUCCESS);
    puts("SCARLET_ANDROID_VULKAN_INSTANCE");
    LOAD_I(EnumeratePhysicalDevices);LOAD_I(GetPhysicalDeviceProperties);LOAD_I(GetPhysicalDeviceQueueFamilyProperties);
    LOAD_I(GetPhysicalDeviceMemoryProperties);LOAD_I(CreateDevice);LOAD_I(GetDeviceProcAddr);LOAD_I(DestroyInstance);
    uint32_t count=0;CHECK(EnumeratePhysicalDevices(instance,&count,NULL)==VK_SUCCESS && count==1);
    VkPhysicalDevice physical;CHECK(EnumeratePhysicalDevices(instance,&count,&physical)==VK_SUCCESS);
    VkPhysicalDeviceProperties properties;GetPhysicalDeviceProperties(physical,&properties);
    printf("SCARLET_ANDROID_VULKAN_DEVICE name=%s type=%u\n",properties.deviceName,properties.deviceType);
    CHECK(strstr(properties.deviceName,"SwiftShader") && properties.deviceType==VK_PHYSICAL_DEVICE_TYPE_CPU);
    VkQueueFamilyProperties families[16];count=16;GetPhysicalDeviceQueueFamilyProperties(physical,&count,families);
    uint32_t family=0;while(family<count && !(families[family].queueFlags&VK_QUEUE_TRANSFER_BIT))family++;
    CHECK(family<count);
    float priority=1;
    VkDeviceQueueCreateInfo queue_info={.sType=VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,.queueFamilyIndex=family,.queueCount=1,.pQueuePriorities=&priority};
    VkDeviceCreateInfo device_info={.sType=VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,.queueCreateInfoCount=1,.pQueueCreateInfos=&queue_info};
    VkDevice device;CHECK(CreateDevice(physical,&device_info,NULL,&device)==VK_SUCCESS);
    PFN_vkGetDeviceProcAddr gdpa=GetDeviceProcAddr;
    LOAD_D(GetDeviceQueue);LOAD_D(CreateBuffer);LOAD_D(GetBufferMemoryRequirements);LOAD_D(AllocateMemory);
    LOAD_D(BindBufferMemory);LOAD_D(MapMemory);LOAD_D(UnmapMemory);LOAD_D(CreateCommandPool);
    LOAD_D(AllocateCommandBuffers);LOAD_D(BeginCommandBuffer);LOAD_D(CmdFillBuffer);LOAD_D(EndCommandBuffer);
    LOAD_D(CreateFence);LOAD_D(QueueSubmit);LOAD_D(WaitForFences);LOAD_D(DestroyFence);
    LOAD_D(DestroyCommandPool);LOAD_D(DestroyBuffer);LOAD_D(FreeMemory);LOAD_D(DestroyDevice);
    VkQueue queue;GetDeviceQueue(device,family,0,&queue);
    VkBufferCreateInfo buffer_info={.sType=VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,.size=4096,.usage=VK_BUFFER_USAGE_TRANSFER_DST_BIT,.sharingMode=VK_SHARING_MODE_EXCLUSIVE};
    VkBuffer buffer;CHECK(CreateBuffer(device,&buffer_info,NULL,&buffer)==VK_SUCCESS);
    VkMemoryRequirements requirement;GetBufferMemoryRequirements(device,buffer,&requirement);
    CHECK(requirement.size>=4096 && requirement.size<=16*1024*1024);
    VkPhysicalDeviceMemoryProperties memory_properties;GetPhysicalDeviceMemoryProperties(physical,&memory_properties);
    uint32_t type=0;VkMemoryPropertyFlags flags=VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT|VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;
    while(type<memory_properties.memoryTypeCount && (!(requirement.memoryTypeBits&(1u<<type)) || (memory_properties.memoryTypes[type].propertyFlags&flags)!=flags))type++;
    CHECK(type<memory_properties.memoryTypeCount);
    VkMemoryAllocateInfo memory_info={.sType=VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,.allocationSize=requirement.size,.memoryTypeIndex=type};
    VkDeviceMemory memory;CHECK(AllocateMemory(device,&memory_info,NULL,&memory)==VK_SUCCESS);
    CHECK(BindBufferMemory(device,buffer,memory,0)==VK_SUCCESS);
    void* mapped;CHECK(MapMemory(device,memory,0,4096,0,&mapped)==VK_SUCCESS);memset(mapped,0,4096);
    VkCommandPoolCreateInfo pool_info={.sType=VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,.queueFamilyIndex=family};
    VkCommandPool pool;CHECK(CreateCommandPool(device,&pool_info,NULL,&pool)==VK_SUCCESS);
    VkCommandBufferAllocateInfo command_info={.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,.commandPool=pool,.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY,.commandBufferCount=1};
    VkCommandBuffer command;CHECK(AllocateCommandBuffers(device,&command_info,&command)==VK_SUCCESS);
    VkCommandBufferBeginInfo begin={.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,.flags=VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT};
    CHECK(BeginCommandBuffer(command,&begin)==VK_SUCCESS);CmdFillBuffer(command,buffer,0,4096,0x5a12bc34);
    CHECK(EndCommandBuffer(command)==VK_SUCCESS);
    VkFenceCreateInfo fence_info={.sType=VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};VkFence fence;
    CHECK(CreateFence(device,&fence_info,NULL,&fence)==VK_SUCCESS);
    VkSubmitInfo submit={.sType=VK_STRUCTURE_TYPE_SUBMIT_INFO,.commandBufferCount=1,.pCommandBuffers=&command};
    CHECK(QueueSubmit(queue,1,&submit,fence)==VK_SUCCESS);
    CHECK(WaitForFences(device,1,&fence,VK_TRUE,10000000000ULL)==VK_SUCCESS);
    for (uint32_t i=0;i<4096/4;i++)CHECK(((uint32_t*)mapped)[i]==0x5a12bc34);
    UnmapMemory(device,memory);DestroyFence(device,fence,NULL);DestroyCommandPool(device,pool,NULL);
    DestroyBuffer(device,buffer,NULL);FreeMemory(device,memory,NULL);DestroyDevice(device,NULL);
    DestroyInstance(instance,NULL);CHECK(opened->close(opened)==0);dlclose(library);
    puts("SCARLET_ANDROID_VULKAN_TRANSFER_OK");
    return 0;
}
