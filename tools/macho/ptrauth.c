// Pointers qualified with __ptrauth (arm64e): signed with a key of their
// own, address-discriminated or not, with a constant discriminator. Their
// DWARF is DW_TAG_LLVM_ptrauth_type (see build-arm64e.cmd).
struct callbacks {
  void (*__ptrauth(0, 1, 0x1234) on_event)(int);
  void *__ptrauth(2, 0, 77) data;
  const char *__ptrauth(3, 1, 0) name;
  int plain;
};

struct callbacks table;

int use(struct callbacks *c) { return c->plain; }
