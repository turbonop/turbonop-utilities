// deep() is a COMDAT that inlines functions from six files: its inline
// sites change files, so their annotations name file checksum offsets.
#pragma once
#include "inl1.h"
#include "inl2.h"
#include "inl3.h"
#include "inl4.h"
#include "inl5.h"
#include "inl6.h"
__declspec(noinline) inline void deep(int n) {
  step1(n);
  step2(n + 1);
  step3(n + 2);
  step4(n + 3);
  step5(n + 4);
  step6(n + 5);
}
__declspec(noinline) inline int shallow(int n) {
  step6(n);
  return n * 3;
}
