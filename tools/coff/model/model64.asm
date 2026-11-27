; An ml64 object (see ..\model.cmd): ml64 starts relocations at even
; offsets, so an odd-sized section is followed by a byte of padding.
EXTERN entry:PROC
PUBLIC asm_entry
.data
odd DB 1, 2, 3
.code
asm_entry PROC FRAME
    push rbx
    .pushreg rbx
    .endprolog
    lea rax, odd
    call entry
    pop rbx
    ret
asm_entry ENDP
END
