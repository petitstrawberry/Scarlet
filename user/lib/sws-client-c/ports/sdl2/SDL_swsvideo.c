/* SPDX-License-Identifier: MIT
 * SDL2 fullscreen Vulkan video driver for Scarlet SWS.
 * The ordinary Vulkan loader owns rendering through VK_KHR_display. */
#include "../../SDL_internal.h"
#ifdef SDL_VIDEO_DRIVER_SWS
#include "../SDL_sysvideo.h"
#include "../../events/SDL_events_c.h"
#include "../../events/SDL_keyboard_c.h"
#include "../../events/SDL_mouse_c.h"
#include "../../events/scancodes_linux.h"
#include "SDL_loadso.h"
#include "SDL_hints.h"
#include "sws_client.h"
#include <linux/input-event-codes.h>

typedef struct SwsVideo {
    SDL_Window *window;
    SwsDisplay display;
    uint32_t window_id;
    int mouse_x, mouse_y;
} SwsVideo;

static int SWS_VideoInit(_THIS)
{
    SwsVideo *data = _this->driverdata;
    SDL_DisplayMode mode;
    if (sws_get_display(&data->display) < 0)
        return SDL_SetError("Cannot connect to the SWS display");
    SDL_zero(mode);
    mode.format = SDL_PIXELFORMAT_BGRA8888;
    mode.w = data->display.width;
    mode.h = data->display.height;
    mode.refresh_rate = 60;
    if (SDL_AddBasicVideoDisplay(&mode) < 0) return -1;
    return 0;
}

static void SWS_GetDisplayModes(_THIS, SDL_VideoDisplay *display)
{
    SDL_AddDisplayMode(display, &display->desktop_mode);
}

static int SWS_SetDisplayMode(_THIS, SDL_VideoDisplay *display, SDL_DisplayMode *mode)
{
    if (mode->w != display->desktop_mode.w || mode->h != display->desktop_mode.h)
        return SDL_SetError("SWS Vulkan uses the native display mode");
    return 0;
}

static int SWS_CreateWindow(_THIS, SDL_Window *window)
{
    SwsVideo *data = _this->driverdata;
    if (data->window) return SDL_SetError("Only one SWS display window is supported");
    if (!(window->flags & SDL_WINDOW_VULKAN))
        return SDL_SetError("The SWS video driver requires a Vulkan window");
    data->window = window;
    window->w = data->display.width;
    window->h = data->display.height;
    SDL_SetMouseFocus(window);
    SDL_SetKeyboardFocus(window);
    return 0;
}

static void SWS_DestroyWindow(_THIS, SDL_Window *window)
{
    SwsVideo *data = _this->driverdata;
    if (data->window_id) sws_window_pointer_lock(data->window_id, 0);
    /* The Vulkan display surface owns the compositor window. */
    data->window = NULL;
    data->window_id = 0;
}

static int SWS_RelativeMouseMode(SDL_bool enabled)
{
    SDL_VideoDevice *device = SDL_GetVideoDevice();
    SwsVideo *data = device->driverdata;
    if (data->window_id && sws_window_pointer_lock(data->window_id, enabled) < 0)
        return SDL_SetError("SWS pointer lock failed");
    return 0;
}

static void SWS_PumpEvents(_THIS)
{
    SwsVideo *data = _this->driverdata;
    SwsEvent event;
    int result;
    if (!data->window) return;
    while ((result = sws_poll_event(&event)) == 1) {
        if (!data->window_id) {
            data->window_id = event.window_id;
            SWS_RelativeMouseMode(SDL_GetRelativeMouseMode());
        }
        if (event.window_id != data->window_id) continue;
        if (event.kind == SWS_EVENT_DESTROYED) {
            SDL_SendWindowEvent(data->window, SDL_WINDOWEVENT_CLOSE, 0, 0);
        } else if (event.kind == SWS_EVENT_FOCUS) {
            SDL_SetKeyboardFocus(event.value ? data->window : NULL);
            SDL_SetMouseFocus(event.value ? data->window : NULL);
        } else if (event.kind == SWS_EVENT_CONFIGURE) {
            SDL_VideoDisplay *display = SDL_GetDisplayForWindow(data->window);
            if (display && event.width && event.height &&
                (data->display.width != event.width || data->display.height != event.height)) {
                SDL_DisplayMode mode = display->desktop_mode;
                mode.w = event.width;
                mode.h = event.height;
                data->display.width = event.width;
                data->display.height = event.height;
                SDL_ResetDisplayModes(SDL_GetIndexOfDisplay(display));
                SDL_SetDesktopDisplayMode(display, &mode);
                SDL_SetCurrentDisplayMode(display, &mode);
            }
            SDL_SendWindowEvent(data->window, SDL_WINDOWEVENT_RESIZED, event.width, event.height);
        } else if (event.kind == SWS_EVENT_INPUT) {
            if (event.type == EV_KEY && event.code >= BTN_LEFT && event.code <= BTN_EXTRA) {
                static const Uint8 buttons[] = {SDL_BUTTON_LEFT, SDL_BUTTON_RIGHT,
                    SDL_BUTTON_MIDDLE, SDL_BUTTON_X1, SDL_BUTTON_X2};
                SDL_SendMouseButton(data->window, 0, event.value ? SDL_PRESSED : SDL_RELEASED,
                    buttons[event.code - BTN_LEFT]);
            } else if (event.type == EV_KEY && event.code < SDL_arraysize(linux_scancode_table)) {
                SDL_SendKeyboardKey(event.value ? SDL_PRESSED : SDL_RELEASED,
                    linux_scancode_table[event.code]);
            } else if (event.type == EV_REL) {
                if (event.code == REL_X || event.code == REL_Y)
                    SDL_SendMouseMotion(data->window, 0, SDL_TRUE,
                        event.code == REL_X ? event.value : 0, event.code == REL_Y ? event.value : 0);
                else if (event.code == REL_WHEEL || event.code == REL_HWHEEL)
                    SDL_SendMouseWheel(data->window, 0,
                        event.code == REL_HWHEEL ? event.value : 0,
                        event.code == REL_WHEEL ? event.value : 0, SDL_MOUSEWHEEL_NORMAL);
            } else if (event.type == EV_ABS) {
                if (event.code == ABS_X) data->mouse_x = event.value;
                if (event.code == ABS_Y) data->mouse_y = event.value;
                if (event.code == ABS_X || event.code == ABS_Y)
                    SDL_SendMouseMotion(data->window, 0, SDL_FALSE, data->mouse_x, data->mouse_y);
            }
        }
    }
    if (result < 0) SDL_SendWindowEvent(data->window, SDL_WINDOWEVENT_CLOSE, 0, 0);
}

static void SWS_Vulkan_UnloadLibrary(_THIS)
{
    if (_this->vulkan_config.loader_handle) {
        SDL_UnloadObject(_this->vulkan_config.loader_handle);
        _this->vulkan_config.loader_handle = NULL;
    }
}

static int SWS_Vulkan_LoadLibrary(_THIS, const char *path)
{
    PFN_vkGetInstanceProcAddr get_proc;
    VkExtensionProperties *extensions;
    Uint32 count, i;
    SDL_bool surface = SDL_FALSE, display = SDL_FALSE;
    if (_this->vulkan_config.loader_handle) return SDL_SetError("Vulkan already loaded");
    if (!path) path = SDL_getenv("SDL_VULKAN_LIBRARY");
    if (!path) path = "libvulkan.so.1";
    _this->vulkan_config.loader_handle = SDL_LoadObject(path);
    if (!_this->vulkan_config.loader_handle) return -1;
    get_proc = (PFN_vkGetInstanceProcAddr)SDL_LoadFunction(
        _this->vulkan_config.loader_handle, "vkGetInstanceProcAddr");
    if (!get_proc) goto fail;
    _this->vulkan_config.vkGetInstanceProcAddr = (void *)get_proc;
    _this->vulkan_config.vkEnumerateInstanceExtensionProperties =
        (void *)get_proc(VK_NULL_HANDLE, "vkEnumerateInstanceExtensionProperties");
    if (!_this->vulkan_config.vkEnumerateInstanceExtensionProperties) goto fail;
    extensions = SDL_Vulkan_CreateInstanceExtensionsList(
        (PFN_vkEnumerateInstanceExtensionProperties)_this->vulkan_config.vkEnumerateInstanceExtensionProperties, &count);
    if (!extensions) goto fail;
    for (i = 0; i < count; ++i) {
        if (!SDL_strcmp(extensions[i].extensionName, VK_KHR_SURFACE_EXTENSION_NAME)) surface = SDL_TRUE;
        if (!SDL_strcmp(extensions[i].extensionName, VK_KHR_DISPLAY_EXTENSION_NAME)) display = SDL_TRUE;
    }
    SDL_free(extensions);
    if (!surface || !display) {
        SDL_SetError("Vulkan does not provide VK_KHR_surface and VK_KHR_display");
        goto fail;
    }
    SDL_strlcpy(_this->vulkan_config.loader_path, path, sizeof(_this->vulkan_config.loader_path));
    return 0;
fail:
    SWS_Vulkan_UnloadLibrary(_this);
    return -1;
}

static SDL_bool SWS_Vulkan_GetInstanceExtensions(_THIS, SDL_Window *window,
    unsigned *count, const char **names)
{
    static const char *const extensions[] = {VK_KHR_SURFACE_EXTENSION_NAME, VK_KHR_DISPLAY_EXTENSION_NAME};
    return SDL_Vulkan_GetInstanceExtensions_Helper(count, names, SDL_arraysize(extensions), extensions);
}

static SDL_bool SWS_Vulkan_CreateSurface(_THIS, SDL_Window *window, VkInstance instance, VkSurfaceKHR *surface)
{
    return SDL_Vulkan_Display_CreateSurface(_this->vulkan_config.vkGetInstanceProcAddr, instance, surface);
}

static void SWS_Vulkan_GetDrawableSize(_THIS, SDL_Window *window, int *w, int *h)
{
    if (w) *w = window->w;
    if (h) *h = window->h;
}

static void SWS_VideoQuit(_THIS) { SDL_GetMouse()->SetRelativeMouseMode = NULL; }
static void SWS_DeleteDevice(SDL_VideoDevice *device)
{
    SDL_free(device->driverdata);
    SDL_free(device);
}
static SDL_VideoDevice *SWS_CreateDevice(void)
{
    SDL_VideoDevice *device = SDL_calloc(1, sizeof(*device));
    if (!device) { SDL_OutOfMemory(); return NULL; }
    device->driverdata = SDL_calloc(1, sizeof(SwsVideo));
    if (!device->driverdata) { SDL_free(device); SDL_OutOfMemory(); return NULL; }
    device->VideoInit = SWS_VideoInit;
    device->VideoQuit = SWS_VideoQuit;
    device->GetDisplayModes = SWS_GetDisplayModes;
    device->SetDisplayMode = SWS_SetDisplayMode;
    device->CreateSDLWindow = SWS_CreateWindow;
    device->DestroyWindow = SWS_DestroyWindow;
    device->PumpEvents = SWS_PumpEvents;
    device->Vulkan_LoadLibrary = SWS_Vulkan_LoadLibrary;
    device->Vulkan_UnloadLibrary = SWS_Vulkan_UnloadLibrary;
    device->Vulkan_GetInstanceExtensions = SWS_Vulkan_GetInstanceExtensions;
    device->Vulkan_CreateSurface = SWS_Vulkan_CreateSurface;
    device->Vulkan_GetDrawableSize = SWS_Vulkan_GetDrawableSize;
    device->free = SWS_DeleteDevice;
    SDL_GetMouse()->SetRelativeMouseMode = SWS_RelativeMouseMode;
    return device;
}
VideoBootStrap SWS_bootstrap = {"sws", "Scarlet SWS Vulkan display", SWS_CreateDevice, NULL};
#endif
