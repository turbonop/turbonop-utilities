// SPDX-License-Identifier: Apache-2.0
// The vtables test program (see testdata/vtables/README.md): classes with
// RTTI, one with two tables (two bases), and lambdas held by
// std::function, whose tables (std::_Func_impl_no_alloc<lambda>'s) have
// no public symbol: in the stripped PDB, only their RTTI names them.

#include <cstdio>
#include <functional>

struct Shape {
    virtual ~Shape() = default;
    virtual int area() const = 0;
    virtual const char* name() const { return "shape"; }
};

struct Named {
    virtual ~Named() = default;
    virtual const char* label() const { return "named"; }
};

struct Square : Shape {
    int side;
    explicit Square(int s) : side(s) {}
    int area() const override { return side * side; }
    const char* name() const override { return "square"; }
};

// Two bases: two tables, the second `{for `Named'}`.
struct Badge : Shape, Named {
    int area() const override { return 1; }
    const char* label() const override { return "badge"; }
};

__declspec(noinline) int run(const std::function<int(int)>& f, int x) { return f(x); }

int main(int argc, char**) {
    Square sq(argc + 1);
    Badge b;
    Shape* shapes[] = {&sq, &b};
    int total = 0;
    for (Shape* s : shapes) {
        total += s->area();
        std::printf("%s\n", s->name());
    }
    Named* n = &b;
    std::printf("%s\n", n->label());
    // Lambdas with captures (so std::function holds each in a
    // _Func_impl_no_alloc of its own, with its own table).
    int k = argc;
    total += run([k](int x) { return x + k; }, 1);
    total += run([k, &total](int x) { return x * k + total; }, 2);
    total += run([&sq](int x) { return x - sq.side; }, 3);
    return total & 0xFF;
}
