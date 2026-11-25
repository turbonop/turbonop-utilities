# Assembled by llvm-mc (see ..\model.cmd): what the compilers don't write
# in one object. Every COMDAT selection, weak externals that alias and
# anti-depend, long section names, a common and a local uninitialized
# symbol, and each kind of section-relative relocation.
	.file	"model.s"
	.def	func;
	.scl	2;
	.type	32;
	.endef
	.text
	.globl	func
	.p2align	4
func:
	callq	weak_target
	callq	anti_target
	leaq	data_in_long_section(%rip), %rax
	retq

	.section	.text,"xr",discard,comdat_any
	.globl	comdat_any
comdat_any:
	retq
	.section	.text,"xr",one_only,comdat_nodup
	.globl	comdat_nodup
comdat_nodup:
	retq
	.section	.text,"xr",same_size,comdat_samesize
	.globl	comdat_samesize
comdat_samesize:
	retq
	.section	.text,"xr",same_contents,comdat_exact
	.globl	comdat_exact
comdat_exact:
	retq
	.section	.text,"xr",largest,comdat_largest
	.globl	comdat_largest
comdat_largest:
	retq
	.section	.xdata,"dr",associative,comdat_any
	.long	1
	.section	.a_very_long_section_name,"dr"
data_in_long_section:
	.long	func
	.secrel32	func
	.secidx	func
	.rva	func
	.symidx	func
	.section	.another_long_section_name_beyond_eight,"dw"
	.quad	func

	.weak	weak_target
	.weak_anti_dep	anti_target
	.set	anti_target, func
	.bss
	.lcomm	local_bss, 64
	.comm	common_symbol, 32, 3
	.data
	.long 7
	.section .drectve,"yn"
	.ascii " /DEFAULTLIB:foo.lib"
