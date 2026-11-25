/* What Intel XED says each instruction reads and writes: the oracle for
 * turbonop-x86's access data (crates/isa/x86/tests/it/access.rs).
 *
 * Reads lines of `<mode> <hex bytes>` (mode 64 or 32) and writes, for each,
 *
 *     <mode> <hex> | <iclass> <length> | r=<regs> | w=<regs> | m=<r><w> | f=<read>/<written>/<undefined>
 *
 * Registers are XED's names, in lowercase, widened to the largest enclosing
 * register (al and eax are rax; xmm0 is zmm0), sorted and comma-separated;
 * reads include the registers that form memory addresses. `m` says whether
 * any memory operand (explicit or implicit, like the stack) is read and
 * written ("-" for neither). Flags are hexadecimal EFLAGS masks, the
 * written mask including the undefined one. Lines XED can't decode end in
 * `| error <name>`.
 *
 * XED (https://github.com/intelxed/xed) is Apache-2.0. Build and run it with
 * build.sh, in WSL or on Linux. */

#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "xed/xed-interface.h"

enum { MAX_REGS = 64 };

typedef struct {
    const char *names[MAX_REGS];
    int count;
} reg_set;

static void add(reg_set *s, xed_reg_enum_t r) {
    if (r == XED_REG_INVALID) {
        return;
    }
    xed_reg_enum_t wide = xed_get_largest_enclosing_register(r);
    if (wide != XED_REG_INVALID) {
        r = wide;
    }
    const char *name = xed_reg_enum_t2str(r);
    for (int i = 0; i < s->count; i++) {
        if (strcmp(s->names[i], name) == 0) {
            return;
        }
    }
    if (s->count < MAX_REGS) {
        s->names[s->count++] = name;
    }
}

static int compare(const void *a, const void *b) {
    return strcmp(*(const char *const *)a, *(const char *const *)b);
}

static void print_set(const char *label, reg_set *s) {
    qsort(s->names, (size_t)s->count, sizeof s->names[0], compare);
    printf(" | %s=", label);
    for (int i = 0; i < s->count; i++) {
        printf("%s", i ? "," : "");
        for (const char *c = s->names[i]; *c; c++) {
            putchar(tolower((unsigned char)*c));
        }
    }
}

int main(void) {
    xed_tables_init();
    char line[1024];
    while (fgets(line, sizeof line, stdin)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0] || line[0] == '#') {
            continue;
        }
        int mode = atoi(line);
        unsigned char bytes[16];
        unsigned n = 0;
        for (char *p = strchr(line, ' '); p && *p && n < sizeof bytes;) {
            while (*p == ' ') {
                p++;
            }
            if (!*p) {
                break;
            }
            bytes[n++] = (unsigned char)strtoul(p, &p, 16);
        }

        xed_decoded_inst_t xedd;
        xed_decoded_inst_zero(&xedd);
        if (mode == 64) {
            xed_decoded_inst_set_mode(&xedd, XED_MACHINE_MODE_LONG_64, XED_ADDRESS_WIDTH_64b);
        } else {
            xed_decoded_inst_set_mode(&xedd, XED_MACHINE_MODE_LEGACY_32, XED_ADDRESS_WIDTH_32b);
        }
        xed_error_enum_t err = xed_decode(&xedd, bytes, n);
        printf("%s", line);
        if (err != XED_ERROR_NONE) {
            printf(" | error %s\n", xed_error_enum_t2str(err));
            continue;
        }
        printf(" | %s %u", xed_iclass_enum_t2str(xed_decoded_inst_get_iclass(&xedd)),
               xed_decoded_inst_get_length(&xedd));

        reg_set reads = {0}, writes = {0};
        int mem_read = 0, mem_written = 0;
        const xed_inst_t *xi = xed_decoded_inst_inst(&xedd);
        unsigned count = xed_inst_noperands(xi);
        for (unsigned i = 0; i < count; i++) {
            const xed_operand_t *op = xed_inst_operand(xi, i);
            xed_operand_enum_t name = xed_operand_name(op);
            xed_operand_action_enum_t action = xed_decoded_inst_operand_action(&xedd, i);
            if (xed_operand_is_register(name) || xed_operand_is_memory_addressing_register(name)) {
                xed_reg_enum_t r = xed_decoded_inst_get_reg(&xedd, name);
                if (xed_operand_action_read(action)) {
                    add(&reads, r);
                }
                if (xed_operand_action_written(action)) {
                    add(&writes, r);
                }
            } else if (name == XED_OPERAND_MEM0 || name == XED_OPERAND_MEM1 ||
                       name == XED_OPERAND_AGEN) {
                unsigned m = name == XED_OPERAND_MEM1 ? 1 : 0;
                add(&reads, xed_decoded_inst_get_base_reg(&xedd, m));
                add(&reads, xed_decoded_inst_get_index_reg(&xedd, m));
                if (name != XED_OPERAND_AGEN) {
                    mem_read |= xed_decoded_inst_mem_read(&xedd, m) != 0;
                    mem_written |= xed_decoded_inst_mem_written(&xedd, m) != 0;
                }
            }
        }
        print_set("r", &reads);
        print_set("w", &writes);
        printf(" | m=%s%s", mem_read ? "r" : "", mem_written ? "w" : "");
        if (!mem_read && !mem_written) {
            printf("-");
        }
        unsigned fr = 0, fw = 0, fu = 0;
        if (xed_decoded_inst_uses_rflags(&xedd)) {
            const xed_simple_flag_t *rf = xed_decoded_inst_get_rflags_info(&xedd);
            fr = xed_simple_flag_get_read_flag_set(rf)->flat;
            fw = xed_simple_flag_get_written_flag_set(rf)->flat;
            fu = xed_simple_flag_get_undefined_flag_set(rf)->flat;
        }
        printf(" | f=%x/%x/%x\n", fr, fw | fu, fu);
    }
    return 0;
}
