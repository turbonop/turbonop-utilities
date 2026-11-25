#include <stdio.h>
extern "C" int a_func(int); extern "C" int shared_f(int);
int main() { printf("a_func: %d\n", a_func(1)); printf("shared_f: %d\n", shared_f(1)); }
