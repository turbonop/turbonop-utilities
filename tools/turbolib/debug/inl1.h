// One of the files deep() inlines from. Part of step1's body is in
// step1.inc: an inline site of it changes files in its annotations.
#pragma once
void sink(int);
inline void step1(int n) {
  for (int i = 0; i < n; ++i)
    sink(i * 1 + n);
#include "step1.inc"
}
