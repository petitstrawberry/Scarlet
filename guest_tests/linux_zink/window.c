/* A real SDL2 Wayland/Kopper window with renderer, pixel and swap gates.
 * Run only in the isolated Scarlet test guest. No software renderer accepted.
 * Escape closes the window; arrow keys change its clear colour and are logged.
 */
#include <SDL.h>
#include <SDL_opengl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    unsigned int duration = argc > 1 ? (unsigned int)atoi(argv[1]) : 10;
    if (SDL_Init(SDL_INIT_VIDEO) != 0) {
        fprintf(stderr, "FAIL video: %s\n", SDL_GetError());
        return 1;
    }
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MAJOR_VERSION, 1);
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MINOR_VERSION, 5);
    SDL_GL_SetAttribute(SDL_GL_DOUBLEBUFFER, 1);
    SDL_GL_SetAttribute(SDL_GL_RED_SIZE, 8);
    SDL_GL_SetAttribute(SDL_GL_GREEN_SIZE, 8);
    SDL_GL_SetAttribute(SDL_GL_BLUE_SIZE, 8);
    SDL_GL_SetAttribute(SDL_GL_DEPTH_SIZE, 0);
    SDL_Window *window = SDL_CreateWindow("Scarlet Zink / SGFX window probe",
        SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED, 640, 400,
        SDL_WINDOW_OPENGL | SDL_WINDOW_RESIZABLE | SDL_WINDOW_SHOWN);
    if (!window) {
        fprintf(stderr, "FAIL window: %s\n", SDL_GetError());
        return 2;
    }
    puts("PASS SDL OpenGL window");
    SDL_GLContext context = SDL_GL_CreateContext(window);
    if (!context) {
        fprintf(stderr, "FAIL context: %s\n", SDL_GetError());
        return 3;
    }
    GLenum context_error = glGetError();
    if (context_error != GL_NO_ERROR) {
        fprintf(stderr, "FAIL fresh SDL GL context error=0x%x\n", context_error);
        return 6;
    }
    puts("PASS fresh SDL GL context has no GL error");
    const char *renderer = (const char *)glGetString(GL_RENDERER);
    printf("SDL backend: %s\nGL vendor: %s\nGL renderer: %s\nGL version: %s\n",
        SDL_GetCurrentVideoDriver(), glGetString(GL_VENDOR), renderer,
        glGetString(GL_VERSION));
    if (!renderer || !strstr(renderer, "zink") || !strstr(renderer, "SGFX") ||
        strstr(renderer, "llvmpipe") || strstr(renderer, "softpipe")) {
        fprintf(stderr, "FAIL actual renderer is not Zink on SGFX\n");
        return 4;
    }
    SDL_GL_SetSwapInterval(1);
    unsigned int start = SDL_GetTicks(), frames = 0, input = 0;
    int quit = 0;
    float green = 0.4f;
    while (!quit && SDL_GetTicks() - start < duration * 1000) {
        SDL_Event event;
        while (SDL_PollEvent(&event)) {
            if (event.type == SDL_QUIT) quit = 1;
            if (event.type == SDL_KEYDOWN) {
                printf("INPUT key=%s\n", SDL_GetKeyName(event.key.keysym.sym));
                input++;
                green = green > 0.5f ? 0.4f : 0.75f;
                if (event.key.keysym.sym == SDLK_ESCAPE) quit = 1;
            }
            if (event.type == SDL_MOUSEBUTTONDOWN) {
                printf("INPUT mouse=%u x=%d y=%d\n", event.button.button, event.button.x, event.button.y);
                input++;
            }
        }
        int width, height;
        SDL_GL_GetDrawableSize(window, &width, &height);
        glViewport(0, 0, width, height);
        glDisable(GL_SCISSOR_TEST);
        glClearColor(0.1f, green, 0.8f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);
        if (frames == 0) {
            unsigned char pixel[4] = {0};
            size_t bytes = (size_t)width * (size_t)height * 4;
            unsigned char *frame = malloc(bytes);
            if (!frame) {
                fprintf(stderr, "FAIL readback allocation (%zu bytes)\n", bytes);
                return 5;
            }
            /* Exercise complete-image GPU readback independently of the
             * driver's separate partial-image transfer implementation. */
            glReadPixels(0, 0, width, height, GL_RGBA, GL_UNSIGNED_BYTE, frame);
            GLenum read_error = glGetError();
            memcpy(pixel, frame + ((size_t)(height / 2) * width + width / 2) * 4, 4);
            free(frame);
            printf("PIXEL RGBA=%u,%u,%u,%u error=0x%x full_frame=%dx%d\n", pixel[0], pixel[1], pixel[2], pixel[3], read_error, width, height);
            if (read_error != GL_NO_ERROR || abs((int)pixel[0]-26) > 2 || abs((int)pixel[1]-102) > 2 ||
                abs((int)pixel[2]-204) > 2) {
                fprintf(stderr, "FAIL clear readback\n");
                return 5;
            }
            puts("PASS clear readback");
        }
        SDL_GL_SwapWindow(window);
        if (frames++ == 0) puts("PASS first Wayland Vulkan WSI swap returned");
        SDL_Delay(16);
    }
    printf("PASS frames=%u input_events=%u\n", frames, input);
    SDL_GL_DeleteContext(context);
    SDL_DestroyWindow(window);
    SDL_Quit();
    return 0;
}
