// An input for Hexcavator's LTCG (/GL) object tests (see build.cmd): a bit
// of everything the IL's symbol table records.
#include <stdio.h>
#pragma comment(lib, "advapi32.lib")
#pragma comment(linker, "/INCLUDE:ltcg_kept")
#pragma detect_mismatch("ltcg_mode", "test")

int counter;                                   // defined, writable
const char banner[] = "an LTCG object";        // defined, read-only
static int hidden[40];                         // static
extern int elsewhere;                          // declared
__declspec(dllexport) int exported_value = 3;  // dllexport
__declspec(dllimport) int imported(int);       // dllimport
extern "C" int ltcg_kept = 1;

struct Shape {
    virtual ~Shape() {}
    virtual int area() const = 0;
};
struct Square : Shape {
    int side;
    explicit Square(int s) : side(s) {}
    int area() const override { return side * side; }
};

template <typename T> T twice(T v) { return v + v; }
static int helper(int n) { return hidden[n & 31] += n; }
[[noreturn]] void fail(const char* why);
int __stdcall callee_pops(int a, int b) { return a - b; }

extern "C" __declspec(dllexport) int exported(int n) {
    Square s(n);
    const Shape& shape = s;
    if (n < 0) fail("negative");
    printf("%s %d\n", banner, elsewhere);
    return twice(shape.area()) + helper(n) + imported(n) + counter + callee_pops(n, 1);
}
