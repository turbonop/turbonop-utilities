// What phase 4's less conservative folding folds (and what it mustn't):
// included by a.cpp, b.cpp, and c.cpp, each with its own VARIANT.
#pragma once

int ext_one();
int ext_two();

// Internal-linkage things that inline functions reach (--locals=copy).
// Read-only data outside COMDATs: a piece the synthesized member copies.
static const int kTable[] = {2, 3, 5, 7, 11, 13};
// Writable: a copy would be another variable, so what reaches it stays.
static int g_counter;
// Local COMDATs: copied whole with the units that reach them.
static inline int twice(int x) { return 2 * x + (x > 100 ? 1 : 0); }
namespace {
// Its name has the translation unit's anonymous namespace in it, so it
// differs in each object; its code doesn't.
inline int thrice(int x) { return 3 * x - (x < -100 ? 1 : 0); }
}  // namespace

// Under /Od without /GF, its string is a local $SG symbol in .rdata.
inline const char* greeting() { return "hello from an inline function"; }
inline int lookup(int i) { return kTable[(i % 6 + 6) % 6]; }
inline int doubled(int x) { return twice(x) + 1; }
inline int tripled(int x) { return thrice(x) + 2; }
inline int both(int x) { return twice(x) + lookup(x) + tripled(x); }
inline int bump() { return ++g_counter; }
inline const char* pick_name(int i) {
    static const char* const names[] = {"zero", "one", "two"};
    return names[(i % 3 + 3) % 3];
}

// Copies of one name with other code in each translation unit: what
// --fold=select-any folds, and --report odr lists.
#ifndef VARIANT
#define VARIANT 0
#endif
inline int variant() { return VARIANT * 10 + 1; }
__declspec(selectany) int selected = VARIANT;
// Copies that refer to other names: under --pull=preserve, select-any
// leaves them (keeping one could load other members).
inline int refers() {
#if VARIANT == 1
    return ext_one();
#else
    return ext_two();
#endif
}
