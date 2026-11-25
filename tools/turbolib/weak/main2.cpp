#include <stdio.h>
extern "C" int shared_f(int); extern "C" int b_func(int);
int main() { printf("shared_f: %d\n", shared_f(1)); printf("b_func: %d\n", b_func(1)); }
