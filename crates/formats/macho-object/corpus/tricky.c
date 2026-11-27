// Features of C objects (and, built for i386 and armv7, of 32-bit ones:
// scattered relocations, their pairs): common symbols (-fcommon), a
// custom section, zero-fill data, PIC references to data and strings,
// a jump table, and data pointing at code and strings.

int printf(const char *, ...);

// Built with -DPAD, a section of 100 bytes after __text moves every
// section after it: the remove_section example removes it and checks the
// object is then the one built without it.
#ifdef PAD
__asm__(".section __TEXT,__pad\n.space 100\n.text\n");
#endif

int common_sym;
int common_array[64];
__attribute__((section("__DATA,__custom"))) int custom = 1;
static int big_zero[4096];
static const char *names[] = {"alpha", "beta", "gamma", "delta"};
static int counter = 3;
int (*callback)(const char *, ...) = printf;

int pick(int i) {
  switch (i) {
  case 0: return printf("%s", names[0]);
  case 1: return printf("%s %d", names[1], counter);
  case 2: return printf("%s %d %d", names[2], counter, i);
  case 3: return printf("x%s", names[3]);
  case 4: return big_zero[i]++;
  case 5: return common_array[i] + custom;
  case 6: return common_sym++;
  }
  return counter++;
}

double scale(double x) { return x * 1.0625 - 2.5; }
