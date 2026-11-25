// A small program for PGO: a hot loop, a cold function, and one never run.
#include <cstdio>
#include <cstdlib>

__declspec(noinline) int hot(int x) {
    int s = 0;
    for (int i = 0; i < x; ++i) {
        s += (i * 7) ^ (s >> 3);
    }
    return s;
}

__declspec(noinline) int cold(int x) {
    return x * 3 + 1;
}

__declspec(noinline) int never(int x) {
    std::printf("never %d\n", x);
    return x - 1;
}

int main(int argc, char** argv) {
    int n = argc > 1 ? std::atoi(argv[1]) : 1000;
    long long total = 0;
    for (int r = 0; r < 2000; ++r) {
        total += hot(n);
    }
    total += cold(argc);
    if (argc > 5) {
        total += never(argc);
    }
    std::printf("%lld\n", total);
    return 0;
}
