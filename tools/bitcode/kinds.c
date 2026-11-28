// The bitcode fixtures' C source (see build.cmd): one function per kind of
// instruction clang makes from C at -O1, and globals of each kind (an
// alias, an ifunc, TLS, a section, a weak definition, a pointer into an
// array).

// (No headers: their paths would be in the debug information.)
typedef __builtin_va_list va_list;
typedef float v4f __attribute__((vector_size(16)));

int global_array[16] = {1, 2, 3};
int *ptr_into_array = &global_array[3];
__attribute__((section(".mydata"))) int in_section = 5;
__thread int tls_counter;
_Atomic int atomic_counter;
extern int external_value;
static const char message[] = "kinds of instructions";

struct pair {
    int a;
    long b;
};

__attribute__((noinline)) int sum_va(int n, ...) {
    va_list ap;
    __builtin_va_start(ap, n);
    int s = 0;
    for (int i = 0; i < n; ++i)
        s += __builtin_va_arg(ap, int);
    __builtin_va_end(ap);
    return s;
}

__attribute__((noinline)) v4f mix(v4f a, v4f b) {
    return __builtin_shufflevector(a, b, 0, 5, 2, 7) * a[1];
}

__attribute__((noinline)) int dispatch(int op, int a, int b) {
    switch (op) {
    case 0:
        return a + b;
    case 1:
        return a - b;
    case 2:
        return a * b;
    case 3:
        return b ? a / b : 0;
    case 7:
        return a << (b & 31);
    default:
        return -1;
    }
}

__attribute__((noinline)) int computed(int i) {
    static void *labels[] = {&&zero, &&one, &&two};
    goto *labels[i % 3];
zero:
    return 10;
one:
    return 20;
two:
    return 30;
}

__attribute__((noinline)) int atomics(int *p) {
    int old = __atomic_fetch_add(p, 2, __ATOMIC_SEQ_CST);
    int expected = old;
    __atomic_compare_exchange_n(p, &expected, 7, 0, __ATOMIC_ACQ_REL,
                                __ATOMIC_ACQUIRE);
    __atomic_thread_fence(__ATOMIC_SEQ_CST);
    __atomic_store_n(p, __atomic_load_n(p, __ATOMIC_ACQUIRE) + 1,
                     __ATOMIC_RELEASE);
    return atomic_counter++ + expected;
}

__attribute__((noinline)) double convert(long x, float f, unsigned char c) {
    return (double)x + f + (double)c + (float)(short)x;
}

__attribute__((noinline)) struct pair make_pair(int a, long b) {
    struct pair p = {a, b * 2};
    return p;
}

__attribute__((noinline)) int minimum(int a, int b) { return a < b ? a : b; }

__attribute__((noinline)) unsigned bits(unsigned x) {
    return __builtin_popcount(x) + __builtin_clz(x | 1);
}

__attribute__((noinline)) int asm_increment(int a) {
    int r;
    __asm__("leal 1(%1), %0" : "=r"(r) : "r"(a));
    return r;
}

__attribute__((noinline)) float negate(float x) { return -x; }

__attribute__((weak)) int weak_function(void) { return 7; }

__attribute__((noinline)) static int helper(int x) { return x * 3; }

int alias_target(void) { return helper(1) + helper(2); }
int alias_function(void) __attribute__((alias("alias_target")));

static int implementation(void) { return 2; }
static void *resolve(void) { return (void *)implementation; }
int ifunc_function(void) __attribute__((ifunc("resolve")));

int (*const operations[])(int) = {helper, asm_increment};

int kinds_main(int argc) {
    v4f a = {1, 2, 3, 4};
    v4f m = mix(a, a);
    struct pair p = make_pair(argc, argc);
    int total = sum_va(3, argc, 2, 3) + dispatch(argc, 4, 5) +
                computed(argc) + atomics(&global_array[argc & 7]) +
                (int)convert(argc, m[2], (unsigned char)argc) + p.a +
                (int)p.b + minimum(argc, 9) + (int)bits(argc) +
                (int)negate((float)argc) + weak_function() + alias_function() +
                ifunc_function() + operations[argc & 1](argc) +
                external_value + tls_counter + *ptr_into_array + in_section +
                message[argc & 15];
    return total;
}
