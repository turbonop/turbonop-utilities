#include <cstdio>

int trace(int id) {
  std::printf("%d\n", id);
  return id;
}

int ua();
int ub();
int uc();

int main() {
  std::printf("main %d\n", ua() + ub() + uc());
  return 0;
}
