// Shared by d.cpp and e.cpp (not by ../loose's a, b, and c): a template,
// a class with virtual functions (its vftable a COMDAT), and an inline
// function with a static local, so that a library of d and e folds too.
// No destructor and no function-local static object: either makes the
// functions need C++ EH tables (FH4's), whose image-relative words
// compare-images compares as bytes, not by what they name, so that any two
// links of them differ.
#pragma once

template <typename T>
inline T scaled(T x, int k) {
    return x * static_cast<T>(k) + static_cast<T>(k % 3);
}

struct Shape {
    virtual int area() const = 0;
    virtual const char* kind() const { return "shape"; }
};

struct Square : Shape {
    explicit Square(int s) : side(s) {}
    int area() const override { return side * side; }
    const char* kind() const override { return "square"; }
    int side;
};

inline int counted() {
    static int calls = 0;
    return ++calls;
}
