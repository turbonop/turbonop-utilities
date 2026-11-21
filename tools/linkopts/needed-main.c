/* A program linked with two libraries (--no-as-needed), calling one: what
   hexc recommend finds unused, by what each library provides. */

int used_fn(int x);

volatile int sink;

void _start(void) {
    sink = used_fn(41);
    for (;;) {
    }
}
