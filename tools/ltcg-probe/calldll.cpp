// Calls the test DLL (dll.cpp) by name: its exports' results, and a C++
// exception thrown across it. calldll <dll>
#include <windows.h>

#include <cstdio>
#include <stdexcept>

int main(int argc, char** argv) {
    if (argc != 2) return 2;
    HMODULE m = LoadLibraryA(argv[1]);
    if (!m) return 3;
    auto sum = reinterpret_cast<int (*)()>(GetProcAddress(m, "dll_sum"));
    auto area = reinterpret_cast<double (*)(double, double)>(GetProcAddress(m, "dll_area"));
    if (!sum || !area) return 4;
    std::printf("sum %d area %g\n", sum(), area(3, 4));
    try {
        area(-1, 1);
        std::printf("no exception\n");
    } catch (const std::invalid_argument& e) {
        std::printf("caught %s\n", e.what());
    }
    return 0;
}
