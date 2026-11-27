## x86_64 relocations and directives the C compilers don't make on their
## own: local relocations (to labels with no symbol before them in their
## section) of each PC-relative bias (SIGNED, SIGNED_1, SIGNED_2,
## SIGNED_4), SUBTRACTOR pairs with a local half, GOT and TLV references,
## data in code, symbol pointer sections (indirect symbols), symbol stubs,
## zero-fill sections in another segment, common and local common
## symbols, alt entries, private externs, weak definitions, aliases (one
## to an undefined symbol: N_INDR), literals, and a linker option.

	.build_version macos, 13, 0
	.linker_option "-lfoo", "-lbar"
	.section	__TEXT,__text,regular,pure_instructions
	.globl	_f
	.p2align	4
_f:
	movb	$1, L_data(%rip)
	movw	$2, L_data(%rip)
	movl	$3, L_data(%rip)
	leaq	L_data+8(%rip), %rax
	leaq	L_str(%rip), %rax
	callq	_g
	callq	L_local_code
	movq	_ext@GOTPCREL(%rip), %rax
	movq	_tlv@TLVP(%rip), %rdi
	leaq	L_jt(%rip), %rcx
	jmpq	*%rcx
	.data_region jt32
L_jt:
	.long	L_a-L_jt
	.long	L_b-L_jt
	.end_data_region
L_a:
	nop
L_b:
	nop
L_local_code:
	retq

	.globl	_alt
	.alt_entry	_alt
_alt:
	retq

	.private_extern	_pe
	.globl	_pe
_pe:
	retq

	.globl	_wd
	.weak_definition	_wd
_wd:
	retq

	.globl	_alias
	.set	_alias, _f
	.globl	_alias_undef
	.set	_alias_undef, _undef_target
	.no_dead_strip	_f

	.section	__TEXT,__stubs,symbol_stubs,pure_instructions,6
L_ext$stub:
	.indirect_symbol	_ext2
	jmpq	*L_ext2$lazy_ptr(%rip)

	.section	__DATA,__la_symbol_ptr,lazy_symbol_pointers
L_ext2$lazy_ptr:
	.indirect_symbol	_ext2
	.quad	0

	.section	__DATA,__data
	.p2align	3
L_data:
	.quad	0
	.quad	L_data2
	.quad	_f - L_data2
	.quad	L_data2 - _f
	.quad	_ext + 16
	.long	L_str - L_data
L_data2:
	.long	0

	.section	__DATA,__nl_symbol_ptr,non_lazy_symbol_pointers
	.p2align	3
L_ext$non_lazy_ptr:
	.indirect_symbol	_ext
	.quad	0
L_f$non_lazy_ptr:
	.indirect_symbol	_f
	.quad	0

	.section	__TEXT,__cstring,cstring_literals
L_str:
	.asciz	"hi"

	.section	__TEXT,__literal16,16byte_literals
	.p2align	4
L_lit16:
	.quad	1
	.quad	2

	.zerofill	__MYSEG,__myzero,_myzero,64,4
	.comm	_common_x,32,4
	.lcomm	_lcomm_y,16,3
	.tbss	_tbss_z$tlv$init,8,3

	.section	__DATA,__thread_vars,thread_local_variables
	.globl	_tbss_z
_tbss_z:
	.quad	__tlv_bootstrap
	.quad	0
	.quad	_tbss_z$tlv$init

	.section	__DWARF,__debug_info,regular,debug
Ldebug:
	.quad	L_local_code
	.long	L_data2

.subsections_via_symbols
