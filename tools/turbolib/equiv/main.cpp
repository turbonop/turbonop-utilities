// The equivalence fixture's program: calls each member and prints what
// they compute (the same in every link).
#include <stdio.h>

// 64 KiB in .text, so that no RVA of the data after it reads as text (two
// printable characters and two NULs: below 0x7F7F), which compare-images
// doesn't read as an RVA when it names data by its content.
#pragma section(".zpad", read, execute)
#pragma comment(linker, "/MERGE:.zpad=.text")
extern "C" __declspec(allocate(".zpad")) const unsigned char padding[0x10000] = {0xCC};

int a_value(int x);
int b_value(int x);
int c_value(int x);

int main() {
    int total = 0;
    for (int i = 0; i < 12; ++i)
        total += a_value(i) * 3 + b_value(i + 1) * 5 + c_value(i + 2) * 7;
    printf("%d\n", total);
    return 0;
}
