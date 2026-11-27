// Links against the library, original or folded, and runs its code.
#include <stdio.h>

int one(int);
int two(int);
int exercise(int);

int main() {
    int a = one(3), b = two(4), c = exercise(5);
    printf("one %d two %d exercise %d\n", a, b, c);
    return a + b + c > 0 ? 0 : 1;
}
