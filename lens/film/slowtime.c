// LD_PRELOAD shim: the process sees time run SLOW times slower (for filming a GPU-starved app in slow time).
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <stdlib.h>
#include <sys/time.h>
#include <time.h>

static int (*real_cg)(clockid_t, struct timespec*);
static int (*real_tw)(pthread_cond_t*, pthread_mutex_t*, const struct timespec*);
static int (*real_cw)(pthread_cond_t*, pthread_mutex_t*, clockid_t, const struct timespec*);
static double K = 1.0, base[16];
static int ready;

static double ts(const struct timespec* t) { return t->tv_sec + t->tv_nsec * 1e-9; }
static void tots(double v, struct timespec* t) { t->tv_sec = (time_t)v; t->tv_nsec = (long)((v - (double)t->tv_sec) * 1e9); }

static void init(void) {
    if (ready) return;
    real_cg = dlsym(RTLD_NEXT, "clock_gettime");
    real_tw = dlsym(RTLD_NEXT, "pthread_cond_timedwait");
    real_cw = dlsym(RTLD_NEXT, "pthread_cond_clockwait");
    const char* e = getenv("SLOW");
    K = e ? atof(e) : 1.0;
    if (K <= 0) K = 1.0;
    // A shared origin so every process with the shim agrees: SLOW_T0 = real CLOCK_MONOTONIC seconds.
    const char* o = getenv("SLOW_T0");
    struct timespec t;
    for (int c = 0; c < 16; c++) {
        if (real_cg(c, &t) == 0) base[c] = ts(&t);
    }
    if (o) {
        double m = atof(o), now = base[CLOCK_MONOTONIC];
        for (int c = 0; c < 16; c++) base[c] -= (now - m);
    }
    ready = 1;
}

static double scaled(clockid_t c, double real) { return base[c] + (real - base[c]) / K; }
static double unscaled(clockid_t c, double v) { return base[c] + (v - base[c]) * K; }

int clock_gettime(clockid_t c, struct timespec* t) {
    init();
    int r = real_cg(c, t);
    if (r == 0 && c >= 0 && c < 16 && c != CLOCK_PROCESS_CPUTIME_ID && c != CLOCK_THREAD_CPUTIME_ID) tots(scaled(c, ts(t)), t);
    return r;
}

int gettimeofday(struct timeval* tv, void* tz) {
    struct timespec t;
    clock_gettime(CLOCK_REALTIME, &t);
    if (tv) { tv->tv_sec = t.tv_sec; tv->tv_usec = t.tv_nsec / 1000; }
    return 0;
}

int pthread_cond_timedwait(pthread_cond_t* c, pthread_mutex_t* m, const struct timespec* a) {
    init();
    struct timespec r;
    tots(unscaled(CLOCK_REALTIME, ts(a)), &r);
    return real_tw(c, m, &r);
}

int pthread_cond_clockwait(pthread_cond_t* c, pthread_mutex_t* m, clockid_t id, const struct timespec* a) {
    init();
    struct timespec r;
    tots(unscaled(id, ts(a)), &r);
    return real_cw(c, m, id, &r);
}
