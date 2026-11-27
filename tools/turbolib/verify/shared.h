// What every object of the fixture library shares: inline functions,
// templates, a vftable, a selectany variable, and a string literal, each a
// COMDAT that every object using it holds a copy of.
#pragma once

#ifdef BROKEN
// The broken library's copy: other bytes under the same name.
inline int shared_add(int a, int b) { return a * 5 - b; }
#else
inline int shared_add(int a, int b) { return a * 3 + b; }
#endif

template <class T> T twice(T v) { return v + v; }

struct Shape {
    virtual int area() const { return 0; }
};
struct Square : Shape {
    int side;
    explicit Square(int s) : side(s) {}
    int area() const override { return side * side; }
};

__declspec(selectany) int shared_counter = 7;

inline const char* greeting() { return "hello from a shared inline function"; }

inline int switch_on(int v) {
    // A jump table (on x64, image-relative addresses in the code section).
    switch (v) {
    case 0: return shared_add(v, 1);
    case 1: return v * 7;
    case 2: return v ^ 0x55;
    case 3: return v - 9;
    case 4: return v + 12;
    case 5: return v << 2;
    default: return -1;
    }
}

int compute();
int a_value(int x);
int b_value(int x);
int c_value(int x);
