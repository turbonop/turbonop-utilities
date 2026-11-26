// Inline functions, templates, and classes that every member of the
// fixture library uses, so that each holds a copy (a COMDAT group) that
// turbolib folds.
#pragma once

#include <cstddef>
#include <stdexcept>
#include <vector>

namespace fx {

struct Point {
  int x, y;
  int dot(const Point& o) const { return x * o.x + y * o.y; }
};

template <class T>
T accumulate(const std::vector<T>& v, T start) {
  T total = start;
  for (std::size_t i = 0; i < v.size(); ++i) {
    T item = v[i];
    if (item > T(0)) {
      T scaled = item * T(3);
      total += scaled;
    } else {
      total -= item;
    }
  }
  return total;
}

inline int counter() {
  static int calls = 0;
  return ++calls;
}

inline thread_local int tls_depth = 0;

inline int with_lambda(int k) {
  auto add = [k](int v) { return v + k * 2; };
  int r = 0;
  for (int i = 0; i < 4; ++i) r = add(r);
  return r;
}

struct Shape {
  virtual ~Shape() = default;
  virtual int area() const { return 0; }
};

struct Rect : Shape {
  int w, h;
  Rect(int w_, int h_) : w(w_), h(h_) {}
  int area() const override { return w * h; }
};

inline int checked(int v) {
  if (v < 0) throw std::runtime_error("negative");
  ++tls_depth;
  return v + tls_depth;
}

inline long mix(long a, long b) {
  long r = accumulate(std::vector<long>{a, b, a - b}, 1L);
  return r ^ (a << 3);
}

}  // namespace fx
