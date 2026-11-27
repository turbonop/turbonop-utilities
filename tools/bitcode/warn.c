// The bitcode fixtures' serialized diagnostics (see build.cmd): clang's
// `--serialize-diagnostics` writes a bitstream (DIAG) with its own blocks,
// named by BLOCKINFO, for these warnings and their notes and fix-its.

static int unused_function(void) { return 1; }

int warn(int x) {
    int unused;
    if (x = 3)
        return x;
    return (long)&x;
}
