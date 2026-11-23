#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef void (*ScarletLog)(const char *line);
typedef struct {
    uint32_t width;
    uint32_t height;
    bool tablet;
    void (*damage)(void);
} ScarletDisplayConfiguration;

typedef struct {
    uint16_t row;
    uint16_t column;
    bool cursorVisible;
    bool applicationCursor;
    bool bracketedPaste;
} ScarletTerminalState;
typedef void (*ScarletScreen)(const char *text, ScarletTerminalState state);

#define SCARLET_IOS_TOUCH_SLOTS 16
#define SCARLET_IOS_TOUCH_MAX 65535
typedef struct {
    int32_t id;
    int32_t x;
    int32_t y;
} ScarletTouchContact;

void scarlet_ios_touch(const ScarletTouchContact *contacts, size_t count, bool cancelled,
                       uint64_t sample_age_ns);
void scarlet_ios_desktop_tab(void);
typedef struct {
    uint32_t width;
    uint32_t height;
    uint64_t generation;
} ScarletFrameInfo;

void scarlet_ios_set_display_size(uint32_t width, uint32_t height);
bool scarlet_ios_open_metal(void *layer, uint32_t width, uint32_t height);
void scarlet_ios_metal_viewport(uint32_t width, uint32_t height);
void scarlet_ios_metal_set_active(bool active);
ScarletFrameInfo scarlet_ios_frame_info(void);
bool scarlet_ios_copy_frame(void *buffer, size_t length, uint64_t generation);
bool scarlet_ios_console_send(const void *bytes, size_t length);
void scarlet_ios_boot(const char *bundle, const char *root, bool bootCheck, bool login, bool shell,
                      ScarletDisplayConfiguration display, ScarletLog log, ScarletScreen screen);
void scarlet_ios_run(const char *image, const char *elf, ScarletLog log);
