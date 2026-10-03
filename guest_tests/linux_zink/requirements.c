/* Mesa 25.0.7 GL 2.1 capability inventory; not a rendering/conformance test. */
#define VK_NO_PROTOTYPES
#include "../linux_vulkan/loader.h"

#define FUNCTIONS(X) \
 X(vkCreateInstance) X(vkDestroyInstance) X(vkEnumerateInstanceExtensionProperties) \
 X(vkEnumeratePhysicalDevices) X(vkGetPhysicalDeviceProperties) \
 X(vkGetPhysicalDeviceFeatures) X(vkEnumerateDeviceExtensionProperties) \
 X(vkGetInstanceProcAddr)
FUNCTIONS(DECLARE)

static int extension(const VkExtensionProperties *list, uint32_t count, const char *name) {
 for (uint32_t i = 0; i < count; ++i)
  if (!strcmp(list[i].extensionName, name)) return 1;
 return 0;
}
static int promoted(uint32_t api, uint32_t minimum) { return minimum && api >= minimum; }
static unsigned missing;
static void report(const char *name, int supported) {
 printf("%s %s\n", supported ? "OK" : "MISSING", name);
 missing += !supported;
}

int main(void) {
 setvbuf(stdout, NULL, _IONBF, 0);
 if (load_vulkan()) return 1;
 FUNCTIONS(LOAD)
 uint32_t count = 0;
 CHECK(vkEnumerateInstanceExtensionProperties(NULL, &count, NULL));
 VkExtensionProperties *extensions = calloc(count ? count : 1, sizeof(*extensions));
 if (!extensions) return 1;
 CHECK(vkEnumerateInstanceExtensionProperties(NULL, &count, extensions));
 const char *query = VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES_2_EXTENSION_NAME;
 int queries2 = extension(extensions, count, query);
 free(extensions);
 VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
  .pApplicationName = "Scarlet Zink requirements", .apiVersion = VK_API_VERSION_1_0};
 VkInstanceCreateInfo info = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
  .pApplicationInfo = &app, .enabledExtensionCount = queries2 ? 1u : 0u,
  .ppEnabledExtensionNames = queries2 ? &query : NULL};
 VkInstance instance;
 CHECK(vkCreateInstance(&info, NULL, &instance));
 CHECK(vkEnumeratePhysicalDevices(instance, &count, NULL));
 if (!count) { fprintf(stderr, "No Vulkan physical device\n"); return 1; }
 VkPhysicalDevice *devices = calloc(count, sizeof(*devices));
 if (!devices) return 1;
 CHECK(vkEnumeratePhysicalDevices(instance, &count, devices));
 PFN_vkGetPhysicalDeviceFeatures2KHR features2 = (PFN_vkGetPhysicalDeviceFeatures2KHR)
  vkGetInstanceProcAddr(instance, "vkGetPhysicalDeviceFeatures2KHR");
 unsigned total = 0;
 for (uint32_t i = 0; i < count; ++i) {
  missing = 0;
  VkPhysicalDeviceProperties properties;
  vkGetPhysicalDeviceProperties(devices[i], &properties);
  printf("DEVICE %s Vulkan %u.%u.%u\n", properties.deviceName,
   VK_VERSION_MAJOR(properties.apiVersion), VK_VERSION_MINOR(properties.apiVersion),
   VK_VERSION_PATCH(properties.apiVersion));
  uint32_t n = 0;
  CHECK(vkEnumerateDeviceExtensionProperties(devices[i], NULL, &n, NULL));
  extensions = calloc(n ? n : 1, sizeof(*extensions));
  if (!extensions) return 1;
  CHECK(vkEnumerateDeviceExtensionProperties(devices[i], NULL, &n, extensions));
  /* Same Vulkan 1.0 baseline as Mesa's pinned profile. Core promotions count. */
  struct { const char *name; uint32_t core; } requirements[] = {
   {"VK_KHR_maintenance1", VK_API_VERSION_1_1},
   {"VK_KHR_create_renderpass2", VK_API_VERSION_1_2},
   {"VK_KHR_imageless_framebuffer", VK_API_VERSION_1_2},
   {"VK_KHR_timeline_semaphore", VK_API_VERSION_1_2},
   {"VK_EXT_custom_border_color", 0}, {"VK_EXT_line_rasterization", 0},
   {"VK_KHR_swapchain_mutable_format", 0}, {"VK_KHR_incremental_present", 0},
   {"VK_EXT_border_color_swizzle", 0},
   {"VK_KHR_descriptor_update_template", VK_API_VERSION_1_1},
   {"VK_EXT_scalar_block_layout", VK_API_VERSION_1_2},
  };
  for (unsigned j = 0; j < sizeof(requirements) / sizeof(requirements[0]); ++j)
   report(requirements[j].name, extension(extensions, n, requirements[j].name)
    || (requirements[j].core && properties.apiVersion >= requirements[j].core));
  VkPhysicalDeviceFeatures core;
  vkGetPhysicalDeviceFeatures(devices[i], &core);
  report("robustBufferAccess", core.robustBufferAccess);
  report("logicOp", core.logicOp);
  report("fillModeNonSolid", core.fillModeNonSolid);
  report("alphaToOne", core.alphaToOne);
  report("shaderClipDistance", core.shaderClipDistance);
  VkPhysicalDeviceTimelineSemaphoreFeatures timeline = {
   .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_TIMELINE_SEMAPHORE_FEATURES};
  VkPhysicalDeviceImagelessFramebufferFeatures imageless = {
   .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_IMAGELESS_FRAMEBUFFER_FEATURES};
  VkPhysicalDeviceScalarBlockLayoutFeatures scalar = {
   .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SCALAR_BLOCK_LAYOUT_FEATURES};
  VkPhysicalDeviceCustomBorderColorFeaturesEXT border = {
   .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_CUSTOM_BORDER_COLOR_FEATURES_EXT};
  VkPhysicalDeviceBorderColorSwizzleFeaturesEXT swizzle = {
   .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_BORDER_COLOR_SWIZZLE_FEATURES_EXT};
  VkPhysicalDeviceLineRasterizationFeaturesEXT lines = {
   .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_LINE_RASTERIZATION_FEATURES_EXT};
  VkPhysicalDeviceFeatures2 chain = {.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
  /* Query only structures supported by the device/extension, keeping pNext valid. */
#define LINK(node, ext, version) do { if (extension(extensions, n, ext) \
 || promoted(properties.apiVersion, version)) { \
 node.pNext = chain.pNext; chain.pNext = &node; } } while (0)
  LINK(timeline, "VK_KHR_timeline_semaphore", VK_API_VERSION_1_2);
  LINK(imageless, "VK_KHR_imageless_framebuffer", VK_API_VERSION_1_2);
  LINK(scalar, "VK_EXT_scalar_block_layout", VK_API_VERSION_1_2);
  LINK(border, "VK_EXT_custom_border_color", 0);
  LINK(swizzle, "VK_EXT_border_color_swizzle", 0);
  LINK(lines, "VK_EXT_line_rasterization", 0);
  if (queries2 && features2) features2(devices[i], &chain);
  report("timelineSemaphore", timeline.timelineSemaphore);
  report("imagelessFramebuffer", imageless.imagelessFramebuffer);
  report("scalarBlockLayout", scalar.scalarBlockLayout);
  report("customBorderColorWithoutFormat", border.customBorderColorWithoutFormat);
  report("borderColorSwizzleFromImage", swizzle.borderColorSwizzleFromImage);
  report("rectangularLines", lines.rectangularLines);
  printf("SUMMARY %u missing capability checks; rendering not tested\n", missing);
  total += missing;
  free(extensions);
 }
 free(devices);
 vkDestroyInstance(instance, NULL);
 return total ? 2 : 0;
}
