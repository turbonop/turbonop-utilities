// An input for Hexcavator's LTCG IL tests (see build.cmd): one of each
// kind of record `.cil$gl` holds, and locals, initializers, and functions
// nothing refers to. No headers, to keep the object small.
#pragma comment(lib, "advapi32.lib")

extern "C" int puts(const char*);

namespace geometry {
struct Shape {
    virtual ~Shape() {}
    virtual int area() const = 0;
};
struct Square : Shape {
    int side;
    explicit Square(int s) : side(s) {}
    int area() const override { return side * side; }
};
}  // namespace geometry

// Variables: defined, static, declared, constant, aligned, in a section.
int counter = 5;
static int hidden[40] = {1, 2, 3};
extern int elsewhere;
const int limit = 70000;
__declspec(align(32)) int aligned_value = 7;
#pragma section(".mine", read, write)
__declspec(allocate(".mine")) int in_section = 9;
__declspec(selectany) int chosen = 11;
const char* const greeting = "hello";
int* pointer_to = &counter;
long long big_value = 0x123456789ab;
double ratio = 1.5;
struct Pair { char c; int i; long long l; } pair = {1, 2, 3};

// Functions: inline and referred to, inline and not, static and not,
// a template explicitly instantiated, attributes.
inline int used_inline(int v) { return v * 3; }
inline int unused_inline(int v) { return v * 5; }
static int unused_static(int v) { return v * 7; }
static int used_static(int v) { return hidden[v & 31] += v; }
template <class T> T twice(T v) { return v + v; }
template long twice<long>(long);
[[nodiscard]] int checked(int v);
[[deprecated("use checked")]] int old_way(int v) { return v - 1; }
[[noreturn]] void fail(const char* why);

struct Global {
    int v;
    Global();
};
Global global;  // a dynamic initializer

int locals(int a, int b) {
    static int calls = 0;
    int sum = a + b;
    if (sum > 10) {
        int inner = sum * 2;
        sum += inner;
    }
    if (b < 0) goto done;
    sum += checked(b);
done:
    return sum + ++calls;
}

extern "C" __declspec(dllexport) int exported(int n) {
    geometry::Square s(n);
    const geometry::Shape& shape = s;
    if (n < 0) fail("negative");
    puts(greeting);
    return twice(shape.area()) + used_inline(n) + used_static(n) + counter + elsewhere + limit +
           aligned_value + in_section + chosen + *pointer_to + (int)big_value + (int)ratio + pair.i +
           old_way(n) + locals(n, 2) + global.v;
}
