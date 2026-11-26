// Links against a library of a.cpp, b.cpp, and c.cpp. The lines it prints
// must be the same with the original library and the folded one, but for
// the "choice:" line: which copy of variant(), selected, and refers() a
// link keeps is the linker's choice (or select-any's), each a possible
// one.
#include <cstdio>
#include <cstring>

int a_run();
int b_run();
int c_run();
const char* a_greeting();
const char* b_greeting();
const char* c_greeting();
const char* a_name(int);
const char* b_name(int);
int a_variant();
int b_variant();
int c_variant();

int main() {
    std::printf("runs: %d %d %d\n", a_run(), b_run(), c_run());
    std::printf("greetings: %s | %s | %s\n", a_greeting(), b_greeting(), c_greeting());
    std::printf("greetings equal: %d %d\n", std::strcmp(a_greeting(), b_greeting()) == 0,
                std::strcmp(b_greeting(), c_greeting()) == 0);
    std::printf("names: %s %s %s %s\n", a_name(0), a_name(4), b_name(0), b_name(-1));
    std::printf("choice: %d %d %d\n", a_variant(), b_variant(), c_variant());
    return 0;
}
