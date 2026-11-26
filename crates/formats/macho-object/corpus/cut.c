// Built with -DKEEP, atoms the cut_atoms example cuts (_cut_me, a
// function between others, and _cut_var, the first variable, 8 bytes, so
// what follows keeps its alignment): the object is then the one built
// without them. (Not the last function: the
// padding before it, the alignment of the function before it, would stay,
// where LLVM writes none.) The atoms refer to nothing the rest doesn't, so nothing
// else changes; what comes after them moves.

int printf(const char *, ...);

#ifdef KEEP
long long cut_var = 17;
#endif
static int counter = 3;
int other_var = 5;
long long other_wide = 9; // __data's alignment stays 8 without _cut_var.
static const char *names[] = {"alpha", "beta"};

int __attribute__((noinline)) first(int x) {
  return x * 3 + counter++ + (int)other_wide;
}

#ifdef KEEP
int __attribute__((noinline)) cut_me(int x) {
  int s = 0;
  for (int i = 0; i < x; i++)
    s += i * 7 + 1;
  return s;
}
#endif

static int __attribute__((noinline)) helper(int x) { return x + other_var; }

int last(int x) {
  return helper(x) + first(x) + printf("%s %d", names[x & 1], counter);
}

double scale(double x) { return x * 1.0625 - 2.5; }
