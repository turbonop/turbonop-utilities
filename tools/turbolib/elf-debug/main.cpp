#include <cstdio>

int use_a(int);
int use_b(int);
int use_c(int);

int main(int argc, char**) {
  int t = use_a(argc) + use_b(argc + 1) + use_c(argc + 2);
  std::printf("%d\n", t);
  return 0;
}
