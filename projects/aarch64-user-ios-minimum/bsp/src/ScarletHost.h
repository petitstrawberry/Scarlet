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
typedef struct {
    int32_t id;
    int32_t x;
    int32_t y;
} ScarletTouchContact;

void scarlet_ios_touch(const ScarletTouchContact *contacts, size_t count, bool cancelled);
void scarlet_ios_desktop_tab(void);
bool scarlet_ios_copy_frame(void *buffer, size_t length);
bool scarlet_ios_console_send(const void *bytes, size_t length);
void scarlet_ios_boot(const char *bundle, const char *root, bool bootCheck, bool login, bool shell,
                      ScarletDisplayConfiguration display, ScarletLog log, ScarletScreen screen);
void scarlet_ios_run(const char *image, const char *elf, ScarletLog log);
