// One of the files deep() inlines from.
#pragma once
void sink(int);
inline void step4(int n) {
  for (int i = 0; i < n; ++i)
    sink(i * 4 + n);
}
