/* What the linker keeps whatever reaches it. Mach-O (keep-macho, linked
   with -dead_strip): a section marked no_dead_strip, and a list the
   Objective-C runtime reads (a class, by its pointer in __objc_classlist,
   as clang emits one). ELF (keep-elf, --gc-sections -z nostart-stop-gc, as
   GNU ld collects): a section named as a C identifier, which __start_ and
   __stop_ symbols enumerate. */

int sink;

#ifdef __APPLE__
static int kept(int x) { return x + 1; }
__attribute__((section("__DATA,__keep,regular,no_dead_strip"), used))
static int (*const keep_list[])(int) = {kept};

static int class_method(int x) { return x * 3; }
struct cls {
    int (*method)(int);
};
static struct cls the_class = {class_method};
__attribute__((section("__DATA,__objc_classlist,regular,no_dead_strip"), used))
static struct cls *const class_list[] = {&the_class};
#else
static int hooked(int x) { return x * 2; }
__attribute__((section("my_hooks"), used)) static int (*const hook)(int) = hooked;
extern int (*const __start_my_hooks[])(int);
#endif

__attribute__((noinline)) int unreached(int x) { return x - 1; }

void _start(void) {
#ifndef __APPLE__
    /* (The linker defines __start_my_hooks only if something refers to it.) */
    sink = (int)(__UINTPTR_TYPE__)__start_my_hooks;
#endif
}
