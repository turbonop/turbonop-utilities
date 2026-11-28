// The classes of turbolib's --debug-types tests (see build.sh).
struct Point {
  int x, y;
  int sum() const { return x + y; }
};
// Only the inline functions name it: no giver's own code does.
struct Hidden {
  int h;
};
inline int twice(const Point &p) { return p.sum() * 2; }
inline int peek(const Hidden *h) { return h->h; }
