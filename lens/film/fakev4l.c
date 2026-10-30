// LD_PRELOAD: pretend a v4l2loopback device labelled "Lens" exists (frames go to /dev/null).
#define _GNU_SOURCE
#include <dirent.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>

static const char* FAKE = "/tmp/claude-0/lens/fakesys";
static int fakefd = -1;

DIR* opendir(const char* p) {
    static DIR* (*r)(const char*);
    if (!r) r = dlsym(RTLD_NEXT, "opendir");
    if (p && strcmp(p, "/sys/class/video4linux") == 0) return r(FAKE);
    return r(p);
}

FILE* fopen(const char* p, const char* m) {
    static FILE* (*r)(const char*, const char*);
    if (!r) r = dlsym(RTLD_NEXT, "fopen");
    if (p && strncmp(p, "/sys/class/video4linux/", 23) == 0) {
        char b[512];
        snprintf(b, sizeof b, "%s/%s", FAKE, p + 23);
        return r(b, m);
    }
    return r(p, m);
}

static int fake_open(const char* p, int (*r)(const char*, int, ...)) {
    int fd = r("/dev/null", O_WRONLY);
    fakefd = fd;
    return fd;
}

int open(const char* p, int f, ...) {
    static int (*r)(const char*, int, ...);
    if (!r) r = dlsym(RTLD_NEXT, "open");
    va_list a; va_start(a, f); int mode = va_arg(a, int); va_end(a);
    if (p && strncmp(p, "/dev/video", 10) == 0) return fake_open(p, r);
    return r(p, f, mode);
}

int open64(const char* p, int f, ...) {
    static int (*r)(const char*, int, ...);
    if (!r) r = dlsym(RTLD_NEXT, "open64");
    va_list a; va_start(a, f); int mode = va_arg(a, int); va_end(a);
    if (p && strncmp(p, "/dev/video", 10) == 0) return fake_open(p, r);
    return r(p, f, mode);
}

int ioctl(int fd, unsigned long req, ...) {
    static int (*r)(int, unsigned long, ...);
    if (!r) r = dlsym(RTLD_NEXT, "ioctl");
    va_list a; va_start(a, req); void* arg = va_arg(a, void*); va_end(a);
    if (fd >= 0 && fd == fakefd) return 0;
    return r(fd, req, arg);
}
