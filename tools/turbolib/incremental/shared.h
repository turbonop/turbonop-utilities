// What every object of the library shares: inline functions, templates, a
// vftable, a selectany variable, a string literal, and types, each a COMDAT
// (or, under /Z7, type records) that every object using it holds a copy of.
// It is also the precompiled header of pch.cpp and the objects using it.
#pragma once

#include <cstring>

struct Point {
    int x, y;
    Point operator+(const Point& o) const { return {x + o.x, y + o.y}; }
};

inline int shared_add(int a, int b) { return a * 3 + b; }

template <class T> T twice(T v) { return v + v; }

template <class T, int N> struct Ring {
    T items[N] = {};
    int at = 0;
    void push(T v) { items[at++ % N] = v; }
    T sum() const {
        T s{};
        for (int i = 0; i < N; ++i) s = s + items[i];
        return s;
    }
};

struct Shape {
    virtual ~Shape() = default;
    virtual int area() const { return 0; }
};
struct Square : Shape {
    int side;
    explicit Square(int s) : side(s) {}
    int area() const override { return side * side; }
};

__declspec(selectany) int shared_counter = 7;

inline const char* greeting() { return "hello from a shared inline function"; }

inline int text_length() { return static_cast<int>(std::strlen(greeting())); }

inline int switch_on(int v) {
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

// Each object's own function: what it computes from the shared ones.
inline int use_shared(int seed) {
    Ring<Point, 4> r;
    for (int i = 0; i < 6; ++i) r.push(Point{seed + i, twice(i)});
    Point p = r.sum();
    Square sq(seed % 5 + 1);
    const Shape& s = sq;
    return shared_add(p.x, p.y) + s.area() + switch_on(seed % 7) + text_length() +
           twice(shared_counter);
}
