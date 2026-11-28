/* An input for Hexcavator's LTCG IL tests (see build.cmd): C's records,
   compiled for x86 too: tentative definitions (commons), calling
   conventions, an inline function's local static, thread-local data. */
struct point { int x, y; };
struct point origin = {1, 2};
int tentative;
int tentative_array[100];
const char *names[] = {"one", "two"};
__declspec(thread) int per_thread = 3;

__inline int *storage(void) {
    static int value;
    return &value;
}

int __cdecl cdecl_fn(int a) { return a + *storage(); }
int __stdcall std_fn(int a, int b) { return a - b; }
int __fastcall fast_fn(int a, int b) { return a * b; }
static int helper(struct point p) { return p.x + p.y + tentative; }

int c_main(void) {
    return cdecl_fn(1) + std_fn(2, 3) + fast_fn(4, 5) + helper(origin) + tentative_array[3] +
           (int)names[1][0] + per_thread;
}
