/* SPDX-License-Identifier: GPL-2.0-or-later
 * vkQuake2 OS window/input adapter for Scarlet's Linux ABI Environment.
 * Rendering uses the upstream Vulkan renderer and the system Vulkan loader. */
#include "ref_vk/vk_local.h"
#include "client/keys.h"
#include "linux/rw_linux.h"
#include "linux/vk_linux.h"
#include "sws_client.h"
#include <linux/input-event-codes.h>
#include <stdint.h>

vkwstate_t vkw_state;
static in_state_t *input_state;
static Key_Event_fp_t key_event;
static uint32_t window_id;
static qboolean mouse_enabled, focused;
static int mouse_x, mouse_y;
static qboolean pressed[256];
static cvar_t *sensitivity, *m_yaw, *m_pitch, *m_side, *m_forward, *freelook;

static int key_code(uint16_t code) {
    static const unsigned char ascii[128] = {
        [KEY_1]='1', [KEY_2]='2', [KEY_3]='3', [KEY_4]='4', [KEY_5]='5',
        [KEY_6]='6', [KEY_7]='7', [KEY_8]='8', [KEY_9]='9', [KEY_0]='0',
        [KEY_Q]='q', [KEY_W]='w', [KEY_E]='e', [KEY_R]='r', [KEY_T]='t',
        [KEY_Y]='y', [KEY_U]='u', [KEY_I]='i', [KEY_O]='o', [KEY_P]='p',
        [KEY_A]='a', [KEY_S]='s', [KEY_D]='d', [KEY_F]='f', [KEY_G]='g',
        [KEY_H]='h', [KEY_J]='j', [KEY_K]='k', [KEY_L]='l', [KEY_Z]='z',
        [KEY_X]='x', [KEY_C]='c', [KEY_V]='v', [KEY_B]='b', [KEY_N]='n', [KEY_M]='m',
        [KEY_MINUS]='-', [KEY_EQUAL]='=', [KEY_LEFTBRACE]='[', [KEY_RIGHTBRACE]=']',
        [KEY_SEMICOLON]=';', [KEY_APOSTROPHE]='\'', [KEY_GRAVE]='`',
        [KEY_BACKSLASH]='\\', [KEY_COMMA]=',', [KEY_DOT]='.', [KEY_SLASH]='/',
        [KEY_SPACE]=' '};
    if (code < sizeof(ascii) && ascii[code]) return ascii[code];
    if (code >= KEY_F1 && code <= KEY_F10) return K_F1 + code - KEY_F1;
    switch (code) {
    case KEY_ESC: return K_ESCAPE; case KEY_ENTER: return K_ENTER;
    case KEY_BACKSPACE: return K_BACKSPACE; case KEY_TAB: return K_TAB;
    case KEY_UP: return K_UPARROW; case KEY_DOWN: return K_DOWNARROW;
    case KEY_LEFT: return K_LEFTARROW; case KEY_RIGHT: return K_RIGHTARROW;
    case KEY_LEFTALT: case KEY_RIGHTALT: return K_ALT;
    case KEY_LEFTCTRL: case KEY_RIGHTCTRL: return K_CTRL;
    case KEY_LEFTSHIFT: case KEY_RIGHTSHIFT: return K_SHIFT;
    case KEY_F11: return K_F11; case KEY_F12: return K_F12;
    case KEY_INSERT: return K_INS; case KEY_DELETE: return K_DEL;
    case KEY_PAGEUP: return K_PGUP; case KEY_PAGEDOWN: return K_PGDN;
    case KEY_HOME: return K_HOME; case KEY_END: return K_END;
    case KEY_PAUSE: return K_PAUSE;
    case BTN_LEFT: return K_MOUSE1; case BTN_RIGHT: return K_MOUSE2;
    case BTN_MIDDLE: return K_MOUSE3; case BTN_SIDE: return K_MOUSE4;
    case BTN_EXTRA: return K_MOUSE5;
    default: return 0;
    }
}
static void send_key(int key, qboolean down) {
    if (!key || key >= 256 || !key_event) return;
    pressed[key] = down;
    key_event(key, down);
}
static void pointer_lock(void) {
    if (window_id) sws_window_pointer_lock(window_id, mouse_enabled && focused);
    mouse_x = mouse_y = 0;
}
void RW_IN_Init(in_state_t *state) {
    input_state = state;
    sensitivity = ri.Cvar_Get("sensitivity", "3", CVAR_ARCHIVE);
    m_yaw = ri.Cvar_Get("m_yaw", "0.022", 0);
    m_pitch = ri.Cvar_Get("m_pitch", "0.022", 0);
    m_side = ri.Cvar_Get("m_side", "0.8", 0);
    m_forward = ri.Cvar_Get("m_forward", "1", 0);
    freelook = ri.Cvar_Get("freelook", "1", 0);
    mouse_enabled = true;
    pointer_lock();
}
void RW_IN_Shutdown(void) { mouse_enabled = false; pointer_lock(); input_state = NULL; }
void RW_IN_Commands(void) {}
void RW_IN_Frame(void) {}
void RW_IN_Activate(qboolean active) { mouse_enabled = active; pointer_lock(); }
void RW_IN_Move(usercmd_t *command) {
    if (!input_state || !mouse_enabled || !focused) { mouse_x = mouse_y = 0; return; }
    float x = mouse_x * sensitivity->value, y = mouse_y * sensitivity->value;
    if (*input_state->in_strafe_state & 1) command->sidemove += m_side->value * x;
    else input_state->viewangles[YAW] -= m_yaw->value * x;
    if (freelook->value && !(*input_state->in_strafe_state & 1)) input_state->viewangles[PITCH] += m_pitch->value * y;
    else command->forwardmove -= m_forward->value * y;
    mouse_x = mouse_y = 0;
}
void KBD_Init(Key_Event_fp_t callback) { key_event = callback; }
void KBD_Close(void) {
    for (int key = 1; key < 256; ++key) if (pressed[key]) send_key(key, false);
    key_event = NULL;
}
void KBD_Update(void) {
    SwsEvent event;
    int result;
    while ((result = sws_poll_event(&event)) == 1) {
        window_id = event.window_id;
        if (event.kind == SWS_EVENT_DESTROYED) {
            if (input_state && input_state->Quit_fp) input_state->Quit_fp();
            return;
        }
        if (event.kind == SWS_EVENT_FOCUS) {
            focused = event.value != 0;
            if (!focused) for (int key = 1; key < 256; ++key) if (pressed[key]) send_key(key, false);
            pointer_lock();
        } else if (event.kind == SWS_EVENT_INPUT) {
            if (event.type == EV_KEY) send_key(key_code(event.code), event.value != 0);
            else if (event.type == EV_REL) {
                if (event.code == REL_X) mouse_x += event.value;
                else if (event.code == REL_Y) mouse_y += event.value;
                else if (event.code == REL_WHEEL && event.value) {
                    int key = event.value > 0 ? K_MWHEELUP : K_MWHEELDOWN;
                    send_key(key, true); send_key(key, false);
                }
            }
        }
    }
    if (result < 0) ri.Sys_Error(ERR_FATAL, "SWS input connection failed: %d", result);
}
int Vkimp_SetMode(int *width, int *height, int mode, qboolean fullscreen) {
    SwsDisplay display;
    (void)mode; (void)fullscreen;
    if (sws_get_display(&display) < 0) return rserr_invalid_mode;
    *width = display.width; *height = display.height;
    ri.Vid_NewWindow(*width, *height);
    ri.Con_Printf(PRINT_ALL, "SWS display plane: %dx%d\n", *width, *height);
    return rserr_ok;
}
void Vkimp_GetInstanceExtensions(char **extensions, uint32_t *count) {
    if (extensions) {
        extensions[0] = VK_KHR_SURFACE_EXTENSION_NAME;
        extensions[1] = VK_KHR_DISPLAY_EXTENSION_NAME;
    }
    if (count) *count = 2;
}
VkResult Vkimp_CreateSurface(void) {
    uint32_t count = 0;
    VkResult result = vkEnumeratePhysicalDevices(vk_instance, &count, NULL);
    if (result != VK_SUCCESS || !count) return VK_ERROR_INITIALIZATION_FAILED;
    VkPhysicalDevice *physicals = calloc(count, sizeof(*physicals));
    if (!physicals) return VK_ERROR_OUT_OF_HOST_MEMORY;
    result = vkEnumeratePhysicalDevices(vk_instance, &count, physicals);
    VkPhysicalDevice physical = physicals[0];
    free(physicals);
    if (result != VK_SUCCESS) return result;
    result = vkGetPhysicalDeviceDisplayPropertiesKHR(physical, &count, NULL);
    if (result != VK_SUCCESS || !count) return VK_ERROR_INITIALIZATION_FAILED;
    VkDisplayPropertiesKHR *displays = calloc(count, sizeof(*displays));
    if (!displays) return VK_ERROR_OUT_OF_HOST_MEMORY;
    result = vkGetPhysicalDeviceDisplayPropertiesKHR(physical, &count, displays);
    VkDisplayKHR display = displays[0].display;
    free(displays);
    if (result != VK_SUCCESS) return result;
    result = vkGetDisplayModePropertiesKHR(physical, display, &count, NULL);
    if (result != VK_SUCCESS || !count) return VK_ERROR_INITIALIZATION_FAILED;
    VkDisplayModePropertiesKHR *modes = calloc(count, sizeof(*modes));
    if (!modes) return VK_ERROR_OUT_OF_HOST_MEMORY;
    result = vkGetDisplayModePropertiesKHR(physical, display, &count, modes);
    VkDisplaySurfaceCreateInfoKHR info = {.sType = VK_STRUCTURE_TYPE_DISPLAY_SURFACE_CREATE_INFO_KHR,
        .displayMode = modes[0].displayMode, .planeIndex = 0,
        .transform = VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR,
        .alphaMode = VK_DISPLAY_PLANE_ALPHA_OPAQUE_BIT_KHR,
        .imageExtent = modes[0].parameters.visibleRegion};
    free(modes);
    if (result != VK_SUCCESS) return result;
    return vkCreateDisplayPlaneSurfaceKHR(vk_instance, &info, NULL, &vk_surface);
}
void Vkimp_Shutdown(void) {
    mouse_enabled = false; pointer_lock(); window_id = 0; focused = false;
    if (vkw_state.log_fp) { fclose(vkw_state.log_fp); vkw_state.log_fp = NULL; }
}
int Vkimp_Init(void *instance, void *window) { (void)instance; (void)window; return true; }
void Vkimp_BeginFrame(float separation) { (void)separation; }
void Vkimp_EndFrame(void) {}
void Vkimp_AppActivate(qboolean active) { RW_IN_Activate(active); }
