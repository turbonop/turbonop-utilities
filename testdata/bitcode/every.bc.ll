; ModuleID = 'every.bc'
source_filename = "every.ll"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

module asm
    "  .globl module_asm_symbol"
    "module_asm_symbol:"

%struct.node = type { ptr, i32, %struct.node.inner }
%struct.node.inner = type <{ i8, i16 }>

$any = comdat any

$exact = comdat exactmatch

$largest = comdat largest

$nodedup = comdat nodeduplicate

$samesize = comdat samesize

@private_var = private global i32 1
@internal_var = internal global i64 2, align 8
@external_var = global i32 3, section ".data.every", align 4
@common_var = common global [32 x i8] zeroinitializer, align 16
@weak_var = weak global i8 4, comdat($any)
@linkonce_var = linkonce global i16 5, comdat($exact)
@linkonce_odr_var = linkonce_odr global i32 6, comdat($largest)
@weak_odr_var = weak_odr global i32 7, comdat($nodedup)
@available_var = available_externally global i32 8
@appending_var = appending global [1 x ptr] [ptr @add]
@extern_weak_var = extern_weak global i32
@declared_var = external global i32
@hidden_var = hidden global i32 9
@protected_var = protected global i32 10
@dllexport_var = dllexport global i32 11
@dllimport_var = external dllimport global i32
@tls_general = thread_local global i32 12
@tls_local_dynamic = thread_local(localdynamic) global i32 13
@tls_initial_exec = thread_local(initialexec) global i32 14
@tls_local_exec = thread_local(localexec) global i32 15
@unnamed = private unnamed_addr constant [6 x i8] c"hello\00"
@local_unnamed = internal local_unnamed_addr global i32 16
@in_addrspace = addrspace(1) global i32 17
@samesize_var = global i32 18, comdat($samesize)
@partitioned = global i32 19, partition "part"
@dso_local_var = dso_local global i32 20
@externally_initialized = externally_initialized global i32 21
@i128_value = global i128 170141183460469231731687303715884105727
@half_value = global half 1.000000e+00
@bfloat_value = global bfloat 1.000000e+00
@float_value = global float 1.500000e+00
@double_value = global double 2.250000e+00
@x86_fp80_value = global x86_fp80 1.000000e+00
@fp128_value = global fp128 1.000000e+00
@ppc_fp128_value = global ppc_fp128 1.000000e+00
@struct_value = global %struct.node { ptr null, i32 1, %struct.node.inner <{ i8 2, i16 3 }> }
@array_value = global [3 x i32] [i32 1, i32 2, i32 3]
@vector_value = global <4 x i32> <i32 1, i32 2, i32 3, i32 4>
@zero_vector = global <4 x float> zeroinitializer
@string_value = constant [4 x i8] c"abc\00"
@nonnull_string = constant [3 x i8] c"xyz"
@undef_value = global i32 undef
@poison_value = global i32 poison
@null_value = global ptr null
@aggregate_ptrs = global [2 x ptr] [ptr @external_var, ptr getelementptr inbounds ([3 x i32], ptr @array_value, i64 0, i64 1)]
@expr_ptrtoint = global i64 ptrtoint (ptr @external_var to i64)
@expr_binop = global i64 add (i64 ptrtoint (ptr @external_var to i64), i64 8)
@expr_gep_inrange = global ptr getelementptr inbounds inrange(-8, 8) ([4 x ptr], ptr @appending_var, i64 0, i64 1)
@expr_blockaddress = global ptr blockaddress(@indirect, %second)
@expr_dso_local = global ptr dso_local_equivalent @add
@expr_no_cfi = global ptr no_cfi @add
@expr_ptrauth = global ptr ptrauth (ptr @external_var, i32 2, i64 1234)
@expr_ptrtoaddr = global i64 ptrtoaddr (ptr @external_var to i64)
@byte_value = global b8 7
@byte_array = constant [3 x b8] c"ab\00"

@alias = alias i32, ptr @external_var
@internal_alias = internal alias i32, ptr @external_var
@hidden_alias = hidden alias i32, ptr @external_var

@ifunc = ifunc i32 (i32), ptr @resolver

declare i32 @declared(i32)

declare extern_weak void @declared_extern_weak()

declare dllimport void @declared_dllimport()

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(none)
declare void @llvm.donothing() #0

declare i32 @__gxx_personality_v0(...)

declare i32 @__CxxFrameHandler3(...)

declare void @thrower()

; Function Attrs: nocallback nofree nosync nounwind willreturn
declare void @llvm.va_start.p0(ptr) #1

; Function Attrs: nocallback nofree nosync nounwind willreturn
declare void @llvm.va_end.p0(ptr) #1

; Function Attrs: nocallback nofree nosync nounwind willreturn
declare token @llvm.call.preallocated.setup(i32 immarg) #1

define internal ptr @resolver() {
  ret ptr @add
}

define i32 @add(i32 %a, i32 %b) {
  %sum = add nsw i32 %a, %b
  ret i32 %sum
}

define linkonce_odr hidden i32 @linkonce_odr_function(i32 %x) comdat($any) {
  ret i32 %x
}

define weak i32 @weak_function() {
  ret i32 0
}

define private void @private_function() {
  ret void
}

define protected void @protected_function() section ".text.protected" align 32 gc "statepoint-example" prefix i32 123 prologue i8 -112 {
  ret void
}

define void @binary(i32 %a, i32 %b, float %f, float %g, <2 x i64> %v) {
  %1 = add nuw i32 %a, %b
  %2 = sub i32 %a, %b
  %3 = mul nuw nsw i32 %a, %b
  %4 = udiv exact i32 %a, %b
  %5 = sdiv i32 %a, %b
  %6 = urem i32 %a, %b
  %7 = srem i32 %a, %b
  %8 = shl i32 %a, 3
  %9 = lshr exact i32 %a, 1
  %10 = ashr i32 %a, 2
  %11 = and i32 %a, %b
  %12 = or disjoint i32 %a, 256
  %13 = xor i32 %a, -1
  %14 = fadd fast float %f, %g
  %15 = fsub nnan float %f, %g
  %16 = fmul ninf float %f, %g
  %17 = fdiv nsz float %f, %g
  %18 = frem arcp float %f, %g
  %19 = fneg contract float %f
  %20 = add <2 x i64> %v, <i64 1, i64 2>
  %21 = freeze i32 %a
  ret void
}

define void @casts(i64 %i, double %d, ptr %p, ptr addrspace(1) %q) {
  %1 = trunc nuw i64 %i to i32
  %2 = zext nneg i32 %1 to i64
  %3 = sext i32 %1 to i64
  %4 = fptoui double %d to i32
  %5 = fptosi double %d to i32
  %6 = uitofp nneg i32 %1 to float
  %7 = sitofp i64 %i to double
  %8 = fptrunc double %d to float
  %9 = fpext float %8 to double
  %10 = ptrtoint ptr %p to i64
  %11 = inttoptr i64 %i to ptr
  %12 = bitcast i64 %i to double
  %13 = addrspacecast ptr addrspace(1) %q to ptr
  %14 = ptrtoaddr ptr %p to i64
  ret void
}

define i32 @memory(ptr %p, i32 %v) {
  %slot = alloca i32, align 4
  %array = alloca i32, i32 %v, align 16
  %inalloca = alloca inalloca i64, align 8
  store i32 %v, ptr %slot, align 4
  store volatile i32 %v, ptr %array, align 4
  %1 = load i32, ptr %slot, align 4
  %2 = load volatile i32, ptr %p, align 4, !nontemporal !4
  %3 = load atomic i32, ptr %p seq_cst, align 4
  %4 = load atomic volatile i32, ptr %p syncscope("singlethread") acquire, align 4
  store atomic i32 %1, ptr %p release, align 4
  store atomic i32 %2, ptr %p syncscope("agent") monotonic, align 4
  %5 = getelementptr inbounds %struct.node, ptr %p, i64 1, i32 2, i32 1
  %6 = getelementptr nusw nuw i8, ptr %p, i64 4
  %7 = getelementptr i32, <2 x ptr> splat (ptr null), <2 x i64> <i64 1, i64 2>
  %8 = cmpxchg ptr %p, i32 %1, i32 %2 acq_rel monotonic, align 4
  %9 = cmpxchg weak volatile ptr %p, i32 %1, i32 %2 syncscope("singlethread") seq_cst seq_cst, align 8
  %10 = extractvalue { i32, i1 } %8, 0
  %11 = extractvalue { i32, i1 } %9, 1
  %12 = atomicrmw xchg ptr %p, i32 1 seq_cst, align 4
  %13 = atomicrmw add ptr %p, i32 1 monotonic, align 4
  %14 = atomicrmw sub ptr %p, i32 1 acquire, align 4
  %15 = atomicrmw and ptr %p, i32 1 release, align 4
  %16 = atomicrmw nand ptr %p, i32 1 acq_rel, align 4
  %17 = atomicrmw or ptr %p, i32 1 seq_cst, align 4
  %18 = atomicrmw xor ptr %p, i32 1 seq_cst, align 4
  %19 = atomicrmw max ptr %p, i32 1 seq_cst, align 4
  %20 = atomicrmw min ptr %p, i32 1 seq_cst, align 4
  %21 = atomicrmw umax ptr %p, i32 1 seq_cst, align 4
  %22 = atomicrmw umin ptr %p, i32 1 seq_cst, align 4
  %23 = atomicrmw fadd ptr %p, float 1.000000e+00 seq_cst, align 4
  %24 = atomicrmw fsub ptr %p, float 1.000000e+00 seq_cst, align 4
  %25 = atomicrmw fmax ptr %p, float 1.000000e+00 seq_cst, align 4
  %26 = atomicrmw fmin ptr %p, float 1.000000e+00 seq_cst, align 4
  %27 = atomicrmw uinc_wrap ptr %p, i32 1 seq_cst, align 4
  %28 = atomicrmw udec_wrap ptr %p, i32 1 seq_cst, align 4
  %29 = atomicrmw usub_cond ptr %p, i32 1 seq_cst, align 4
  %30 = atomicrmw usub_sat ptr %p, i32 1 seq_cst, align 4
  %31 = atomicrmw volatile xchg ptr %p, i32 1 syncscope("singlethread") monotonic, align 8
  fence acquire
  fence syncscope("singlethread") seq_cst
  %32 = add i32 %10, %3
  %33 = add i32 %32, %4
  ret i32 %33
}

define <4 x float> @vectors(<4 x float> %a, <4 x float> %b, float %f, <vscale x 4 x i32> %s) {
  %1 = extractelement <4 x float> %a, i32 1
  %2 = insertelement <4 x float> %b, float %f, i64 0
  %3 = shufflevector <4 x float> %a, <4 x float> %2, <4 x i32> <i32 0, i32 5, i32 poison, i32 7>
  %4 = fcmp olt <4 x float> %a, %b
  %5 = select <4 x i1> %4, <4 x float> %a, <4 x float> %3
  %6 = fadd float %1, %f
  %7 = insertelement <vscale x 4 x i32> %s, i32 0, i32 0
  %8 = shufflevector <vscale x 4 x i32> %7, <vscale x 4 x i32> poison, <vscale x 4 x i32> zeroinitializer
  %9 = extractelement <vscale x 4 x i32> %8, i32 0
  ret <4 x float> %5
}

define { i32, float } @aggregates({ i32, float } %agg, [2 x i8] %arr) {
  %1 = extractvalue { i32, float } %agg, 0
  %2 = insertvalue { i32, float } %agg, i32 %1, 0
  %3 = extractvalue [2 x i8] %arr, 1
  %4 = insertvalue [2 x i8] %arr, i8 %3, 0
  ret { i32, float } %2
}

define i32 @control(i32 %x, i1 %c) {
entry:
  %cmp = icmp samesign ult i32 %x, 10
  %fc = fcmp fast oeq float 1.000000e+00, 2.000000e+00
  br i1 %cmp, label %small, label %large

small:                                            ; preds = %entry
  switch i32 %x, label %done [
    i32 0, label %zero
    i32 1, label %one
  ]

zero:                                             ; preds = %small
  br label %done

one:                                              ; preds = %small
  br label %done

large:                                            ; preds = %entry
  %y = select i1 %c, i32 %x, i32 100
  br label %done

done:                                             ; preds = %large, %one, %zero, %small
  %r = phi i32 [ 0, %small ], [ 1, %zero ], [ 2, %one ], [ %y, %large ]
  br label %define

use:                                              ; preds = %define
  %forward = add i32 %later, %r
  %cond = icmp eq i32 %forward, 0
  br i1 %cond, label %exit, label %dead

define:                                           ; preds = %done
  %later = add i32 %r, 1
  br label %use

dead:                                             ; preds = %use
  unreachable

exit:                                             ; preds = %use
  ret i32 %r
}

define i32 @indirect(ptr %target) {
entry:
  indirectbr ptr %target, [label %first, label %second]

first:                                            ; preds = %entry
  ret i32 1

second:                                           ; preds = %entry
  ret i32 2
}

define i32 @calls(i32 %x, ptr %fp, ...) {
  %va = alloca ptr, align 8
  call void @llvm.va_start.p0(ptr %va)
  %arg = va_arg ptr %va, i32
  call void @llvm.va_end.p0(ptr %va)
  %1 = call i32 @add(i32 %x, i32 %arg)
  %2 = tail call fastcc i32 @declared(i32 %1)
  %3 = musttail call i32 (i32, ptr, ...) %fp(i32 %2, ptr %fp, ...)
  ret i32 %3
}

define i32 @more_calls(i32 %x, ptr %fp) {
  %1 = notail call i32 %fp(i32 %x)
  %2 = call i32 asm sideeffect "mov $1, $0", "=r,r"(i32 %1)
  %3 = call nnan float @llvm.fabs.f32(float 1.000000e+00)
  call void @llvm.donothing() [ "deopt"(i32 %x), "funclet-free"() ]
  %4 = call i32 @declared(i32 %2) #3
  call void @declared_extern_weak()
  %5 = call i32 dso_local_equivalent @add(i32 %4, i32 1)
  ret i32 %5
}

; Function Attrs: nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare float @llvm.fabs.f32(float) #2

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(none)
declare x86_amx @llvm.x86.cast.vector.to.tile.v256i32(<256 x i32>) #0

define i32 @call_branch(i32 %x) {
entry:
  %r = callbr i32 asm "", "=r,r,!i"(i32 %x)
          to label %fallthrough [label %indirect]

fallthrough:                                      ; preds = %entry
  ret i32 %r

indirect:                                         ; preds = %entry
  ret i32 0
}

define i32 @itanium_eh(i32 %x) personality ptr @__gxx_personality_v0 {
entry:
  invoke void @thrower()
          to label %normal unwind label %landing

normal:                                           ; preds = %entry
  ret i32 %x

landing:                                          ; preds = %entry
  %lp = landingpad { ptr, i32 }
          cleanup
          catch ptr null
          filter [1 x ptr] [ptr @external_var]
  %sel = extractvalue { ptr, i32 } %lp, 1
  resume { ptr, i32 } %lp
}

define i32 @windows_eh(i32 %x) personality ptr @__CxxFrameHandler3 {
entry:
  invoke void @thrower()
          to label %normal unwind label %dispatch

normal:                                           ; preds = %entry
  invoke void @thrower()
          to label %done unwind label %cleanup

dispatch:                                         ; preds = %entry
  %cs = catchswitch within none [label %handler] unwind label %cleanup

handler:                                          ; preds = %dispatch
  %cp = catchpad within %cs [ptr null, i32 64, ptr null]
  catchret from %cp to label %done

cleanup:                                          ; preds = %dispatch, %normal
  %cl = cleanuppad within none []
  cleanupret from %cl unwind to caller

done:                                             ; preds = %handler, %normal
  ret i32 %x
}

define void @debug_records(i32 %x) !dbg !6 {
  %slot = alloca i32, align 4
    #dbg_declare(ptr %slot, !11, !DIExpression(), !12)
  store i32 %x, ptr %slot, align 4
    #dbg_value(i32 %x, !11, !DIExpression(), !12)
  ret void, !dbg !12
}

define b32 @bytes(b32 %base, i8 %v, ptr %p) {
  %1 = bitinsert b32 %base, i8 %v, i32 8
  %2 = bitextract i8, b32 %1, i32 16
  %3 = load b8, ptr %p, align 1
  ret b32 %1
}

define void @types(half %h, bfloat %bf, x86_fp80 %x, fp128 %q, ppc_fp128 %pq, target("spirv.Image", void, 1, 0) %img, ptr addrspace(3) %p3, i128 %wide, i1 %bit) {
  %1 = fadd half %h, %h
  %2 = fadd bfloat %bf, %bf
  %3 = fadd x86_fp80 %x, %x
  %4 = fadd fp128 %q, %q
  %5 = fadd ppc_fp128 %pq, %pq
  %6 = add i128 %wide, 1
  ret void
}

attributes #0 = { nocallback nofree nosync nounwind willreturn memory(none) }
attributes #1 = { nocallback nofree nosync nounwind willreturn }
attributes #2 = { nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none) }
attributes #3 = { nounwind "key"="value" "string-attribute" }

!llvm.dbg.cu = !{!0}
!llvm.module.flags = !{!2, !3}
!named.metadata = !{!4, !5}

!0 = distinct !DICompileUnit(language: DW_LANG_C99, file: !1, producer: "hand-written", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug)
!1 = !DIFile(filename: "every.ll", directory: "R:\\tools\\bitcode")
!2 = !{i32 2, !"Debug Info Version", i32 3}
!3 = !{i32 7, !"Dwarf Version", i32 5}
!4 = !{i32 1}
!5 = !{!"a string", i64 42, ptr @external_var}
!6 = distinct !DISubprogram(name: "debug_records", linkageName: "debug_records", scope: !1, file: !1, line: 7, type: !7, scopeLine: 7, spFlags: DISPFlagDefinition, unit: !0, retainedNodes: !10)
!7 = !DISubroutineType(types: !8)
!8 = !{null, !9}
!9 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
!10 = !{}
!11 = !DILocalVariable(name: "x", arg: 1, scope: !6, file: !1, line: 7, type: !9)
!12 = !DILocation(line: 8, column: 3, scope: !6)
