// One of the files deep() inlines from.
#pragma once
void sink(int);
inline void step1(int n) {
  for (int i = 0; i < n; ++i)
    sink(i * 1 + n);
}
