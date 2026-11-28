// The bitcode fixtures' portable C source (see build.cmd), built for every
// object format with its bitcode embedded (-fembed-bitcode), and as
// Darwin's wrapped LTO bitcode.

int embedded_counter = 1;
static const char embedded_name[] = "embedded";

__attribute__((noinline)) static int embedded_helper(int x) {
    return x * 5 + embedded_name[x & 7];
}

int embedded_entry(int x) {
    embedded_counter += embedded_helper(x);
    return embedded_counter;
}
