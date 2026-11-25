; An armasm64 object (see ..\model.cmd): with -g, its functions have
; function definition, .bf and .ef records, and its .file name several
; records.
    AREA |.text|, CODE, READONLY
    IMPORT entry
    EXPORT asm_entry
asm_entry PROC
    stp x29, x30, [sp, #-16]!
    bl entry
    adrp x0, data_value
    add x0, x0, data_value
    ldp x29, x30, [sp], #16
    ret
    ENDP
    AREA |.data|, DATA
data_value DCQ 1234
    END
