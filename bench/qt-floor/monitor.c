/* Isolated labwc map observer; ext-foreign-toplevel-list supplies monotonic time. */
#include <wayland-client.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "toplevel.h"
struct window { char *app; int seen; };
static void closed(void *d, struct ext_foreign_toplevel_handle_v1 *h) {
    struct window *w = d; free(w->app); free(w);
    ext_foreign_toplevel_handle_v1_destroy(h);
}
static void done(void *d, struct ext_foreign_toplevel_handle_v1 *h) {
    (void)h; struct window *w = d;
    if (!w->seen && w->app) {
        struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t);
        printf("MAP %.9f %s\n", t.tv_sec + t.tv_nsec / 1e9, w->app);
        fflush(stdout); w->seen = 1;
    }
}
static void title(void *d, struct ext_foreign_toplevel_handle_v1 *h, const char *s) {(void)d;(void)h;(void)s;}
static void app_id(void *d, struct ext_foreign_toplevel_handle_v1 *h, const char *s) {
    (void)h; struct window *w = d; free(w->app); w->app = strdup(s);
}
static const struct ext_foreign_toplevel_handle_v1_listener hl = {closed, done, title, app_id, title};
static void top(void *d, struct ext_foreign_toplevel_list_v1 *l, struct ext_foreign_toplevel_handle_v1 *h) {
    (void)d;(void)l;
    struct window *w = calloc(1, sizeof(*w));
    ext_foreign_toplevel_handle_v1_add_listener(h, &hl, w);
}
static void finish(void *d, struct ext_foreign_toplevel_list_v1 *l) {(void)d;(void)l;}
static const struct ext_foreign_toplevel_list_v1_listener ll = {top, finish};
static int bound;
static void global(void *d, struct wl_registry *r, uint32_t name, const char *iface, uint32_t v) {
    (void)d;(void)v;
    if (!strcmp(iface, "ext_foreign_toplevel_list_v1")) {
        struct ext_foreign_toplevel_list_v1 *l = wl_registry_bind(r, name, &ext_foreign_toplevel_list_v1_interface, 1);
        ext_foreign_toplevel_list_v1_add_listener(l, &ll, NULL); bound = 1;
    }
}
static void removed(void *d, struct wl_registry *r, uint32_t name) {(void)d;(void)r;(void)name;}
static const struct wl_registry_listener rl = {global, removed};
int main(void) {
    struct wl_display *d = wl_display_connect(NULL); if (!d) return 2;
    struct wl_registry *r = wl_display_get_registry(d);
    wl_registry_add_listener(r, &rl, NULL); wl_display_roundtrip(d);
    if (!bound) return 3;
    puts("READY"); fflush(stdout);
    while (wl_display_dispatch(d) >= 0) {}
    return 0;
}
