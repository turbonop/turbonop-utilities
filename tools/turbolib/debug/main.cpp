// The program the fixture's libraries are linked into.
#include <cstdio>
int use_a(int);
int use_b(int);
void use_c(int);
void use_d(int);
#ifndef NO_PCH
int use_u1(int);
int use_u2(int);
#endif
static long long total;
void sink(int v) { total = total * 31 + v; }
int main() {
  long long r = use_a(3) + use_b(4);
  use_c(5);
  use_d(6);
#ifndef NO_PCH
  r += use_u1(7) + use_u2(8);
#endif
  std::printf("%lld %lld\n", r, total);
  return 0;
}
