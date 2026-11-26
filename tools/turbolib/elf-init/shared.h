// Inline variables and templates' static data members with dynamic
// initialization, which clang puts in the variable's COMDAT group with its
// `.init_array` entry, and GCC guards from each object's own initializer.
#pragma once

// Prints its argument in order; numbers, not strings, so that no
// initializer names a string literal (a local symbol outside its group,
// which would keep it from folding for that reason alone).
int trace(int id);

struct Logger {
  int id;
  explicit Logger(int n) : id(n) { trace(n); }
  ~Logger();
};

inline Logger::~Logger() { trace(-id); }

inline int helper(int x) { return x * 3 + 1; }

inline int iv = trace(10);
inline Logger il{11};
__attribute__((init_priority(2000))) inline Logger ip{12};
inline thread_local int tl = trace(13);

template <class T> struct S {
  static int m;
};
template <class T> int S<T>::m = trace(20 + int(sizeof(T)));

template <class T> inline int vt = trace(40 + int(sizeof(T)));
