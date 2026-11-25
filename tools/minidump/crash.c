/* A program that crashes on purpose and writes a minidump of itself, for
 * dumps whose state is known: the main thread waits on a worker, and the
 * worker dereferences a null pointer three calls deep, after branches
 * whose outcomes its arguments decide. The unhandled exception filter
 * writes the dump (MiniDumpNormal: threads, stacks, modules).
 *
 * Built and run by make.cmd, which records DbgEng's view of the dump as
 * the oracle.
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <dbghelp.h>
#include <stdio.h>
#include <string.h>

#pragma comment(lib, "dbghelp.lib")

struct node {
    struct node *next;
    int value;
};

volatile unsigned long long marker = 0x1122334455667788ull;
static const wchar_t *dump_path;

__declspec(noinline) int leaf(struct node *n, int depth) {
    int total = 0;
    if (depth > 2) {
        total += 100;
    } else {
        total += 1;
    }
    /* n->next is null: this faults. */
    return total + n->next->value;
}

__declspec(noinline) int middle(struct node *n, int depth) {
    char buffer[64];
    memset(buffer, 'm', sizeof buffer);
    if (n->value == 42) {
        return leaf(n, depth + 1) + buffer[3];
    }
    return buffer[0];
}

__declspec(noinline) int outer(int depth) {
    struct node n = {0, 42};
    return middle(&n, depth + 1);
}

static DWORD WINAPI worker(void *arg) {
    (void)arg;
    printf("%d\n", outer(1));
    return 0;
}

static LONG WINAPI write_dump(EXCEPTION_POINTERS *e) {
    HANDLE f = CreateFileW(dump_path, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0);
    MINIDUMP_EXCEPTION_INFORMATION mei = {GetCurrentThreadId(), e, FALSE};
    MiniDumpWriteDump(GetCurrentProcess(), GetCurrentProcessId(), f, MiniDumpNormal, &mei, 0, 0);
    CloseHandle(f);
    return EXCEPTION_EXECUTE_HANDLER;
}

int wmain(int argc, wchar_t **argv) {
    dump_path = argc > 1 ? argv[1] : L"crash.dmp";
    SetUnhandledExceptionFilter(write_dump);
    HANDLE t = CreateThread(0, 64 * 1024, worker, 0, STACK_SIZE_PARAM_IS_A_RESERVATION, 0);
    WaitForSingleObject(t, INFINITE);
    return (int)marker;
}
