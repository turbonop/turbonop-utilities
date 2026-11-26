// arm64 relocations and directives the C compilers make rarely: addends
// on page and branch relocations (ARM64_RELOC_ADDEND pairs), GOT and TLV
// page references, linker optimization hints, data in code, a pointer to
// a GOT entry (POINTER_TO_GOT), SUBTRACTOR pairs, local pointer-sized
// relocations, a symbol pointer section, and a debug section's 4-byte
// local relocation.

	.build_version macos, 13, 0
	.section	__TEXT,__text,regular,pure_instructions
	.globl	_f
	.p2align	2
_f:
Lloh0:
	adrp	x8, _g@PAGE
Lloh1:
	ldr	x8, [x8, _g@PAGEOFF]
	adrp	x9, _g@PAGE+16
	add	x9, x9, _g@PAGEOFF+16
	bl	_h
	b	_h+8
Lloh2:
	adrp	x10, _ext@GOTPAGE
Lloh3:
	ldr	x10, [x10, _ext@GOTPAGEOFF]
	adrp	x0, _tlv@TLVPPAGE
	ldr	x0, [x0, _tlv@TLVPPAGEOFF]
	ret
	.data_region jt8
	.byte	1, 2, 3, 4
	.end_data_region
	.loh AdrpLdr	Lloh0, Lloh1
	.loh AdrpLdrGot	Lloh2, Lloh3

	.section	__DATA,__data
	.p2align	3
L_d:
	.quad	L_d2
	.quad	_f - _g
	.long	_ext@GOT - .
	.long	0
L_d2:
	.quad	_f + 4

	.section	__DATA,__nl_symbol_ptr,non_lazy_symbol_pointers
	.p2align	3
L_ext$non_lazy_ptr:
	.indirect_symbol	_ext
	.quad	0

	.section	__DWARF,__debug_info,regular,debug
Ldebug:
	.long	L_d
	.quad	L_d2

.subsections_via_symbols
