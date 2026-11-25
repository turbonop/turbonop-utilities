// The precompiled header of the PCH fixtures: types its users share.
#pragma once

struct Shared {
    int a;
    double b;
    Shared* next;
};

enum class Color { red, green, blue };

inline int shared_twice(int v) {
    return v * 2;
}
