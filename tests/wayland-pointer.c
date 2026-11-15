/* Pointer input for the owned, single-output Wayland QA lab only. */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <limits.h>
#include <linux/input-event-codes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"

static struct zwlr_virtual_pointer_manager_v1 *manager;

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data;
    if (strcmp(interface, zwlr_virtual_pointer_manager_v1_interface.name) == 0) {
        manager = wl_registry_bind(registry, name,
                                   &zwlr_virtual_pointer_manager_v1_interface,
                                   version < 2 ? version : 2);
    }
}

static void global_remove(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data;
    (void)registry;
    (void)name;
}

static bool private_display(const char *lab, const char *display) {
    if (!lab || !display || (strncmp(lab, "/tmp/", 5) != 0 &&
                             strncmp(lab, "/var/tmp/", 9) != 0)) return false;
    struct stat directory, socket;
    if (lstat(lab, &directory) || !S_ISDIR(directory.st_mode) ||
        directory.st_uid != geteuid() || (directory.st_mode & 077) != 0) return false;
    size_t length = strlen(lab);
    if (strncmp(display, lab, length) != 0 || strncmp(display + length, "/r/", 3) != 0)
        return false;
    return lstat(display, &socket) == 0 && S_ISSOCK(socket.st_mode) &&
           socket.st_uid == geteuid();
}

static bool number(const char *text, uint32_t *value) {
    errno = 0;
    char *end;
    unsigned long parsed = strtoul(text, &end, 10);
    if (errno || *text == '\0' || *end != '\0' || text[0] == '-' || parsed > 65535)
        return false;
    *value = (uint32_t)parsed;
    return true;
}

static uint32_t now_ms(void) {
    struct timespec value;
    if (clock_gettime(CLOCK_MONOTONIC, &value)) abort();
    return (uint32_t)((uint64_t)value.tv_sec * 1000 + (uint64_t)value.tv_nsec / 1000000);
}

static void settle(long nanoseconds) {
    struct timespec delay = {.tv_sec = 0, .tv_nsec = nanoseconds};
    while (nanosleep(&delay, &delay) && errno == EINTR) {}
}

int main(int argc, char **argv) {
    uint32_t x, y, width, height, end_x = 0, end_y = 0;
    if ((argc != 5 && argc != 6 && argc != 8) || !number(argv[1], &x) || !number(argv[2], &y) ||
        !number(argv[3], &width) || !number(argv[4], &height) ||
        !width || !height || x >= width || y >= height ||
        (argc >= 6 && strcmp(argv[5], "left") && strcmp(argv[5], "right")) ||
        (argc == 8 && (!number(argv[6], &end_x) || !number(argv[7], &end_y) ||
                       end_x >= width || end_y >= height))) {
        fputs("usage: wayland-pointer X Y WIDTH HEIGHT [left|right [END_X END_Y]]\n", stderr);
        return 2;
    }
    const uint32_t button = argc >= 6 && strcmp(argv[5], "right") == 0 ? BTN_RIGHT : BTN_LEFT;
    const char *display_name = getenv("WAYLAND_DISPLAY");
    if (!private_display(getenv("GUST_OPTIMIZATION_LAB"), display_name)) {
        fputs("refused: pointer requires the owned private QA socket\n", stderr);
        return 2;
    }
    struct wl_display *display = wl_display_connect(display_name);
    if (!display) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    const struct wl_registry_listener listener = {.global = global, .global_remove = global_remove};
    wl_registry_add_listener(registry, &listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !manager) {
        fputs("virtual pointer extension unavailable\n", stderr);
        wl_display_disconnect(display);
        return 77;
    }
    struct zwlr_virtual_pointer_v1 *pointer =
        zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, NULL);
    zwlr_virtual_pointer_v1_motion_absolute(pointer, now_ms(), x, y, width, height);
    zwlr_virtual_pointer_v1_frame(pointer);
    if (wl_display_roundtrip(display) < 0) return 1;
    /* Let Qt receive pointer entry before the press; this is not a timing probe. */
    settle(75000000);
    zwlr_virtual_pointer_v1_button(pointer, now_ms(), button, WL_POINTER_BUTTON_STATE_PRESSED);
    zwlr_virtual_pointer_v1_frame(pointer);
    if (wl_display_roundtrip(display) < 0) return 1;
    settle(20000000);
    if (argc == 8) {
        for (int64_t step = 1; step <= 8; ++step) {
            const uint32_t move_x = (uint32_t)((int64_t)x + ((int64_t)end_x - x) * step / 8);
            const uint32_t move_y = (uint32_t)((int64_t)y + ((int64_t)end_y - y) * step / 8);
            zwlr_virtual_pointer_v1_motion_absolute(pointer, now_ms(), move_x, move_y, width, height);
            zwlr_virtual_pointer_v1_frame(pointer);
            if (wl_display_roundtrip(display) < 0) return 1;
            settle(12500000);
        }
    }
    zwlr_virtual_pointer_v1_button(pointer, now_ms(), button, WL_POINTER_BUTTON_STATE_RELEASED);
    zwlr_virtual_pointer_v1_frame(pointer);
    int result = wl_display_roundtrip(display) < 0 ? 1 : 0;
    zwlr_virtual_pointer_v1_destroy(pointer);
    zwlr_virtual_pointer_manager_v1_destroy(manager);
    wl_registry_destroy(registry);
    wl_display_disconnect(display);
    return result;
}
