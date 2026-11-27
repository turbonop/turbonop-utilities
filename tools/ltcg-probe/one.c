// One translation unit, compiled with /GL: the smallest LTCG link.
#include <stdio.h>

static int square(int x) { return x * x; }

int main(int argc, char** argv) {
    (void)argv;
    printf("%d\n", square(argc + 1));
    return 0;
}
