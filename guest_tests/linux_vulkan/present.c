/* Standard Linux Wayland and Win64 WSI; exercise child surfaces, release and
 * swapchain replacement. No direct SGFX API or private Vulkan loader. */
#define _GNU_SOURCE
#define VK_NO_PROTOTYPES
#ifdef _WIN32
#define VK_USE_PLATFORM_WIN32_KHR
#else
#define VK_USE_PLATFORM_WAYLAND_KHR
#include <wayland-client.h>
#include "xdg-shell-client.h"
#include <sys/mman.h>
#include <unistd.h>
#include <fcntl.h>
#endif
#include "loader.h"

#define FUNCTIONS(X) \
 X(vkCreateInstance) X(vkDestroyInstance) X(vkEnumeratePhysicalDevices) \
 X(vkGetPhysicalDeviceProperties) X(vkGetPhysicalDeviceQueueFamilyProperties) \
 X(vkCreateDevice) X(vkDestroyDevice) X(vkGetDeviceQueue) X(vkGetInstanceProcAddr) \
 X(vkGetPhysicalDeviceSurfaceSupportKHR) X(vkGetPhysicalDeviceSurfaceCapabilitiesKHR) \
 X(vkCreateSwapchainKHR) X(vkDestroySwapchainKHR) X(vkGetSwapchainImagesKHR) \
 X(vkAcquireNextImageKHR) X(vkQueuePresentKHR) X(vkDestroySurfaceKHR) \
 X(vkCreateCommandPool) X(vkDestroyCommandPool) X(vkAllocateCommandBuffers) \
 X(vkFreeCommandBuffers) X(vkBeginCommandBuffer) X(vkEndCommandBuffer) \
 X(vkCmdPipelineBarrier) X(vkCmdClearColorImage) X(vkQueueSubmit) X(vkQueueWaitIdle) \
 X(vkCreateFence) X(vkDestroyFence) X(vkWaitForFences) X(vkResetFences)
FUNCTIONS(DECLARE)

#ifdef _WIN32
static HWND window;
static HINSTANCE app;
static void pump(void) {
 MSG message;
 while (PeekMessageA(&message, NULL, 0, 0, PM_REMOVE)) {
  TranslateMessage(&message); DispatchMessageA(&message);
 }
 Sleep(16);
}
static LRESULT CALLBACK procedure(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
 return DefWindowProcA(hwnd, msg, wp, lp);
}
static int create_window(void) {
 app = GetModuleHandleA(NULL);
 WNDCLASSA cls = {.lpfnWndProc=procedure, .hInstance=app, .lpszClassName="ScarletVkProbe"};
 if (!RegisterClassA(&cls)) return 1;
 RECT size = {0,0,128,96}; AdjustWindowRect(&size, WS_OVERLAPPEDWINDOW, FALSE);
 window = CreateWindowA(cls.lpszClassName, "Wine Vulkan GPU", WS_OVERLAPPEDWINDOW,
  80,80,size.right-size.left,size.bottom-size.top,NULL,NULL,app,NULL);
 if (!window) return 1;
 ShowWindow(window, SW_SHOW); UpdateWindow(window);
 for (int i=0; i<5; ++i) pump();
 return 0;
}
static int create_surface(VkInstance instance, VkSurfaceKHR *surface) {
 PFN_vkCreateWin32SurfaceKHR create = (PFN_vkCreateWin32SurfaceKHR)
  vkGetInstanceProcAddr(instance, "vkCreateWin32SurfaceKHR");
 if (!create) return 1;
 VkWin32SurfaceCreateInfoKHR info = {.sType=VK_STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR,
  .hinstance=app,.hwnd=window};
 CHECK(create(instance,&info,NULL,surface));
 return 0;
}
static void resize_window(unsigned width, unsigned height) {
 RECT size = {0,0,width,height}; AdjustWindowRect(&size, WS_OVERLAPPEDWINDOW, FALSE);
 SetWindowPos(window,NULL,0,0,size.right-size.left,size.bottom-size.top,SWP_NOMOVE|SWP_NOZORDER);
 for (int i=0; i<5; ++i) pump();
}
static void destroy_window(void) { DestroyWindow(window); }
#define PLATFORM_EXTENSION VK_KHR_WIN32_SURFACE_EXTENSION_NAME
#else
static struct wl_display *display;
static struct wl_compositor *compositor;
static struct wl_subcompositor *subcompositor;
static struct wl_shm *shm;
static struct xdg_wm_base *shell;
static struct wl_surface *root, *child;
static struct wl_subsurface *subsurface;
static struct xdg_surface *xdg;
static struct xdg_toplevel *toplevel;
static struct wl_buffer *root_buffer;
static int configured;
static void ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
 (void)data; xdg_wm_base_pong(base,serial);
}
static const struct xdg_wm_base_listener shell_listener = {ping};
static void configure(void *data, struct xdg_surface *surface, uint32_t serial) {
 (void)data; xdg_surface_ack_configure(surface,serial); configured=1;
}
static const struct xdg_surface_listener xdg_listener = {configure};
static void top_configure(void *data, struct xdg_toplevel *top, int32_t w, int32_t h, struct wl_array *states) {
 (void)data; (void)top; (void)w; (void)h; (void)states;
}
static void top_close(void *data, struct xdg_toplevel *top) { (void)data; (void)top; }
static const struct xdg_toplevel_listener top_listener = {.configure=top_configure,.close=top_close};
static void global(void *data, struct wl_registry *reg, uint32_t id, const char *name, uint32_t version) {
 (void)data;
 if (!strcmp(name,"wl_compositor")) compositor=wl_registry_bind(reg,id,&wl_compositor_interface,version<4?version:4);
 if (!strcmp(name,"wl_subcompositor")) subcompositor=wl_registry_bind(reg,id,&wl_subcompositor_interface,1);
 if (!strcmp(name,"wl_shm")) shm=wl_registry_bind(reg,id,&wl_shm_interface,1);
 if (!strcmp(name,"xdg_wm_base")) {
  shell=wl_registry_bind(reg,id,&xdg_wm_base_interface,1);
  xdg_wm_base_add_listener(shell,&shell_listener,NULL);
 }
}
static void removed(void *data,struct wl_registry *reg,uint32_t id) {(void)data;(void)reg;(void)id;}
static const struct wl_registry_listener registry_listener={global,removed};
static void pump(void) {
 wl_display_roundtrip(display);
 usleep(16000);
}
static int create_window(void) {
 display=wl_display_connect(NULL); if (!display) return 1;
 struct wl_registry *registry=wl_display_get_registry(display);
 wl_registry_add_listener(registry,&registry_listener,NULL);
 if (wl_display_roundtrip(display)<0 || !compositor || !shell || !shm || !subcompositor) return 1;
 wl_registry_destroy(registry);
 root=wl_compositor_create_surface(compositor);
 xdg=xdg_wm_base_get_xdg_surface(shell,root);
 xdg_surface_add_listener(xdg,&xdg_listener,NULL);
 toplevel=xdg_surface_get_toplevel(xdg);
 xdg_toplevel_add_listener(toplevel,&top_listener,NULL);
 xdg_toplevel_set_title(toplevel,"Linux Vulkan GPU child surface");
 wl_surface_commit(root);
 while (!configured) if (wl_display_dispatch(display)<0) return 1;
 char path[]="/var/tmp/vulkan-shm-XXXXXX";
 int fd=mkstemp(path); if (fd<0) return 1;
 unlink(path);
 unsigned bytes=256*180*4;
 if (ftruncate(fd,bytes)<0) return 1;
 uint32_t *pixels=mmap(NULL,bytes,PROT_READ|PROT_WRITE,MAP_SHARED,fd,0);
 if (pixels==MAP_FAILED) return 1;
 for (unsigned i=0; i<bytes/4; ++i) pixels[i]=0xff202028;
 struct wl_shm_pool *pool=wl_shm_create_pool(shm,fd,bytes);
 root_buffer=wl_shm_pool_create_buffer(pool,0,256,180,256*4,WL_SHM_FORMAT_ARGB8888);
 wl_shm_pool_destroy(pool); close(fd); munmap(pixels,bytes);
 wl_surface_attach(root,root_buffer,0,0); wl_surface_damage(root,0,0,256,180);
 wl_surface_commit(root);
 child=wl_compositor_create_surface(compositor);
 subsurface=wl_subcompositor_get_subsurface(subcompositor,child,root);
 wl_subsurface_set_position(subsurface,24,24);
 wl_subsurface_set_desync(subsurface);
 wl_surface_commit(root);
 pump(); return 0;
}
static int create_surface(VkInstance instance, VkSurfaceKHR *surface) {
 PFN_vkCreateWaylandSurfaceKHR create=(PFN_vkCreateWaylandSurfaceKHR)
  vkGetInstanceProcAddr(instance,"vkCreateWaylandSurfaceKHR");
 PFN_vkGetPhysicalDeviceWaylandPresentationSupportKHR supports=(PFN_vkGetPhysicalDeviceWaylandPresentationSupportKHR)
  vkGetInstanceProcAddr(instance,"vkGetPhysicalDeviceWaylandPresentationSupportKHR");
 if (!create || !supports) return 1;
 VkWaylandSurfaceCreateInfoKHR info={.sType=VK_STRUCTURE_TYPE_WAYLAND_SURFACE_CREATE_INFO_KHR,
  .display=display,.surface=child};
 CHECK(create(instance,&info,NULL,surface)); return 0;
}
static void resize_window(unsigned w,unsigned h) { (void)w;(void)h; pump(); }
static void destroy_window(void) {
 wl_subsurface_destroy(subsurface); wl_surface_destroy(child); wl_buffer_destroy(root_buffer);
 xdg_toplevel_destroy(toplevel); xdg_surface_destroy(xdg); wl_surface_destroy(root);
 xdg_wm_base_destroy(shell); wl_shm_destroy(shm); wl_subcompositor_destroy(subcompositor);
 wl_compositor_destroy(compositor); wl_display_flush(display); wl_display_disconnect(display);
}
#define PLATFORM_EXTENSION VK_KHR_WAYLAND_SURFACE_EXTENSION_NAME
#endif

int main(void) {
 setvbuf(stdout,NULL,_IONBF,0);
 if (load_vulkan()) return 1;
 FUNCTIONS(LOAD)
 if (create_window()) { fprintf(stderr,"Window creation failed\n"); return 1; }
 const char *extensions[]={VK_KHR_SURFACE_EXTENSION_NAME,PLATFORM_EXTENSION};
 VkApplicationInfo app_info={.sType=VK_STRUCTURE_TYPE_APPLICATION_INFO,
  .pApplicationName="Scarlet Vulkan present",.apiVersion=VK_API_VERSION_1_0};
 VkInstanceCreateInfo instance_info={.sType=VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
  .pApplicationInfo=&app_info,.enabledExtensionCount=2,.ppEnabledExtensionNames=extensions};
 VkInstance instance; CHECK(vkCreateInstance(&instance_info,NULL,&instance));
 VkSurfaceKHR surface; if (create_surface(instance,&surface)) return 1;
 uint32_t count=8; VkPhysicalDevice physicals[8];
 CHECK(vkEnumeratePhysicalDevices(instance,&count,physicals)); if (!count) return 1;
 VkPhysicalDevice physical=physicals[0]; VkPhysicalDeviceProperties properties;
 vkGetPhysicalDeviceProperties(physical,&properties); printf("Vulkan device: %s\n",properties.deviceName);
 uint32_t family_count=8; VkQueueFamilyProperties families[8];
 vkGetPhysicalDeviceQueueFamilyProperties(physical,&family_count,families);
 uint32_t family=UINT32_MAX;
 for (uint32_t i=0;i<family_count;++i) {
  VkBool32 support=0; CHECK(vkGetPhysicalDeviceSurfaceSupportKHR(physical,i,surface,&support));
  if ((families[i].queueFlags&VK_QUEUE_GRAPHICS_BIT) && support) {family=i;break;}
 }
 if (family==UINT32_MAX) return 1;
 float priority=1; VkDeviceQueueCreateInfo queue_info={.sType=VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
  .queueFamilyIndex=family,.queueCount=1,.pQueuePriorities=&priority};
 const char *swapchain_extension=VK_KHR_SWAPCHAIN_EXTENSION_NAME;
 VkDeviceCreateInfo device_info={.sType=VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,.queueCreateInfoCount=1,
  .pQueueCreateInfos=&queue_info,.enabledExtensionCount=1,.ppEnabledExtensionNames=&swapchain_extension};
 VkDevice device; CHECK(vkCreateDevice(physical,&device_info,NULL,&device));
 VkQueue queue; vkGetDeviceQueue(device,family,0,&queue);
 VkCommandPoolCreateInfo pool_info={.sType=VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,.queueFamilyIndex=family};
 VkCommandPool pool; CHECK(vkCreateCommandPool(device,&pool_info,NULL,&pool));
 VkFenceCreateInfo fence_info={.sType=VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
 VkFence fence; CHECK(vkCreateFence(device,&fence_info,NULL,&fence));
 VkSwapchainKHR swapchain=VK_NULL_HANDLE;
 for (unsigned generation=0;generation<2;++generation) {
  unsigned width=generation?176:128,height=generation?112:96;
  resize_window(width,height);
  VkSurfaceCapabilitiesKHR caps; CHECK(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(physical,surface,&caps));
  VkSwapchainCreateInfoKHR swap_info={.sType=VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR,
   .surface=surface,.minImageCount=3,.imageFormat=VK_FORMAT_B8G8R8A8_UNORM,
   .imageColorSpace=VK_COLOR_SPACE_SRGB_NONLINEAR_KHR,.imageExtent={width,height},.imageArrayLayers=1,
   .imageUsage=VK_IMAGE_USAGE_TRANSFER_DST_BIT,.imageSharingMode=VK_SHARING_MODE_EXCLUSIVE,
   .preTransform=VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR,.compositeAlpha=VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR,
   .presentMode=VK_PRESENT_MODE_FIFO_KHR,.clipped=VK_TRUE,.oldSwapchain=swapchain};
  VkSwapchainKHR next; CHECK(vkCreateSwapchainKHR(device,&swap_info,NULL,&next));
  if (swapchain) vkDestroySwapchainKHR(device,swapchain,NULL);
  swapchain=next;
  count=8; VkImage images[8]; CHECK(vkGetSwapchainImagesKHR(device,swapchain,&count,images));
  unsigned char used[8]={0};
  for (unsigned frame=0;frame<20;++frame) {
   uint32_t index; CHECK(vkAcquireNextImageKHR(device,swapchain,5000000000ULL,VK_NULL_HANDLE,fence,&index));
   CHECK(vkWaitForFences(device,1,&fence,VK_TRUE,5000000000ULL)); CHECK(vkResetFences(device,1,&fence));
   VkCommandBufferAllocateInfo allocate={.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
    .commandPool=pool,.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY,.commandBufferCount=1};
   VkCommandBuffer command; CHECK(vkAllocateCommandBuffers(device,&allocate,&command));
   VkCommandBufferBeginInfo begin={.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
   CHECK(vkBeginCommandBuffer(command,&begin));
   VkImageMemoryBarrier barrier={.sType=VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
    .srcAccessMask=0,.dstAccessMask=VK_ACCESS_TRANSFER_WRITE_BIT,
    .oldLayout=used[index]?VK_IMAGE_LAYOUT_PRESENT_SRC_KHR:VK_IMAGE_LAYOUT_UNDEFINED,
    .newLayout=VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,.srcQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,
    .dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED,.image=images[index],
    .subresourceRange={VK_IMAGE_ASPECT_COLOR_BIT,0,1,0,1}};
   vkCmdPipelineBarrier(command,VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT,
    0,0,NULL,0,NULL,1,&barrier);
   VkClearColorValue color={.float32={generation?0.05f:0.9f,generation?0.65f:0.12f,
    (float)frame/20.0f,1}};
   vkCmdClearColorImage(command,images[index],VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,&color,1,&barrier.subresourceRange);
   barrier.srcAccessMask=VK_ACCESS_TRANSFER_WRITE_BIT;barrier.dstAccessMask=0;
   barrier.oldLayout=VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;barrier.newLayout=VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
   vkCmdPipelineBarrier(command,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
    0,0,NULL,0,NULL,1,&barrier);
   CHECK(vkEndCommandBuffer(command));
   VkSubmitInfo submit={.sType=VK_STRUCTURE_TYPE_SUBMIT_INFO,.commandBufferCount=1,.pCommandBuffers=&command};
   CHECK(vkQueueSubmit(queue,1,&submit,VK_NULL_HANDLE)); CHECK(vkQueueWaitIdle(queue));
   VkPresentInfoKHR present={.sType=VK_STRUCTURE_TYPE_PRESENT_INFO_KHR,.swapchainCount=1,
    .pSwapchains=&swapchain,.pImageIndices=&index}; CHECK(vkQueuePresentKHR(queue,&present));
   used[index]=1; vkFreeCommandBuffers(device,pool,1,&command); pump();
  }
  printf("PASS: generation %u, 20 GPU frames, %ux%u\n",generation,width,height);
 }
 for (int i=0;i<180;++i) pump();
 vkDestroySwapchainKHR(device,swapchain,NULL); vkDestroyFence(device,fence,NULL);
 vkDestroyCommandPool(device,pool,NULL); vkDestroyDevice(device,NULL);
 vkDestroySurfaceKHR(instance,surface,NULL); vkDestroyInstance(instance,NULL); destroy_window();
 puts("PASS: Vulkan presentation, image reuse and swapchain replacement"); return 0;
}
