// Inputs for turbonop-coff's tests (see ..\model.cmd): COMDATs and their
// associative sections, statics, uninitialized data, and a selectany
// variable, in as few bytes as the compilers allow.

int counter;                                   // .bss
int table[4] = {1, 2, 3, 4};                   // .data
__declspec(selectany) int chosen = 42;         // a COMDAT of data
static const char text[] = "a literal in .rdata";

template <typename T> T twice(T v) { return v + v; }

struct Shape {
    virtual ~Shape() {}
    virtual int area() const = 0;
};
struct Square : Shape {
    int side;
    explicit Square(int s) : side(s) {}
    int area() const override { return side * side; }
};

int thrower(int n) {
    if (n < 0)
        throw n;
    return n;
}

int catcher(int n) {
    try {
        return thrower(n);
    } catch (int) {
        return -1;
    }
}

extern "C" int entry(int n) {
    Square s(n);
    const Shape& shape = s;
    return twice(shape.area()) + twice(static_cast<long long>(n)) > 0
               ? catcher(n) + text[n & 7] + chosen
               : table[n & 3] + counter;
}
