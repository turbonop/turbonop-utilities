// The program: prints what each library object computes. MAIN_VERSION
// changes it, to relink incrementally with one object changed. With
// WITH_ZI and WITH_GL, it calls those objects' functions too.
#include <cstdio>

#include "shared.h"

#ifndef MAIN_VERSION
#define MAIN_VERSION 1
#endif

int a_value();
int a_pid_ok();
int b_value();
int c_value();
#ifdef WITH_ZI
int zi_value();
#endif
#ifdef WITH_GL
int gl_value();
#endif

int main() {
    int zi = 0, gl = 0;
#ifdef WITH_ZI
    zi = zi_value();
#endif
#ifdef WITH_GL
    gl = gl_value();
#endif
    std::printf("main %d a %d b %d c %d zi %d gl %d pid %d shared %d\n", MAIN_VERSION,
                a_value(), b_value(), c_value(), zi, gl, a_pid_ok(), use_shared(MAIN_VERSION));
    return 0;
}
