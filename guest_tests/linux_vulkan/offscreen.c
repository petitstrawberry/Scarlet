/* Same Vulkan GPU readback check on Linux and 64-bit Windows/Wine. */
#define VK_NO_PROTOTYPES
#include "loader.h"

#define FUNCTIONS(X) \
 X(vkCreateInstance) X(vkDestroyInstance) X(vkEnumeratePhysicalDevices) \
 X(vkGetPhysicalDeviceProperties) X(vkGetPhysicalDeviceMemoryProperties) \
 X(vkGetPhysicalDeviceQueueFamilyProperties) X(vkCreateDevice) X(vkDestroyDevice) \
 X(vkGetDeviceQueue) X(vkCreateImage) X(vkDestroyImage) X(vkGetImageMemoryRequirements) \
 X(vkCreateBuffer) X(vkDestroyBuffer) X(vkGetBufferMemoryRequirements) \
 X(vkAllocateMemory) X(vkFreeMemory) X(vkBindImageMemory) X(vkBindBufferMemory) \
 X(vkMapMemory) X(vkUnmapMemory) X(vkCreateCommandPool) X(vkDestroyCommandPool) \
 X(vkAllocateCommandBuffers) X(vkBeginCommandBuffer) X(vkEndCommandBuffer) \
 X(vkCmdPipelineBarrier) X(vkCmdClearColorImage) X(vkCmdCopyImageToBuffer) \
 X(vkQueueSubmit) X(vkQueueWaitIdle)
FUNCTIONS(DECLARE)

static uint32_t memory_type(VkPhysicalDevice physical, uint32_t bits) {
 VkPhysicalDeviceMemoryProperties properties;
 vkGetPhysicalDeviceMemoryProperties(physical, &properties);
 for (uint32_t i = 0; i < properties.memoryTypeCount; ++i)
  if ((bits & (1u << i)) && (properties.memoryTypes[i].propertyFlags &
      (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) ==
      (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) return i;
 return UINT32_MAX;
}

int main(void) {
 setvbuf(stdout, NULL, _IONBF, 0);
 if (load_vulkan()) return 1;
 FUNCTIONS(LOAD)
 VkApplicationInfo application = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
  .pApplicationName = "Scarlet Vulkan readback", .apiVersion = VK_API_VERSION_1_0};
 VkInstanceCreateInfo instance_info = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
  .pApplicationInfo = &application};
 VkInstance instance;
 CHECK(vkCreateInstance(&instance_info, NULL, &instance));
 uint32_t count = 0;
 CHECK(vkEnumeratePhysicalDevices(instance, &count, NULL));
 if (!count) { fprintf(stderr, "No Vulkan GPU\n"); return 1; }
 VkPhysicalDevice *physicals = calloc(count, sizeof(*physicals));
 CHECK(vkEnumeratePhysicalDevices(instance, &count, physicals));
 VkPhysicalDevice physical = physicals[0];
 free(physicals);
 VkPhysicalDeviceProperties properties;
 vkGetPhysicalDeviceProperties(physical, &properties);
 printf("Vulkan device: %s, API %u.%u\n", properties.deviceName,
  VK_VERSION_MAJOR(properties.apiVersion), VK_VERSION_MINOR(properties.apiVersion));
 vkGetPhysicalDeviceQueueFamilyProperties(physical, &count, NULL);
 VkQueueFamilyProperties *families = calloc(count, sizeof(*families));
 vkGetPhysicalDeviceQueueFamilyProperties(physical, &count, families);
 uint32_t family = UINT32_MAX;
 for (uint32_t i = 0; i < count; ++i)
  if (families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) { family = i; break; }
 free(families);
 if (family == UINT32_MAX) return 1;
 float priority = 1;
 VkDeviceQueueCreateInfo queue_info = {.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
  .queueFamilyIndex = family, .queueCount = 1, .pQueuePriorities = &priority};
 VkDeviceCreateInfo device_info = {.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
  .queueCreateInfoCount = 1, .pQueueCreateInfos = &queue_info};
 VkDevice device;
 CHECK(vkCreateDevice(physical, &device_info, NULL, &device));
 VkQueue queue;
 vkGetDeviceQueue(device, family, 0, &queue);
 VkImageCreateInfo image_info = {.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
  .imageType = VK_IMAGE_TYPE_2D, .format = VK_FORMAT_B8G8R8A8_UNORM,
  .extent = {64, 64, 1}, .mipLevels = 1, .arrayLayers = 1,
  .samples = VK_SAMPLE_COUNT_1_BIT, .tiling = VK_IMAGE_TILING_OPTIMAL,
  .usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT,
  .sharingMode = VK_SHARING_MODE_EXCLUSIVE, .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED};
 VkImage image;
 CHECK(vkCreateImage(device, &image_info, NULL, &image));
 VkMemoryRequirements required;
 vkGetImageMemoryRequirements(device, image, &required);
 VkMemoryAllocateInfo allocation = {.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
  .allocationSize = required.size, .memoryTypeIndex = memory_type(physical, required.memoryTypeBits)};
 VkDeviceMemory image_memory;
 CHECK(vkAllocateMemory(device, &allocation, NULL, &image_memory));
 CHECK(vkBindImageMemory(device, image, image_memory, 0));
 VkBufferCreateInfo buffer_info = {.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
  .size = 64 * 64 * 4, .usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT,
  .sharingMode = VK_SHARING_MODE_EXCLUSIVE};
 VkBuffer buffer;
 CHECK(vkCreateBuffer(device, &buffer_info, NULL, &buffer));
 vkGetBufferMemoryRequirements(device, buffer, &required);
 allocation.allocationSize = required.size;
 allocation.memoryTypeIndex = memory_type(physical, required.memoryTypeBits);
 VkDeviceMemory buffer_memory;
 CHECK(vkAllocateMemory(device, &allocation, NULL, &buffer_memory));
 CHECK(vkBindBufferMemory(device, buffer, buffer_memory, 0));
 VkCommandPoolCreateInfo pool_info = {.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
  .queueFamilyIndex = family};
 VkCommandPool pool;
 CHECK(vkCreateCommandPool(device, &pool_info, NULL, &pool));
 VkCommandBufferAllocateInfo commands = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
  .commandPool = pool, .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY, .commandBufferCount = 1};
 VkCommandBuffer command;
 CHECK(vkAllocateCommandBuffers(device, &commands, &command));
 VkCommandBufferBeginInfo begin = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
 CHECK(vkBeginCommandBuffer(command, &begin));
 VkImageMemoryBarrier barrier = {.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
  .srcAccessMask = 0, .dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
  .oldLayout = VK_IMAGE_LAYOUT_UNDEFINED, .newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
  .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED, .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
  .image = image, .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
 vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
  0, 0, NULL, 0, NULL, 1, &barrier);
 VkClearColorValue color = {.float32 = {1.0f, 0.0f, 0.0f, 1.0f}};
 vkCmdClearColorImage(command, image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
  &color, 1, &barrier.subresourceRange);
 barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
 barrier.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
 barrier.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
 barrier.newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
 vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
  0, 0, NULL, 0, NULL, 1, &barrier);
 VkBufferImageCopy copy = {.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1},
  .imageExtent = {64, 64, 1}};
 vkCmdCopyImageToBuffer(command, image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, buffer, 1, &copy);
 CHECK(vkEndCommandBuffer(command));
 VkSubmitInfo submit = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
  .commandBufferCount = 1, .pCommandBuffers = &command};
 CHECK(vkQueueSubmit(queue, 1, &submit, VK_NULL_HANDLE));
 CHECK(vkQueueWaitIdle(queue));
 unsigned char *pixels;
 CHECK(vkMapMemory(device, buffer_memory, 0, buffer_info.size, 0, (void **)&pixels));
 for (unsigned i = 0; i < 4096; ++i) {
  if (pixels[4*i] || pixels[4*i+1] || pixels[4*i+2] != 255 || pixels[4*i+3] != 255) {
   fprintf(stderr, "Pixel %u: BGRA %u %u %u %u\n", i,
    pixels[4*i], pixels[4*i+1], pixels[4*i+2], pixels[4*i+3]); return 1;
  }
 }
 vkUnmapMemory(device, buffer_memory);
 vkDestroyCommandPool(device, pool, NULL);
 vkDestroyBuffer(device, buffer, NULL);
 vkFreeMemory(device, buffer_memory, NULL);
 vkDestroyImage(device, image, NULL);
 vkFreeMemory(device, image_memory, NULL);
 vkDestroyDevice(device, NULL);
 vkDestroyInstance(instance, NULL);
 puts("PASS: Vulkan GPU clear and readback, all 4096 BGRA pixels");
 return 0;
}
