// CodeView records that only some builds have: optimized code's locals,
// live ranges and inline sites, /GS's frame cookies, thread-local and
// file-static data, annotations, jump tables, heap allocation sites,
// indirect calls, adjustor thunks, using-directives, and inlined code
// from two files. build.cmd compiles it with MSVC and clang-cl, for x64,
// x86 and ARM64.

#include <intrin.h>
#include <new>

namespace fixture {
int global_value = 1;
}
using namespace fixture;

static int file_static = 3;
thread_local int tls_counter;
__declspec(thread) int tls_plain;

struct Point {
    int x, y;
};

static inline int square(int v) {
    return v * v + file_static;
}

// Inlined code from two files: its inline sites change file.
static inline int from_two_files(int v) {
    int r = v * 3;
#include "kinds-body.inc"
    return r + 1;
}

extern "C" __declspec(noinline) int two_files(int v) {
    return from_two_files(v) + from_two_files(v + 1);
}

extern "C" __declspec(noinline) int sum_of_squares(int n) {
    int s = 0;
    for (int i = 0; i < n; ++i) {
        s += square(i);
    }
    return s + global_value;
}

// A buffer /GS protects with a cookie.
extern "C" __declspec(noinline) int cookie(const char* in, int n) {
    char buf[64];
    int i = 0;
    for (; i < n && i < 63 && in[i]; ++i) {
        buf[i] = in[i];
    }
    buf[i] = 0;
    return buf[n / 2] + i;
}

// A switch with a jump table.
extern "C" __declspec(noinline) int dispatch(int k, int v) {
    switch (k) {
    case 0: return v + 1;
    case 1: return v * 3;
    case 2: return v - 7;
    case 3: return v ^ 0x55;
    case 4: return v << 2;
    case 5: return v >> 1;
    case 6: return -v;
    case 7: return v * v;
    case 8: return v | 9;
    case 9: return v & 0x1F;
    default: return 0;
    }
}

extern "C" __declspec(noinline) Point* make_point(int x) {
    return new Point{x, x + 1};
}

extern "C" __declspec(noinline) void annotate() {
    __annotation(L"turbonop", L"fixture");
}

extern "C" __declspec(noinline) int call_indirect(int (*f)(int), int v) {
    return f(v) + 1;
}

extern "C" __declspec(noinline) int bump() {
    tls_plain += 2;
    return ++tls_counter + tls_plain;
}

// Calling Derived's g through Other's vftable needs an adjustor thunk.
struct Base {
    virtual int f();
    int b = 1;
};
struct Other {
    virtual int g();
    int o = 2;
};
struct Derived : Base, Other {
    int f() override;
    int g() override;
};
int Base::f() { return b; }
int Other::g() { return o; }
int Derived::f() { return b + 10; }
int Derived::g() { return o + 20; }

extern "C" __declspec(noinline) int use_derived(Other* p) {
    return p->g();
}

extern "C" Other* make_derived() {
    return new Derived;
}

// Locals that live in registers and on the stack in turn.
extern "C" __declspec(noinline) long long mix(long long a, long long b, int n) {
    long long acc = a;
    Point p{static_cast<int>(a), static_cast<int>(b)};
    for (int i = 0; i < n; ++i) {
        acc = acc * 31 + b + p.x;
        p.y += static_cast<int>(acc & 7);
        if (acc & 1) {
            p.x ^= i;
        }
    }
    return acc + p.y;
}
