// One of the files deep() inlines from.
#pragma once
void sink(int);
inline void step3(int n) {
  for (int i = 0; i < n; ++i)
    sink(i * 3 + n);
}
