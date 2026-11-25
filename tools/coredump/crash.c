/* A program that crashes on purpose, for core files whose state is known:
 * the main thread waits on a worker (with a small stack, so the core stays
 * small), and the worker dereferences a null pointer three calls deep,
 * after branches whose outcomes its arguments decide.
 *
 * Built and run by make.sh, which records gdb's view of the core as the
 * oracle.
 */
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct node {
    struct node *next;
    int value;
};

volatile uint64_t marker = 0x1122334455667788ull;

__attribute__((noinline)) int leaf(struct node *n, int depth) {
    int total = 0;
    if (depth > 2) {
        total += 100;
    } else {
        total += 1;
    }
    /* n->next is null: this faults. */
    return total + n->next->value;
}

__attribute__((noinline)) int middle(struct node *n, int depth) {
    char buffer[64];
    memset(buffer, 'm', sizeof buffer);
    if (n->value == 42) {
        return leaf(n, depth + 1) + buffer[3];
    }
    return buffer[0];
}

__attribute__((noinline)) int outer(int depth) {
    struct node n = {0, 42};
    return middle(&n, depth + 1);
}

static void *worker(void *arg) {
    (void)arg;
    printf("%d\n", outer(1));
    return 0;
}

int main(void) {
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, 64 * 1024);
    pthread_t t;
    pthread_create(&t, &attr, worker, 0);
    pthread_join(t, 0);
    return (int)marker;
}
