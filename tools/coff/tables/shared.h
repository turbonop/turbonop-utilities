// Inline functions and templates that both translation units use, so that
// each object has a copy of every one, with the side-table entries each
// option gives them. No headers beyond <setjmp.h>, to keep the objects small.
#pragma once
#include <setjmp.h>

// Each is kept out of line, so that each object has its COMDAT.
#define SHARED __declspec(noinline) inline

extern "C" __declspec(dllimport) unsigned long __stdcall GetTickCount(void);

typedef int (*binary)(int, int);
typedef int (*unary)(int);

SHARED int add(int a, int b) { return a + b; }
template <class T> __declspec(noinline) T twice(T x) { return x + x; }

// /guard:cf: the address of an inline function and of a template, taken by
// inline code (.gfids$y).
SHARED binary add_address() { return &add; }
SHARED unary twice_address() { return &twice<int>; }

// /guard:cf: an import's address, taken (.giats$y).
SHARED void *tick_address() { return (void *)&GetTickCount; }

// /guard:cf: a longjmp target (.gljmp$y). The buffer is an inline
// function's static, so it is shared, as the function is.
SHARED jmp_buf &buffer() {
    static jmp_buf b;
    return b;
}
SHARED int jumps(int x) {
    if (setjmp(buffer()) == 0) {
        if (x)
            longjmp(buffer(), 1);
        return 0;
    }
    return 1;
}

// /guard:ehcont: a catch's continuation (.gehcont$y); x86: the C++
// handler (.sxdata).
struct oops {
    int value;
};
SHARED int throws(int x) {
    try {
        if (x)
            throw oops{x};
    } catch (const oops &o) {
        return o.value + 1;
    }
    return 0;
}

// x86: an SEH handler (.sxdata), and FPO data (.debug$F).
SHARED int faults(int *p) {
    __try {
        return *p;
    } __except (1) {
        return -1;
    }
}

// /guard:cf: a vftable's functions (.gfids$y, associated with the vftable).
struct shape {
    virtual int area() const = 0;
    virtual ~shape() {}
};
struct square : shape {
    int side;
    explicit square(int s) : side(s) {}
    int area() const override { return side * side; }
};
SHARED int area_of(int s) {
    square q(s);
    const shape &r = q;
    return r.area();
}

// /volatileMetadata: volatile accesses (.voltbl).
SHARED int touch(volatile int *p) {
    *p = 1;
    return *p + 1;
}

// /d2guardretpoline: an indirect call and a switch's jump (.retplne).
SHARED int call(unary f, int x) { return f(x) + 2; }
SHARED int pick(int x) {
    switch (x) {
    case 0: return 4;
    case 1: return 7;
    case 2: return 9;
    case 3: return 11;
    case 4: return 13;
    case 5: return 2;
    default: return 0;
    }
}
