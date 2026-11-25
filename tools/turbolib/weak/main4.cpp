#include <stdio.h>
extern "C" int shared_f(int);
int main() { printf("shared_f: %d\n", shared_f(1)); }
