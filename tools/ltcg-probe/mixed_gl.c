// Mixed: this object is /GL, mixed_native.c isn't; each calls the other.
#include <stdio.h>

int native_twice(int x);

int gl_plus_one(int x) { return x + 1; }

int main(int argc, char** argv) {
    (void)argv;
    printf("%d\n", native_twice(gl_plus_one(argc)));
    return 0;
}
