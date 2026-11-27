#include <stdio.h>

int lib_used(int x);
int lib_also_used(int x);

int main(int argc, char** argv) {
    (void)argv;
    printf("%d\n", lib_used(argc) + lib_also_used(argc));
    return 0;
}
