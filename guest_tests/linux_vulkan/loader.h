#ifndef SCARLET_VULKAN_LOADER_H
#define SCARLET_VULKAN_LOADER_H
#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#endif
#include <vulkan/vulkan.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
static HMODULE vulkan_library;
#define SYMBOL(name) GetProcAddress(vulkan_library, #name)
#else
static void *vulkan_library;
#define SYMBOL(name) dlsym(vulkan_library, #name)
#endif
static int load_vulkan(void) {
#ifdef _WIN32
 vulkan_library = LoadLibraryA("vulkan-1.dll");
#else
 vulkan_library = dlopen("libvulkan.so.1", RTLD_NOW | RTLD_LOCAL);
#endif
 if (!vulkan_library) { fprintf(stderr, "Cannot load Vulkan loader\n"); return 1; }
 return 0;
}
/* memcpy avoids incompatible-function-cast warnings on the Win32 ABI. */
#define LOAD(name) do { __typeof__(SYMBOL(name)) symbol = SYMBOL(name); \
 _Static_assert(sizeof(symbol) == sizeof(name), "function pointer size"); \
 memcpy(&name, &symbol, sizeof(name)); \
 if (!name) { fprintf(stderr, "Missing %s\n", #name); return 1; } } while (0);
#define DECLARE(name) static PFN_##name name;
#define CHECK(call) do { VkResult result = (call); if (result != VK_SUCCESS) { \
 fprintf(stderr, "%s: VkResult %d\n", #call, result); return 1; } } while (0)
#endif
