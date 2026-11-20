/* What runs when a WebAssembly module starts, as wasm-ld links it: a
   constructor (which wasm-ld's __wasm_call_ctors calls, and which has no
   DWARF of its own), and either a WASI command's _start (startup.wasm) or
   a reactor's _initialize (startup-reactor.wasm, -DREACTOR). Freestanding:
   no WASI libc. */

extern void __wasm_call_ctors(void);

int counter;

__attribute__((constructor)) static void init_counter(void) { counter = 41; }

__attribute__((noinline)) int main(void) { return counter + 1; }

#ifdef REACTOR
__attribute__((export_name("_initialize"))) void _initialize(void) { __wasm_call_ctors(); }
__attribute__((export_name("answer"))) int answer(void) { return main(); }
#else
void _start(void) {
    __wasm_call_ctors();
    main();
}
#endif
