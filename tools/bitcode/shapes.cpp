// The bitcode fixtures' C++ source (see build.cmd): classes with virtual
// functions, a template, exceptions (invoke and landingpad on Linux, the
// funclets catchswitch, catchpad, and cleanuppad on Windows), a static
// initializer, and calls through pointers.

struct Shape {
    virtual ~Shape() {}
    virtual double area() const = 0;
    virtual const char *name() const { return "shape"; }
};

struct Square : Shape {
    double side;
    explicit Square(double s) : side(s) {}
    ~Square() override { ++destroyed; }
    double area() const override { return side * side; }
    const char *name() const override { return "square"; }
    static int destroyed;
};

int Square::destroyed = 0;

struct Circle : Shape {
    double radius;
    explicit Circle(double r) : radius(r) {}
    double area() const override { return 3.14159 * radius * radius; }
};

template <typename T> T clamp_to(T v, T lo, T hi) {
    return v < lo ? lo : (v > hi ? hi : v);
}

struct Failure {
    int code;
};

int counter = 0;

__attribute__((noinline)) void may_throw(int x) {
    if (x > 3)
        throw Failure{x};
    ++counter;
}

int guarded(int x) {
    try {
        Square s(x);
        may_throw(x);
        return static_cast<int>(s.area());
    } catch (const Failure &f) {
        return -f.code;
    } catch (...) {
        return -1;
    }
}

static Circle unit(1.0);

double total(Shape **shapes, int n) {
    double t = 0;
    for (int i = 0; i < n; ++i)
        t += shapes[i]->area();
    return t;
}

namespace detail {
int twice(int x) { return x * 2; }
} // namespace detail

int (*const table[])(int) = {guarded, detail::twice};

extern "C" int shapes_main(int argc) {
    Square sq(argc);
    Shape *list[] = {&sq, &unit};
    return clamp_to<int>(static_cast<int>(total(list, 2)), 0, 100) +
           static_cast<int>(clamp_to<long>(argc, 1, 5)) +
           table[argc & 1](argc) + (sq.name()[0] == 's');
}
