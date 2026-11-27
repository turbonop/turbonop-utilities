; A MASM object's CodeView (ml64 /Zi): procedures, labels, and lines of
; its own. build.cmd assembles it.

_TEXT SEGMENT

PUBLIC asm_add
asm_add PROC
    mov rax, rcx
    add rax, rdx
asm_add_done::
    ret
asm_add ENDP

PUBLIC asm_twice
asm_twice PROC
    lea rax, [rcx + rcx]
    ret
asm_twice ENDP

_TEXT ENDS
END
