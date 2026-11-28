// Links against libraries of ../loose's a.cpp, b.cpp, and c.cpp and of
// d.cpp and e.cpp, folded once and again (run.py). It prints the same
// lines whichever library it is linked with.
#include <cstdio>
#include <cstring>

int a_run();
int b_run();
int c_run();
int d_run();
int e_run();
const char* a_greeting();
const char* d_greeting();
const char* e_greeting();
const char* d_kind();
int b_variant();
int c_variant();
int e_variant();

int main() {
    std::printf("runs: %d %d %d %d %d\n", a_run(), b_run(), c_run(), d_run(), e_run());
    std::printf("greetings: %s | %s | %s\n", a_greeting(), d_greeting(), e_greeting());
    std::printf("greetings equal: %d\n", std::strcmp(d_greeting(), e_greeting()) == 0);
    std::printf("kind: %s\n", d_kind());
    std::printf("variants: %d %d %d\n", b_variant(), c_variant(), e_variant());
    return 0;
}
