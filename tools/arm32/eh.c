/* 32-bit ARM exception tables (EHABI): a function with a cleanup, which
   needs an LSDA for its personality routine (in .ARM.extab), since what it
   calls may throw; one whose unwinding fits in its .ARM.exidx entry; and
   one with a cleanup though nothing it calls can throw. Built with
   -fexceptions; its personality routine and may_throw aren't linked
   (nothing runs these). */

void may_throw(int);

int released;

static void release(int *p) { released += *p; }

__attribute__((noinline)) int with_cleanup(int v) {
    int x __attribute__((cleanup(release))) = v;
    may_throw(x);
    return x + 1;
}

__attribute__((noinline)) int plain(int v) { return v * 3; }

__attribute__((noinline, nothrow)) static int quiet(int v) { return v - 1; }

__attribute__((noinline)) int needless(int v) {
    int x __attribute__((cleanup(release))) = v;
    return quiet(x);
}

int sink;

void start(void) { sink = with_cleanup(sink) + plain(sink) + needless(sink); }
