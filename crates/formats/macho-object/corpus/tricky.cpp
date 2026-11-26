// Features of C++ objects the model must keep: exceptions (__eh_frame,
// __gcc_except_tab, personality and typeinfo references), a global
// constructor (__mod_init_func), thread-local variables with and without
// initializers (__thread_vars, __thread_data, __thread_bss, TLV
// relocations), weak (coalesced) definitions, a weak import, a hidden
// definition, zero-fill data, literals, a jump table, and pointers to
// strings and to a local function. No headers: there is no SDK.

extern "C" int printf(const char *, ...);

#ifdef PAD
// With -DPAD, a section that moves the ones after it (as in tricky.c).
__asm__(".section __TEXT,__pad\n.space 100\n.text\n");
#endif

struct Counter {
  int n;
  Counter();
  ~Counter();
};
Counter::Counter() : n(printf("ctor\n")) {}
Counter::~Counter() { printf("dtor %d\n", n); }

Counter global_counter;
thread_local Counter tls_counter;
thread_local int tls_plain = 7;
thread_local int tls_zero;
static int zeroes[1024];

double literal(double x) { return x * 3.25 + 1.5; }

inline int weak_inline(int x) { return x * 2 + 1; }
int use_inline(int x) { return weak_inline(x) + zeroes[x & 1023]++; }

int may_throw(int x) {
  if (x > 3)
    throw x;
  return x;
}
int catcher(int x) {
  try {
    return may_throw(x);
  } catch (int e) {
    return -e;
  }
}

int jump(int x) {
  switch (x) {
  case 0: return printf("a");
  case 1: return printf("bb");
  case 2: return printf("ccc");
  case 3: return printf("dddd");
  case 4: return printf("eeeee");
  case 5: return printf("ffffff");
  case 6: return printf("ggggggg");
  case 7: return printf("hhhhhhhh");
  }
  return 0;
}

int touch_tls() { return tls_counter.n + tls_plain + tls_zero++; }

template <class T> T tmax(T a, T b) { return a < b ? b : a; }
int tm(int a, int b) { return tmax(a, b) + (int)tmax(1.5, (double)b); }

const char *strs[] = {"one", "two", "three"};
static int __attribute__((noinline)) static_func(int x) { return x + 1; }
int (*fp)(int) = static_func;

__attribute__((weak)) int weak_def = 3;
extern int weak_import __attribute__((weak_import));
int use_weak_import() { return &weak_import ? weak_import : 0; }
__attribute__((visibility("hidden"))) int hidden_var = 5;

typedef float v4 __attribute__((vector_size(16)));
v4 vec(v4 a) { return a * (v4){1.5f, 2.5f, 3.5f, 4.5f}; }
