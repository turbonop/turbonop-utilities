#pragma once
struct Point { int x, y; int sum() const { return x + y; } };
template <class T> struct Box { T value; T get() const { return value; } };
inline int twice(int v) { return v * 2; }
