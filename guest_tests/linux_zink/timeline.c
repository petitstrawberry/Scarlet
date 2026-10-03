/* A host signal must unblock an already accepted GPU submission. */
#define VK_NO_PROTOTYPES
#include "../linux_vulkan/loader.h"
#include <time.h>
#define FUNCTIONS(X) \
 X(vkCreateInstance) X(vkDestroyInstance) X(vkEnumeratePhysicalDevices) \
 X(vkGetPhysicalDeviceProperties) X(vkGetPhysicalDeviceMemoryProperties) \
 X(vkCreateDevice) X(vkDestroyDevice) X(vkGetDeviceQueue) \
 X(vkCreateBuffer) X(vkDestroyBuffer) X(vkGetBufferMemoryRequirements) \
 X(vkAllocateMemory) X(vkFreeMemory) X(vkBindBufferMemory) X(vkMapMemory) X(vkUnmapMemory) \
 X(vkCreateCommandPool) X(vkDestroyCommandPool) X(vkAllocateCommandBuffers) \
 X(vkBeginCommandBuffer) X(vkEndCommandBuffer) X(vkCmdFillBuffer) X(vkCmdUpdateBuffer) \
 X(vkCmdPipelineBarrier) X(vkQueueSubmit) X(vkQueueWaitIdle) \
 X(vkCreateSemaphore) X(vkDestroySemaphore) X(vkGetDeviceProcAddr) \
 X(vkCreateFence) X(vkDestroyFence) X(vkWaitForFences) \
 X(vkCreateImage) X(vkDestroyImage) X(vkGetImageMemoryRequirements) X(vkBindImageMemory) \
 X(vkCmdClearColorImage) X(vkCmdCopyImageToBuffer)
FUNCTIONS(DECLARE)
DECLARE(vkGetSemaphoreCounterValueKHR)
DECLARE(vkSignalSemaphoreKHR)
DECLARE(vkWaitSemaphoresKHR)
static double seconds(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec+t.tv_nsec/1e9; }
#define REQUIRE(x) do { if (!(x)) { fprintf(stderr,"failed: %s\n",#x); return 1; } } while (0)
#define DEVICE_LOAD(n) do { PFN_vkVoidFunction f=vkGetDeviceProcAddr(device,#n); memcpy(&n,&f,sizeof(n)); REQUIRE(n); } while (0)
int main(void) {
 setvbuf(stdout,NULL,_IONBF,0);
 if (load_vulkan()) return 1;
 FUNCTIONS(LOAD)
 VkInstance instance; VkInstanceCreateInfo ii={.sType=VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
 CHECK(vkCreateInstance(&ii,NULL,&instance));
 uint32_t count=1; VkPhysicalDevice physical;
 CHECK(vkEnumeratePhysicalDevices(instance,&count,&physical)); REQUIRE(count);
 VkPhysicalDeviceProperties props; vkGetPhysicalDeviceProperties(physical,&props); printf("GPU: %s\n",props.deviceName);
 float priority=1; VkDeviceQueueCreateInfo qi={.sType=VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,.queueFamilyIndex=0,.queueCount=1,.pQueuePriorities=&priority};
 const char *extensions[]={VK_KHR_TIMELINE_SEMAPHORE_EXTENSION_NAME};
 VkPhysicalDeviceTimelineSemaphoreFeatures tf={.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_TIMELINE_SEMAPHORE_FEATURES,.timelineSemaphore=VK_TRUE};
 VkDeviceCreateInfo di={.sType=VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,.pNext=&tf,.queueCreateInfoCount=1,.pQueueCreateInfos=&qi,.enabledExtensionCount=1,.ppEnabledExtensionNames=extensions};
 VkDevice device; CHECK(vkCreateDevice(physical,&di,NULL,&device));
 DEVICE_LOAD(vkGetSemaphoreCounterValueKHR); DEVICE_LOAD(vkSignalSemaphoreKHR); DEVICE_LOAD(vkWaitSemaphoresKHR);
 VkQueue queue; vkGetDeviceQueue(device,0,0,&queue);
 VkSemaphoreTypeCreateInfo ti={.sType=VK_STRUCTURE_TYPE_SEMAPHORE_TYPE_CREATE_INFO,.semaphoreType=VK_SEMAPHORE_TYPE_TIMELINE,.initialValue=0};
 VkSemaphoreCreateInfo si={.sType=VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO,.pNext=&ti}; VkSemaphore timeline;
 CHECK(vkCreateSemaphore(device,&si,NULL,&timeline));
 VkBufferCreateInfo bi={.sType=VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,.size=64,.usage=VK_BUFFER_USAGE_TRANSFER_DST_BIT,.sharingMode=VK_SHARING_MODE_EXCLUSIVE}; VkBuffer buffer;
 CHECK(vkCreateBuffer(device,&bi,NULL,&buffer)); VkMemoryRequirements req; vkGetBufferMemoryRequirements(device,buffer,&req);
 VkPhysicalDeviceMemoryProperties mp; vkGetPhysicalDeviceMemoryProperties(physical,&mp); uint32_t type=UINT32_MAX;
 for(uint32_t i=0;i<mp.memoryTypeCount;i++) if((req.memoryTypeBits&(1u<<i)) && (mp.memoryTypes[i].propertyFlags&(VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT|VK_MEMORY_PROPERTY_HOST_COHERENT_BIT))==(VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT|VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) {type=i;break;}
 REQUIRE(type!=UINT32_MAX); VkMemoryAllocateInfo mi={.sType=VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,.allocationSize=req.size,.memoryTypeIndex=type}; VkDeviceMemory memory;
 CHECK(vkAllocateMemory(device,&mi,NULL,&memory)); CHECK(vkBindBufferMemory(device,buffer,memory,0));
 VkImageCreateInfo image_info={.sType=VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,.imageType=VK_IMAGE_TYPE_2D,.format=VK_FORMAT_R8G8B8A8_UNORM,.extent={4,4,1},.mipLevels=1,.arrayLayers=1,.samples=VK_SAMPLE_COUNT_1_BIT,.tiling=VK_IMAGE_TILING_OPTIMAL,.usage=VK_IMAGE_USAGE_TRANSFER_SRC_BIT|VK_IMAGE_USAGE_TRANSFER_DST_BIT,.sharingMode=VK_SHARING_MODE_EXCLUSIVE,.initialLayout=VK_IMAGE_LAYOUT_UNDEFINED};
 VkImage image; CHECK(vkCreateImage(device,&image_info,NULL,&image)); vkGetImageMemoryRequirements(device,image,&req); mi.allocationSize=req.size;
 VkDeviceMemory image_memory; CHECK(vkAllocateMemory(device,&mi,NULL,&image_memory)); CHECK(vkBindImageMemory(device,image,image_memory,0));
 VkBuffer gpu_output; CHECK(vkCreateBuffer(device,&bi,NULL,&gpu_output)); vkGetBufferMemoryRequirements(device,gpu_output,&req); mi.allocationSize=req.size;
 VkDeviceMemory gpu_memory; CHECK(vkAllocateMemory(device,&mi,NULL,&gpu_memory)); CHECK(vkBindBufferMemory(device,gpu_output,gpu_memory,0));
 VkCommandPoolCreateInfo pi={.sType=VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,.queueFamilyIndex=0}; VkCommandPool pool;
 CHECK(vkCreateCommandPool(device,&pi,NULL,&pool));
 VkCommandBufferAllocateInfo ai={.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,.commandPool=pool,.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY,.commandBufferCount=1}; VkCommandBuffer cmd;
 CHECK(vkAllocateCommandBuffers(device,&ai,&cmd)); VkCommandBufferBeginInfo begin={.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO}; CHECK(vkBeginCommandBuffer(cmd,&begin));
 vkCmdFillBuffer(cmd,buffer,0,VK_WHOLE_SIZE,0x12345678);
 uint32_t patch[]={0xaabbccdd,0x11223344}; vkCmdUpdateBuffer(cmd,buffer,8,sizeof(patch),patch); patch[0]=patch[1]=0;
 VkBufferMemoryBarrier barrier={.sType=VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER,.srcAccessMask=VK_ACCESS_TRANSFER_WRITE_BIT,.dstAccessMask=VK_ACCESS_HOST_READ_BIT,.srcQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.buffer=buffer,.size=VK_WHOLE_SIZE};
 vkCmdPipelineBarrier(cmd,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_HOST_BIT,0,0,NULL,1,&barrier,0,NULL);
 /* Force a real GPU receipt as well as the buffer writes: a clear reaches the
  * Vulkan image and its pixels are copied into an independent readback buffer. */
 VkImageMemoryBarrier ib={.sType=VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,.dstAccessMask=VK_ACCESS_TRANSFER_WRITE_BIT,.oldLayout=VK_IMAGE_LAYOUT_UNDEFINED,.newLayout=VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,.srcQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.image=image,.subresourceRange={VK_IMAGE_ASPECT_COLOR_BIT,0,1,0,1}};
 vkCmdPipelineBarrier(cmd,VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT,0,0,NULL,0,NULL,1,&ib);
 VkClearColorValue red={.float32={1,0,0,1}}; vkCmdClearColorImage(cmd,image,VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,&red,1,&ib.subresourceRange);
 ib.srcAccessMask=VK_ACCESS_TRANSFER_WRITE_BIT; ib.dstAccessMask=VK_ACCESS_TRANSFER_READ_BIT; ib.oldLayout=VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL; ib.newLayout=VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
 vkCmdPipelineBarrier(cmd,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT,0,0,NULL,0,NULL,1,&ib);
 VkBufferImageCopy region={.imageSubresource={VK_IMAGE_ASPECT_COLOR_BIT,0,0,1},.imageExtent={4,4,1}};
 vkCmdCopyImageToBuffer(cmd,image,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,gpu_output,1,&region);
 barrier.buffer=gpu_output; vkCmdPipelineBarrier(cmd,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_HOST_BIT,0,0,NULL,1,&barrier,0,NULL);
 CHECK(vkEndCommandBuffer(cmd));
 uint64_t wait=1,signal=2; VkTimelineSemaphoreSubmitInfo tsi={.sType=VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO,.waitSemaphoreValueCount=1,.pWaitSemaphoreValues=&wait,.signalSemaphoreValueCount=1,.pSignalSemaphoreValues=&signal};
 VkPipelineStageFlags stage=VK_PIPELINE_STAGE_TRANSFER_BIT;
 VkSubmitInfo submit={.sType=VK_STRUCTURE_TYPE_SUBMIT_INFO,.pNext=&tsi,.waitSemaphoreCount=1,.pWaitSemaphores=&timeline,.pWaitDstStageMask=&stage,.commandBufferCount=1,.pCommandBuffers=&cmd,.signalSemaphoreCount=1,.pSignalSemaphores=&timeline};
 double start=seconds(); CHECK(vkQueueSubmit(queue,1,&submit,VK_NULL_HANDLE)); double elapsed=seconds()-start;
 printf("submit returned with unsatisfied wait: %.3fs\n",elapsed); REQUIRE(elapsed<2);
 uint64_t value; CHECK(vkGetSemaphoreCounterValueKHR(device,timeline,&value)); REQUIRE(value==0);
 VkSemaphoreWaitInfo wi={.sType=VK_STRUCTURE_TYPE_SEMAPHORE_WAIT_INFO,.semaphoreCount=1,.pSemaphores=&timeline,.pValues=&signal}; REQUIRE(vkWaitSemaphoresKHR(device,&wi,0)==VK_TIMEOUT);
 VkSemaphoreSignalInfo host={.sType=VK_STRUCTURE_TYPE_SEMAPHORE_SIGNAL_INFO,.semaphore=timeline,.value=1}; CHECK(vkSignalSemaphoreKHR(device,&host));
 CHECK(vkWaitSemaphoresKHR(device,&wi,10000000000ull)); CHECK(vkGetSemaphoreCounterValueKHR(device,timeline,&value)); REQUIRE(value==2);
 CHECK(vkQueueWaitIdle(queue)); uint32_t *mapped; CHECK(vkMapMemory(device,memory,0,64,0,(void**)&mapped));
 for(unsigned i=0;i<16;i++) { REQUIRE(mapped[i]==(i==2?0xaabbccdd:i==3?0x11223344:0x12345678)); }
 vkUnmapMemory(device,memory);
 CHECK(vkMapMemory(device,gpu_memory,0,64,0,(void**)&mapped));
 for(unsigned i=0;i<16;i++) { REQUIRE(mapped[i]==0xff0000ff); }
 vkUnmapMemory(device,gpu_memory);
 /* Empty submits must retire after earlier packets, and packet boundaries matter. */
 VkSemaphore binary; VkSemaphoreCreateInfo binary_info={.sType=VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO}; CHECK(vkCreateSemaphore(device,&binary_info,NULL,&binary));
 VkSubmitInfo packets[2]={{.sType=VK_STRUCTURE_TYPE_SUBMIT_INFO,.signalSemaphoreCount=1,.pSignalSemaphores=&binary},{.sType=VK_STRUCTURE_TYPE_SUBMIT_INFO,.waitSemaphoreCount=1,.pWaitSemaphores=&binary,.pWaitDstStageMask=&stage}};
 VkFenceCreateInfo fi={.sType=VK_STRUCTURE_TYPE_FENCE_CREATE_INFO}; VkFence fence; CHECK(vkCreateFence(device,&fi,NULL,&fence));
 CHECK(vkQueueSubmit(queue,2,packets,fence)); CHECK(vkWaitForFences(device,1,&fence,VK_TRUE,10000000000ull));
 CHECK(vkQueueWaitIdle(queue)); vkDestroyFence(device,fence,NULL); vkDestroySemaphore(device,binary,NULL);
 vkDestroyImage(device,image,NULL); vkFreeMemory(device,image_memory,NULL); vkDestroyBuffer(device,gpu_output,NULL); vkFreeMemory(device,gpu_memory,NULL);
 vkDestroyCommandPool(device,pool,NULL); vkDestroyBuffer(device,buffer,NULL); vkFreeMemory(device,memory,NULL); vkDestroySemaphore(device,timeline,NULL); vkDestroyDevice(device,NULL); vkDestroyInstance(instance,NULL);
 puts("PASS timeline host wait/signal, GPU clear/readback retirement, fill/update readback, binary packet ordering"); return 0;
}
