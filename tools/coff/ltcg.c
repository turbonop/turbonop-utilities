/* An input for Hexcavator's LTCG (/GL) object tests (see build.cmd): C's
   common symbols and x86's decorated names. */
int tentative;
int tentative_array[100];
int __stdcall std_fn(int a, int b) { return a - b + tentative; }
int __fastcall fast_fn(int a, int b) { return a * b + tentative_array[a & 63]; }
int cdecl_fn(int a) { return std_fn(a, 1) + fast_fn(a, 2); }
