#include <stdio.h>
int use_a(int); int use_b(int);
int main(int argc, char**) { printf("%d\n", use_a(argc) + use_b(argc)); return 0; }
