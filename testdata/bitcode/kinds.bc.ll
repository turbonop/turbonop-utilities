; ModuleID = 'kinds.bc'
source_filename = "R:\\tools\\bitcode\\kinds.c"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

%struct.__va_list_tag = type { i32, i32, ptr, ptr }

@global_array = dso_local global <{ i32, i32, i32, [13 x i32] }> <{ i32 1, i32 2, i32 3, [13 x i32] zeroinitializer }>, align 16, !dbg !0
@ptr_into_array = dso_local local_unnamed_addr global ptr getelementptr inbounds nuw (i8, ptr @global_array, i64 12), align 8, !dbg !12
@in_section = dso_local local_unnamed_addr global i32 5, section ".mydata", align 4, !dbg !16
@computed.labels = internal unnamed_addr constant [3 x ptr] [ptr blockaddress(@computed, %8), ptr blockaddress(@computed, %6), ptr blockaddress(@computed, %7)], align 16, !dbg !42
@atomic_counter = dso_local global i32 0, align 4, !dbg !29
@operations = dso_local local_unnamed_addr constant [2 x ptr] [ptr @helper, ptr @asm_increment], align 16, !dbg !18
@external_value = external local_unnamed_addr global i32, align 4
@tls_counter = dso_local thread_local local_unnamed_addr global i32 0, align 4, !dbg !27
@message = internal unnamed_addr constant [22 x i8] c"kinds of instructions\00", align 16, !dbg !32

@alias_function = dso_local alias i32 (), ptr @alias_target

@ifunc_function = dso_local ifunc i32 (), ptr @resolve

; Function Attrs: nofree noinline norecurse nosync nounwind uwtable
define dso_local i32 @sum_va(i32 noundef %0, ...) local_unnamed_addr #0 !dbg !66 {
  %2 = alloca [1 x %struct.__va_list_tag], align 16, !DIAssignID !87
    #dbg_assign(i1 poison, !71, !DIExpression(), !87, ptr %2, !DIExpression(), !88)
    #dbg_value(i32 %0, !70, !DIExpression(), !88)
  call void @llvm.lifetime.start.p0(ptr nonnull %2), !dbg !89
  call void @llvm.va_start.p0(ptr nonnull %2), !dbg !90
    #dbg_value(i32 0, !84, !DIExpression(), !88)
    #dbg_value(i32 0, !85, !DIExpression(), !91)
    #dbg_value(i32 0, !85, !DIExpression(), !91)
    #dbg_value(i32 0, !84, !DIExpression(), !88)
  %3 = icmp sgt i32 %0, 0, !dbg !92
  br i1 %3, label %4, label %10, !dbg !94

4:                                                ; preds = %1
  %5 = load i32, ptr %2, align 16
  %6 = getelementptr inbounds nuw i8, ptr %2, i64 8
  %7 = getelementptr inbounds nuw i8, ptr %2, i64 16
  %8 = load ptr, ptr %7, align 16
  %9 = load ptr, ptr %6, align 8
  br label %12, !dbg !94

10:                                               ; preds = %24, %1
  %11 = phi i32 [ 0, %1 ], [ %29, %24 ], !dbg !88
  call void @llvm.va_end.p0(ptr %2), !dbg !95
  call void @llvm.lifetime.end.p0(ptr nonnull %2), !dbg !96
  ret i32 %11, !dbg !97

12:                                               ; preds = %4, %24
  %13 = phi ptr [ %9, %4 ], [ %25, %24 ]
  %14 = phi i32 [ 0, %4 ], [ %30, %24 ]
  %15 = phi i32 [ 0, %4 ], [ %29, %24 ]
  %16 = phi i32 [ %5, %4 ], [ %26, %24 ]
    #dbg_value(i32 %14, !85, !DIExpression(), !91)
    #dbg_value(i32 %15, !84, !DIExpression(), !88)
  %17 = icmp ult i32 %16, 41, !dbg !98
  br i1 %17, label %18, label %22, !dbg !98

18:                                               ; preds = %12
  %19 = zext nneg i32 %16 to i64, !dbg !98
  %20 = getelementptr i8, ptr %8, i64 %19, !dbg !98
  %21 = add nuw nsw i32 %16, 8, !dbg !98
  store i32 %21, ptr %2, align 16, !dbg !98, !DIAssignID !99
    #dbg_assign(i32 %21, !71, !DIExpression(DW_OP_LLVM_fragment, 0, 32), !99, ptr %2, !DIExpression(), !88)
  br label %24, !dbg !98

22:                                               ; preds = %12
  %23 = getelementptr i8, ptr %13, i64 8, !dbg !98
  store ptr %23, ptr %6, align 8, !dbg !98, !DIAssignID !100
    #dbg_assign(ptr %23, !71, !DIExpression(DW_OP_LLVM_fragment, 64, 64), !100, ptr %6, !DIExpression(), !88)
  br label %24, !dbg !98

24:                                               ; preds = %22, %18
  %25 = phi ptr [ %13, %18 ], [ %23, %22 ]
  %26 = phi i32 [ %21, %18 ], [ %16, %22 ]
  %27 = phi ptr [ %20, %18 ], [ %13, %22 ], !dbg !98
  %28 = load i32, ptr %27, align 4, !dbg !98, !tbaa !101
  %29 = add nsw i32 %28, %15, !dbg !102
    #dbg_value(i32 %29, !84, !DIExpression(), !88)
  %30 = add nuw nsw i32 %14, 1, !dbg !103
    #dbg_value(i32 %30, !85, !DIExpression(), !91)
  %31 = icmp eq i32 %30, %0, !dbg !104
  br i1 %31, label %10, label %12, !dbg !105, !llvm.loop !106
}

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(argmem: readwrite)
declare void @llvm.lifetime.start.p0(ptr captures(none)) #1

; Function Attrs: nocallback nofree nosync nounwind willreturn
declare void @llvm.va_start.p0(ptr) #2

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(argmem: readwrite)
declare void @llvm.lifetime.end.p0(ptr captures(none)) #1

; Function Attrs: nocallback nofree nosync nounwind willreturn
declare void @llvm.va_end.p0(ptr) #2

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local <4 x float> @mix(<4 x float> noundef %0, <4 x float> noundef %1) local_unnamed_addr #3 !dbg !111 {
    #dbg_value(<4 x float> %0, !119, !DIExpression(), !121)
    #dbg_value(<4 x float> %1, !120, !DIExpression(), !121)
  %3 = shufflevector <4 x float> %0, <4 x float> %1, <4 x i32> <i32 0, i32 5, i32 2, i32 7>, !dbg !122
  %4 = shufflevector <4 x float> %0, <4 x float> poison, <4 x i32> <i32 1, i32 1, i32 1, i32 1>, !dbg !123
  %5 = fmul <4 x float> %4, %3, !dbg !124
  ret <4 x float> %5, !dbg !125
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local i32 @dispatch(i32 noundef %0, i32 noundef %1, i32 noundef %2) local_unnamed_addr #4 !dbg !126 {
    #dbg_value(i32 %0, !130, !DIExpression(), !133)
    #dbg_value(i32 %1, !131, !DIExpression(), !133)
    #dbg_value(i32 %2, !132, !DIExpression(), !133)
  switch i32 %0, label %17 [
    i32 0, label %4
    i32 1, label %6
    i32 2, label %8
    i32 3, label %10
    i32 7, label %14
  ], !dbg !134

4:                                                ; preds = %3
  %5 = add nsw i32 %2, %1, !dbg !135
  br label %17, !dbg !137

6:                                                ; preds = %3
  %7 = sub nsw i32 %1, %2, !dbg !138
  br label %17, !dbg !139

8:                                                ; preds = %3
  %9 = mul nsw i32 %2, %1, !dbg !140
  br label %17, !dbg !141

10:                                               ; preds = %3
  %11 = icmp eq i32 %2, 0, !dbg !142
  br i1 %11, label %17, label %12, !dbg !143

12:                                               ; preds = %10
  %13 = sdiv i32 %1, %2, !dbg !144
  br label %17, !dbg !145

14:                                               ; preds = %3
  %15 = and i32 %2, 31, !dbg !146
  %16 = shl i32 %1, %15, !dbg !147
  br label %17, !dbg !148

17:                                               ; preds = %3, %12, %10, %14, %8, %6, %4
  %18 = phi i32 [ 0, %10 ], [ %5, %4 ], [ %7, %6 ], [ %9, %8 ], [ %16, %14 ], [ %13, %12 ], [ -1, %3 ], !dbg !149
  ret i32 %18, !dbg !150
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local range(i32 10, 31) i32 @computed(i32 noundef %0) local_unnamed_addr #4 !dbg !44 {
    #dbg_value(i32 %0, !46, !DIExpression(), !151)
  %2 = srem i32 %0, 3, !dbg !152
  %3 = sext i32 %2 to i64, !dbg !153
  %4 = getelementptr inbounds [8 x i8], ptr @computed.labels, i64 %3, !dbg !153
  %5 = load ptr, ptr %4, align 8, !dbg !153, !tbaa !154
  indirectbr ptr %5, [label %8, label %6, label %7]

6:                                                ; preds = %1
    #dbg_label(!48, !156)
  br label %8, !dbg !157

7:                                                ; preds = %1
    #dbg_label(!49, !158)
  br label %8, !dbg !159

8:                                                ; preds = %1, %7, %6
  %9 = phi i32 [ 30, %7 ], [ 20, %6 ], [ 10, %1 ], !dbg !151
  ret i32 %9, !dbg !160
}

; Function Attrs: mustprogress noinline norecurse nounwind willreturn uwtable
define dso_local i32 @atomics(ptr nofree noundef captures(none) %0) local_unnamed_addr #5 !dbg !161 {
    #dbg_value(ptr %0, !165, !DIExpression(), !168)
  %2 = atomicrmw add ptr %0, i32 2 seq_cst, align 4, !dbg !169
    #dbg_value(i32 %2, !166, !DIExpression(), !168)
    #dbg_value(i32 %2, !167, !DIExpression(), !168)
  %3 = cmpxchg ptr %0, i32 %2, i32 7 acq_rel acquire, align 4, !dbg !170
  %4 = extractvalue { i32, i1 } %3, 0
    #dbg_value(i32 %4, !167, !DIExpression(), !168)
  fence seq_cst, !dbg !171
  %5 = load atomic i32, ptr %0 acquire, align 4, !dbg !172
  %6 = add nsw i32 %5, 1, !dbg !173
  store atomic i32 %6, ptr %0 release, align 4, !dbg !174
  %7 = atomicrmw add ptr @atomic_counter, i32 1 seq_cst, align 4, !dbg !175
  %8 = add nsw i32 %7, %4, !dbg !176
  ret i32 %8, !dbg !177
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local noundef double @convert(i64 noundef %0, float noundef %1, i8 noundef zeroext %2) local_unnamed_addr #4 !dbg !178 {
    #dbg_value(i64 %0, !183, !DIExpression(), !186)
    #dbg_value(float %1, !184, !DIExpression(), !186)
    #dbg_value(i8 %2, !185, !DIExpression(), !186)
  %4 = sitofp i64 %0 to double, !dbg !187
  %5 = fpext float %1 to double, !dbg !188
  %6 = fadd double %4, %5, !dbg !189
  %7 = uitofp i8 %2 to double, !dbg !190
  %8 = fadd double %6, %7, !dbg !191
  %9 = trunc i64 %0 to i16, !dbg !192
  %10 = sitofp i16 %9 to double, !dbg !193
  %11 = fadd double %8, %10, !dbg !194
  ret double %11, !dbg !195
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local { i32, i64 } @make_pair(i32 noundef %0, i64 noundef %1) local_unnamed_addr #4 !dbg !196 {
    #dbg_value(i32 %0, !204, !DIExpression(), !207)
    #dbg_value(i64 %1, !205, !DIExpression(), !207)
    #dbg_value(i32 %0, !206, !DIExpression(DW_OP_LLVM_fragment, 0, 32), !207)
    #dbg_value(i32 0, !206, !DIExpression(DW_OP_LLVM_fragment, 32, 32), !207)
  %3 = shl nsw i64 %1, 1, !dbg !208
    #dbg_value(i64 %3, !206, !DIExpression(DW_OP_LLVM_fragment, 64, 64), !207)
  %4 = insertvalue { i32, i64 } poison, i32 %0, 0, !dbg !209
  %5 = insertvalue { i32, i64 } %4, i64 %3, 1, !dbg !209
  ret { i32, i64 } %5, !dbg !210
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local noundef i32 @minimum(i32 noundef %0, i32 noundef %1) local_unnamed_addr #4 !dbg !211 {
    #dbg_value(i32 %0, !215, !DIExpression(), !217)
    #dbg_value(i32 %1, !216, !DIExpression(), !217)
  %3 = tail call i32 @llvm.smin.i32(i32 %0, i32 %1), !dbg !218
  ret i32 %3, !dbg !219
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local range(i32 0, 64) i32 @bits(i32 noundef %0) local_unnamed_addr #4 !dbg !220 {
    #dbg_value(i32 %0, !224, !DIExpression(), !225)
  %2 = tail call range(i32 0, 33) i32 @llvm.ctpop.i32(i32 %0), !dbg !226
  %3 = or i32 %0, 1, !dbg !227
  %4 = tail call range(i32 0, 32) i32 @llvm.ctlz.i32(i32 %3, i1 true), !dbg !228
  %5 = add nuw nsw i32 %4, %2, !dbg !229
  ret i32 %5, !dbg !230
}

; Function Attrs: nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare i32 @llvm.ctpop.i32(i32) #6

; Function Attrs: nocallback nofree nosync nounwind speculatable willreturn memory(none)
declare i32 @llvm.ctlz.i32(i32, i1 immarg) #7

; Function Attrs: nofree noinline nosync nounwind memory(none) uwtable
define dso_local i32 @asm_increment(i32 noundef %0) #8 !dbg !231 {
    #dbg_value(i32 %0, !233, !DIExpression(), !235)
  %2 = tail call i32 asm "leal 1($1), $0", "=r,r,~{dirflag},~{fpsr},~{flags}"(i32 %0) #13, !dbg !236, !srcloc !237
    #dbg_value(i32 %2, !234, !DIExpression(), !235)
  ret i32 %2, !dbg !238
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local noundef float @negate(float noundef %0) local_unnamed_addr #4 !dbg !239 {
    #dbg_value(float %0, !243, !DIExpression(), !244)
  %2 = fneg float %0, !dbg !245
  ret float %2, !dbg !246
}

; Function Attrs: nounwind uwtable
define weak dso_local i32 @weak_function() local_unnamed_addr #9 !dbg !247 {
  ret i32 7, !dbg !250
}

; Function Attrs: mustprogress nofree norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local i32 @alias_target() #10 !dbg !251 {
  %1 = tail call i32 @helper(i32 noundef 1), !dbg !252
  %2 = tail call i32 @helper(i32 noundef 2), !dbg !253
  %3 = add nsw i32 %2, %1, !dbg !254
  ret i32 %3, !dbg !255
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define internal i32 @helper(i32 noundef %0) #4 !dbg !256 {
    #dbg_value(i32 %0, !258, !DIExpression(), !259)
  %2 = mul nsw i32 %0, 3, !dbg !260
  ret i32 %2, !dbg !261
}

; Function Attrs: nounwind uwtable
define dso_local i32 @kinds_main(i32 noundef %0) local_unnamed_addr #11 !dbg !262 {
    #dbg_value(i32 %0, !264, !DIExpression(), !269)
    #dbg_value(<4 x float> <float 1.000000e+00, float 2.000000e+00, float 3.000000e+00, float 4.000000e+00>, !265, !DIExpression(), !269)
  %2 = tail call <4 x float> @mix(<4 x float> noundef <float 1.000000e+00, float 2.000000e+00, float 3.000000e+00, float 4.000000e+00>, <4 x float> noundef <float 1.000000e+00, float 2.000000e+00, float 3.000000e+00, float 4.000000e+00>), !dbg !270
    #dbg_value(<4 x float> %2, !266, !DIExpression(), !269)
  %3 = sext i32 %0 to i64, !dbg !271
  %4 = tail call { i32, i64 } @make_pair(i32 noundef %0, i64 noundef %3), !dbg !272
  %5 = extractvalue { i32, i64 } %4, 0, !dbg !273
    #dbg_value(i32 %5, !267, !DIExpression(DW_OP_LLVM_fragment, 0, 32), !269)
  %6 = extractvalue { i32, i64 } %4, 1, !dbg !273
    #dbg_value(i64 %6, !267, !DIExpression(DW_OP_LLVM_fragment, 64, 64), !269)
  %7 = tail call i32 (i32, ...) @sum_va(i32 noundef 3, i32 noundef %0, i32 noundef 2, i32 noundef 3), !dbg !274
  %8 = tail call i32 @dispatch(i32 noundef %0, i32 noundef 4, i32 noundef 5), !dbg !275
  %9 = tail call i32 @computed(i32 noundef %0), !dbg !276
  %10 = and i32 %0, 7, !dbg !277
  %11 = zext nneg i32 %10 to i64, !dbg !278
  %12 = getelementptr inbounds nuw [4 x i8], ptr @global_array, i64 %11, !dbg !278
  %13 = tail call i32 @atomics(ptr noundef nonnull %12), !dbg !279
  %14 = extractelement <4 x float> %2, i64 2, !dbg !280
  %15 = trunc i32 %0 to i8, !dbg !281
  %16 = tail call double @convert(i64 noundef %3, float noundef %14, i8 noundef zeroext %15), !dbg !282
  %17 = fptosi double %16 to i32, !dbg !283
  %18 = trunc i64 %6 to i32, !dbg !284
  %19 = tail call i32 @minimum(i32 noundef %0, i32 noundef 9), !dbg !285
  %20 = tail call i32 @bits(i32 noundef %0), !dbg !286
  %21 = sitofp i32 %0 to float, !dbg !287
  %22 = tail call float @negate(float noundef %21), !dbg !288
  %23 = fptosi float %22 to i32, !dbg !289
  %24 = tail call i32 @weak_function(), !dbg !290
  %25 = tail call i32 @helper(i32 noundef 1), !dbg !291
  %26 = tail call i32 @helper(i32 noundef 2), !dbg !293
  %27 = and i32 %0, 1, !dbg !294
  %28 = zext nneg i32 %27 to i64, !dbg !295
  %29 = getelementptr inbounds nuw [8 x i8], ptr @operations, i64 %28, !dbg !295
  %30 = load ptr, ptr %29, align 8, !dbg !295, !tbaa !154
  %31 = tail call i32 %30(i32 noundef %0) #14, !dbg !295
  %32 = load i32, ptr @external_value, align 4, !dbg !296, !tbaa !101
  %33 = tail call align 4 ptr @llvm.threadlocal.address.p0(ptr align 4 @tls_counter), !dbg !297
  %34 = load i32, ptr %33, align 4, !dbg !297, !tbaa !101
  %35 = load ptr, ptr @ptr_into_array, align 8, !dbg !298, !tbaa !299
  %36 = load i32, ptr %35, align 4, !dbg !301, !tbaa !101
  %37 = load i32, ptr @in_section, align 4, !dbg !302, !tbaa !101
  %38 = and i32 %0, 15, !dbg !303
  %39 = zext nneg i32 %38 to i64, !dbg !304
  %40 = getelementptr inbounds nuw i8, ptr @message, i64 %39, !dbg !304
  %41 = load i8, ptr %40, align 1, !dbg !304, !tbaa !305
  %42 = sext i8 %41 to i32, !dbg !304
  %43 = add i32 %7, 2, !dbg !306
  %44 = add i32 %43, %5, !dbg !307
  %45 = add i32 %44, %8, !dbg !308
  %46 = add i32 %45, %18, !dbg !309
  %47 = add i32 %46, %9, !dbg !310
  %48 = add i32 %47, %13, !dbg !311
  %49 = add i32 %48, %17, !dbg !312
  %50 = add i32 %49, %19, !dbg !313
  %51 = add i32 %50, %20, !dbg !314
  %52 = add i32 %51, %23, !dbg !315
  %53 = add i32 %52, %24, !dbg !316
  %54 = add i32 %53, %25, !dbg !317
  %55 = add i32 %54, %26, !dbg !318
  %56 = add nsw i32 %55, %31, !dbg !319
  %57 = add nsw i32 %56, %32, !dbg !320
  %58 = add nsw i32 %57, %34, !dbg !321
  %59 = add nsw i32 %58, %36, !dbg !322
  %60 = add nsw i32 %59, %37, !dbg !323
  %61 = add nsw i32 %60, %42, !dbg !324
    #dbg_value(i32 %61, !268, !DIExpression(), !269)
  ret i32 %61, !dbg !325
}

; Function Attrs: nocallback nofree nosync nounwind speculatable willreturn memory(none)
declare nonnull ptr @llvm.threadlocal.address.p0(ptr nonnull) #7

; Function Attrs: disable_sanitizer_instrumentation nounwind uwtable
define internal ptr @resolve() #12 !dbg !326 {
  ret ptr @implementation, !dbg !329
}

; Function Attrs: mustprogress nofree norecurse nosync nounwind willreturn memory(none) uwtable
define internal noundef i32 @implementation() #10 !dbg !330 {
  ret i32 2, !dbg !331
}

; Function Attrs: nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare i32 @llvm.smin.i32(i32, i32) #6

attributes #0 = { nofree noinline norecurse nosync nounwind uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #1 = { nocallback nofree nosync nounwind willreturn memory(argmem: readwrite) }
attributes #2 = { nocallback nofree nosync nounwind willreturn }
attributes #3 = { mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable "min-legal-vector-width"="128" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #4 = { mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #5 = { mustprogress noinline norecurse nounwind willreturn uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #6 = { nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none) }
attributes #7 = { nocallback nofree nosync nounwind speculatable willreturn memory(none) }
attributes #8 = { nofree noinline nosync nounwind memory(none) uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #9 = { nounwind uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #10 = { mustprogress nofree norecurse nosync nounwind willreturn memory(none) uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #11 = { nounwind uwtable "min-legal-vector-width"="128" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #12 = { disable_sanitizer_instrumentation nounwind uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #13 = { nounwind memory(none) }
attributes #14 = { nounwind }

!llvm.dbg.cu = !{!2}
!llvm.module.flags = !{!53, !54, !55, !56, !57, !58, !59}
!llvm.ident = !{!60}
!llvm.errno.tbaa = !{!61}

!0 = !DIGlobalVariableExpression(var: !1, expr: !DIExpression())
!1 = distinct !DIGlobalVariable(name: "global_array", scope: !2, file: !14, line: 10, type: !39, isLocal: false, isDefinition: true)
!2 = distinct !DICompileUnit(language: DW_LANG_C11, file: !3, producer: "clang version 24.0.0git (https://github.com/llvm/llvm-project.git c74b358f44c31e3615fe942556a41be8c9a8d877)", isOptimized: true, runtimeVersion: 0, emissionKind: FullDebug, retainedTypes: !4, globals: !11, splitDebugInlining: false, nameTableKind: None)
!3 = !DIFile(filename: "R:\\tools\\bitcode\\kinds.c", directory: "R:\\target\\fixtures\\bitcode", checksumkind: CSK_MD5, checksum: "eb806a2a69008090eda09adc6b3c6113")
!4 = !{!5, !6, !7, !8, !9, !10}
!5 = !DIBasicType(name: "double", size: 64, encoding: DW_ATE_float)
!6 = !DIBasicType(name: "float", size: 32, encoding: DW_ATE_float)
!7 = !DIBasicType(name: "short", size: 16, encoding: DW_ATE_signed)
!8 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
!9 = !DIBasicType(name: "unsigned char", size: 8, encoding: DW_ATE_unsigned_char)
!10 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: null, size: 64)
!11 = !{!0, !12, !16, !18, !27, !29, !32}
!12 = !DIGlobalVariableExpression(var: !13, expr: !DIExpression())
!13 = distinct !DIGlobalVariable(name: "ptr_into_array", scope: !2, file: !14, line: 11, type: !15, isLocal: false, isDefinition: true)
!14 = !DIFile(filename: "R:\\tools\\bitcode\\kinds.c", directory: "", checksumkind: CSK_MD5, checksum: "eb806a2a69008090eda09adc6b3c6113")
!15 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !8, size: 64)
!16 = !DIGlobalVariableExpression(var: !17, expr: !DIExpression())
!17 = distinct !DIGlobalVariable(name: "in_section", scope: !2, file: !14, line: 12, type: !8, isLocal: false, isDefinition: true)
!18 = !DIGlobalVariableExpression(var: !19, expr: !DIExpression())
!19 = distinct !DIGlobalVariable(name: "operations", scope: !2, file: !14, line: 110, type: !20, isLocal: false, isDefinition: true)
!20 = !DICompositeType(tag: DW_TAG_array_type, baseType: !21, size: 128, elements: !25)
!21 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !22)
!22 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !23, size: 64)
!23 = !DISubroutineType(types: !24)
!24 = !{!8, !8}
!25 = !{!26}
!26 = !DISubrange(count: 2)
!27 = !DIGlobalVariableExpression(var: !28, expr: !DIExpression())
!28 = distinct !DIGlobalVariable(name: "tls_counter", scope: !2, file: !14, line: 13, type: !8, isLocal: false, isDefinition: true)
!29 = !DIGlobalVariableExpression(var: !30, expr: !DIExpression())
!30 = distinct !DIGlobalVariable(name: "atomic_counter", scope: !2, file: !14, line: 14, type: !31, isLocal: false, isDefinition: true)
!31 = !DIDerivedType(tag: DW_TAG_atomic_type, baseType: !8)
!32 = !DIGlobalVariableExpression(var: !33, expr: !DIExpression())
!33 = distinct !DIGlobalVariable(name: "message", scope: !2, file: !14, line: 16, type: !34, isLocal: true, isDefinition: true)
!34 = !DICompositeType(tag: DW_TAG_array_type, baseType: !35, size: 176, elements: !37)
!35 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !36)
!36 = !DIBasicType(name: "char", size: 8, encoding: DW_ATE_signed_char)
!37 = !{!38}
!38 = !DISubrange(count: 22)
!39 = !DICompositeType(tag: DW_TAG_array_type, baseType: !8, size: 512, elements: !40)
!40 = !{!41}
!41 = !DISubrange(count: 16)
!42 = !DIGlobalVariableExpression(var: !43, expr: !DIExpression())
!43 = distinct !DIGlobalVariable(name: "labels", scope: !44, file: !14, line: 55, type: !50, isLocal: true, isDefinition: true)
!44 = distinct !DISubprogram(name: "computed", scope: !14, file: !14, line: 54, type: !23, scopeLine: 54, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !45, keyInstructions: true)
!45 = !{!46, !42, !47, !48, !49}
!46 = !DILocalVariable(name: "i", arg: 1, scope: !44, file: !14, line: 54, type: !8)
!47 = !DILabel(scope: !44, name: "zero", file: !14, line: 57, column: 1)
!48 = !DILabel(scope: !44, name: "one", file: !14, line: 59, column: 1)
!49 = !DILabel(scope: !44, name: "two", file: !14, line: 61, column: 1)
!50 = !DICompositeType(tag: DW_TAG_array_type, baseType: !10, size: 192, elements: !51)
!51 = !{!52}
!52 = !DISubrange(count: 3)
!53 = !{i32 7, !"Dwarf Version", i32 5}
!54 = !{i32 2, !"Debug Info Version", i32 3}
!55 = !{i32 2, !"debugger-tuning", !"gdb"}
!56 = !{i32 8, !"PIC Level", i32 2}
!57 = !{i32 7, !"PIE Level", i32 2}
!58 = !{i32 7, !"uwtable", i32 2}
!59 = !{i32 7, !"debug-info-assignment-tracking", i1 true}
!60 = !{!"clang version 24.0.0git (https://github.com/llvm/llvm-project.git c74b358f44c31e3615fe942556a41be8c9a8d877)"}
!61 = !{!62, !63, i64 0}
!62 = !{!"__libc_errno", !63, i64 0}
!63 = !{!"int", !64, i64 0}
!64 = !{!"omnipotent char", !65, i64 0}
!65 = !{!"Simple C/C++ TBAA"}
!66 = distinct !DISubprogram(name: "sum_va", scope: !14, file: !14, line: 23, type: !67, scopeLine: 23, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !69, keyInstructions: true)
!67 = !DISubroutineType(types: !68)
!68 = !{!8, !8, null}
!69 = !{!70, !71, !84, !85}
!70 = !DILocalVariable(name: "n", arg: 1, scope: !66, file: !14, line: 23, type: !8)
!71 = !DILocalVariable(name: "ap", scope: !66, file: !14, line: 24, type: !72)
!72 = !DIDerivedType(tag: DW_TAG_typedef, name: "va_list", file: !14, line: 7, baseType: !73)
!73 = !DIDerivedType(tag: DW_TAG_typedef, name: "__builtin_va_list", file: !14, baseType: !74)
!74 = !DICompositeType(tag: DW_TAG_array_type, baseType: !75, size: 192, elements: !82)
!75 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "__va_list_tag", size: 192, elements: !76)
!76 = !{!77, !79, !80, !81}
!77 = !DIDerivedType(tag: DW_TAG_member, name: "gp_offset", scope: !75, file: !14, line: 24, baseType: !78, size: 32)
!78 = !DIBasicType(name: "unsigned int", size: 32, encoding: DW_ATE_unsigned)
!79 = !DIDerivedType(tag: DW_TAG_member, name: "fp_offset", scope: !75, file: !14, line: 24, baseType: !78, size: 32, offset: 32)
!80 = !DIDerivedType(tag: DW_TAG_member, name: "overflow_arg_area", scope: !75, file: !14, line: 24, baseType: !10, size: 64, offset: 64)
!81 = !DIDerivedType(tag: DW_TAG_member, name: "reg_save_area", scope: !75, file: !14, line: 24, baseType: !10, size: 64, offset: 128)
!82 = !{!83}
!83 = !DISubrange(count: 1)
!84 = !DILocalVariable(name: "s", scope: !66, file: !14, line: 26, type: !8)
!85 = !DILocalVariable(name: "i", scope: !86, file: !14, line: 27, type: !8)
!86 = distinct !DILexicalBlock(scope: !66, file: !14, line: 27, column: 5)
!87 = distinct !DIAssignID()
!88 = !DILocation(line: 0, scope: !66)
!89 = !DILocation(line: 24, column: 5, scope: !66)
!90 = !DILocation(line: 25, column: 5, scope: !66)
!91 = !DILocation(line: 0, scope: !86)
!92 = !DILocation(line: 27, column: 23, scope: !93, atomGroup: 10, atomRank: 1)
!93 = distinct !DILexicalBlock(scope: !86, file: !14, line: 27, column: 5)
!94 = !DILocation(line: 27, column: 5, scope: !86, atomGroup: 11, atomRank: 1)
!95 = !DILocation(line: 29, column: 5, scope: !66)
!96 = !DILocation(line: 31, column: 1, scope: !66)
!97 = !DILocation(line: 30, column: 5, scope: !66, atomGroup: 9, atomRank: 1)
!98 = !DILocation(line: 28, column: 14, scope: !93)
!99 = distinct !DIAssignID()
!100 = distinct !DIAssignID()
!101 = !{!63, !63, i64 0}
!102 = !DILocation(line: 28, column: 11, scope: !93, atomGroup: 5, atomRank: 2)
!103 = !DILocation(line: 27, column: 28, scope: !93, atomGroup: 6, atomRank: 2)
!104 = !DILocation(line: 27, column: 23, scope: !93, atomGroup: 3, atomRank: 1)
!105 = !DILocation(line: 27, column: 5, scope: !86, atomGroup: 4, atomRank: 1)
!106 = distinct !{!106, !107, !108, !109, !110}
!107 = !DILocation(line: 27, column: 5, scope: !86)
!108 = !DILocation(line: 28, column: 38, scope: !86)
!109 = !{!"llvm.loop.mustprogress"}
!110 = !{!"llvm.loop.unroll.disable"}
!111 = distinct !DISubprogram(name: "mix", scope: !14, file: !14, line: 33, type: !112, scopeLine: 33, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !118, keyInstructions: true)
!112 = !DISubroutineType(types: !113)
!113 = !{!114, !114, !114}
!114 = !DIDerivedType(tag: DW_TAG_typedef, name: "v4f", file: !14, line: 8, baseType: !115)
!115 = !DICompositeType(tag: DW_TAG_array_type, baseType: !6, size: 128, flags: DIFlagVector, elements: !116)
!116 = !{!117}
!117 = !DISubrange(count: 4)
!118 = !{!119, !120}
!119 = !DILocalVariable(name: "a", arg: 1, scope: !111, file: !14, line: 33, type: !114)
!120 = !DILocalVariable(name: "b", arg: 2, scope: !111, file: !14, line: 33, type: !114)
!121 = !DILocation(line: 0, scope: !111)
!122 = !DILocation(line: 34, column: 12, scope: !111)
!123 = !DILocation(line: 34, column: 56, scope: !111)
!124 = !DILocation(line: 34, column: 54, scope: !111, atomGroup: 1, atomRank: 2)
!125 = !DILocation(line: 34, column: 5, scope: !111, atomGroup: 1, atomRank: 1)
!126 = distinct !DISubprogram(name: "dispatch", scope: !14, file: !14, line: 37, type: !127, scopeLine: 37, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !129, keyInstructions: true)
!127 = !DISubroutineType(types: !128)
!128 = !{!8, !8, !8, !8}
!129 = !{!130, !131, !132}
!130 = !DILocalVariable(name: "op", arg: 1, scope: !126, file: !14, line: 37, type: !8)
!131 = !DILocalVariable(name: "a", arg: 2, scope: !126, file: !14, line: 37, type: !8)
!132 = !DILocalVariable(name: "b", arg: 3, scope: !126, file: !14, line: 37, type: !8)
!133 = !DILocation(line: 0, scope: !126)
!134 = !DILocation(line: 38, column: 5, scope: !126, atomGroup: 1, atomRank: 1)
!135 = !DILocation(line: 40, column: 18, scope: !136, atomGroup: 2, atomRank: 2)
!136 = distinct !DILexicalBlock(scope: !126, file: !14, line: 38, column: 17)
!137 = !DILocation(line: 40, column: 9, scope: !136, atomGroup: 2, atomRank: 1)
!138 = !DILocation(line: 42, column: 18, scope: !136, atomGroup: 3, atomRank: 2)
!139 = !DILocation(line: 42, column: 9, scope: !136, atomGroup: 3, atomRank: 1)
!140 = !DILocation(line: 44, column: 18, scope: !136, atomGroup: 4, atomRank: 2)
!141 = !DILocation(line: 44, column: 9, scope: !136, atomGroup: 4, atomRank: 1)
!142 = !DILocation(line: 46, column: 16, scope: !136, atomGroup: 6, atomRank: 2)
!143 = !DILocation(line: 46, column: 16, scope: !136, atomGroup: 6, atomRank: 1)
!144 = !DILocation(line: 46, column: 22, scope: !136)
!145 = !DILocation(line: 46, column: 16, scope: !136)
!146 = !DILocation(line: 48, column: 24, scope: !136)
!147 = !DILocation(line: 48, column: 18, scope: !136, atomGroup: 7, atomRank: 2)
!148 = !DILocation(line: 48, column: 9, scope: !136, atomGroup: 7, atomRank: 1)
!149 = !DILocation(line: 0, scope: !136)
!150 = !DILocation(line: 52, column: 1, scope: !126, atomGroup: 9, atomRank: 1)
!151 = !DILocation(line: 0, scope: !44)
!152 = !DILocation(line: 56, column: 20, scope: !44)
!153 = !DILocation(line: 56, column: 11, scope: !44)
!154 = !{!155, !155, i64 0}
!155 = !{!"any pointer", !64, i64 0}
!156 = !DILocation(line: 59, column: 1, scope: !44)
!157 = !DILocation(line: 60, column: 5, scope: !44, atomGroup: 3, atomRank: 1)
!158 = !DILocation(line: 61, column: 1, scope: !44)
!159 = !DILocation(line: 62, column: 5, scope: !44, atomGroup: 4, atomRank: 1)
!160 = !DILocation(line: 63, column: 1, scope: !44, atomGroup: 5, atomRank: 1)
!161 = distinct !DISubprogram(name: "atomics", scope: !14, file: !14, line: 65, type: !162, scopeLine: 65, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !164, keyInstructions: true)
!162 = !DISubroutineType(types: !163)
!163 = !{!8, !15}
!164 = !{!165, !166, !167}
!165 = !DILocalVariable(name: "p", arg: 1, scope: !161, file: !14, line: 65, type: !15)
!166 = !DILocalVariable(name: "old", scope: !161, file: !14, line: 66, type: !8)
!167 = !DILocalVariable(name: "expected", scope: !161, file: !14, line: 67, type: !8)
!168 = !DILocation(line: 0, scope: !161)
!169 = !DILocation(line: 66, column: 15, scope: !161, atomGroup: 2, atomRank: 2)
!170 = !DILocation(line: 68, column: 5, scope: !161)
!171 = !DILocation(line: 70, column: 5, scope: !161)
!172 = !DILocation(line: 71, column: 25, scope: !161, atomGroup: 6, atomRank: 2)
!173 = !DILocation(line: 71, column: 62, scope: !161, atomGroup: 5, atomRank: 2)
!174 = !DILocation(line: 71, column: 5, scope: !161, atomGroup: 5, atomRank: 1)
!175 = !DILocation(line: 73, column: 26, scope: !161)
!176 = !DILocation(line: 73, column: 29, scope: !161, atomGroup: 7, atomRank: 2)
!177 = !DILocation(line: 73, column: 5, scope: !161, atomGroup: 7, atomRank: 1)
!178 = distinct !DISubprogram(name: "convert", scope: !14, file: !14, line: 76, type: !179, scopeLine: 76, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !182, keyInstructions: true)
!179 = !DISubroutineType(types: !180)
!180 = !{!5, !181, !6, !9}
!181 = !DIBasicType(name: "long", size: 64, encoding: DW_ATE_signed)
!182 = !{!183, !184, !185}
!183 = !DILocalVariable(name: "x", arg: 1, scope: !178, file: !14, line: 76, type: !181)
!184 = !DILocalVariable(name: "f", arg: 2, scope: !178, file: !14, line: 76, type: !6)
!185 = !DILocalVariable(name: "c", arg: 3, scope: !178, file: !14, line: 76, type: !9)
!186 = !DILocation(line: 0, scope: !178)
!187 = !DILocation(line: 77, column: 12, scope: !178)
!188 = !DILocation(line: 77, column: 24, scope: !178)
!189 = !DILocation(line: 77, column: 22, scope: !178)
!190 = !DILocation(line: 77, column: 28, scope: !178)
!191 = !DILocation(line: 77, column: 26, scope: !178)
!192 = !DILocation(line: 77, column: 47, scope: !178)
!193 = !DILocation(line: 77, column: 40, scope: !178)
!194 = !DILocation(line: 77, column: 38, scope: !178, atomGroup: 1, atomRank: 2)
!195 = !DILocation(line: 77, column: 5, scope: !178, atomGroup: 1, atomRank: 1)
!196 = distinct !DISubprogram(name: "make_pair", scope: !14, file: !14, line: 80, type: !197, scopeLine: 80, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !203, keyInstructions: true)
!197 = !DISubroutineType(types: !198)
!198 = !{!199, !8, !181}
!199 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "pair", file: !14, line: 18, size: 128, elements: !200)
!200 = !{!201, !202}
!201 = !DIDerivedType(tag: DW_TAG_member, name: "a", scope: !199, file: !14, line: 19, baseType: !8, size: 32)
!202 = !DIDerivedType(tag: DW_TAG_member, name: "b", scope: !199, file: !14, line: 20, baseType: !181, size: 64, offset: 64)
!203 = !{!204, !205, !206}
!204 = !DILocalVariable(name: "a", arg: 1, scope: !196, file: !14, line: 80, type: !8)
!205 = !DILocalVariable(name: "b", arg: 2, scope: !196, file: !14, line: 80, type: !181)
!206 = !DILocalVariable(name: "p", scope: !196, file: !14, line: 81, type: !199)
!207 = !DILocation(line: 0, scope: !196)
!208 = !DILocation(line: 81, column: 27, scope: !196, atomGroup: 1, atomRank: 2)
!209 = !DILocation(line: 82, column: 5, scope: !196, atomGroup: 2, atomRank: 2)
!210 = !DILocation(line: 82, column: 5, scope: !196, atomGroup: 2, atomRank: 1)
!211 = distinct !DISubprogram(name: "minimum", scope: !14, file: !14, line: 85, type: !212, scopeLine: 85, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !214, keyInstructions: true)
!212 = !DISubroutineType(types: !213)
!213 = !{!8, !8, !8}
!214 = !{!215, !216}
!215 = !DILocalVariable(name: "a", arg: 1, scope: !211, file: !14, line: 85, type: !8)
!216 = !DILocalVariable(name: "b", arg: 2, scope: !211, file: !14, line: 85, type: !8)
!217 = !DILocation(line: 0, scope: !211)
!218 = !DILocation(line: 85, column: 62, scope: !211, atomGroup: 2, atomRank: 1)
!219 = !DILocation(line: 85, column: 55, scope: !211, atomGroup: 1, atomRank: 1)
!220 = distinct !DISubprogram(name: "bits", scope: !14, file: !14, line: 87, type: !221, scopeLine: 87, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !223, keyInstructions: true)
!221 = !DISubroutineType(types: !222)
!222 = !{!78, !78}
!223 = !{!224}
!224 = !DILocalVariable(name: "x", arg: 1, scope: !220, file: !14, line: 87, type: !78)
!225 = !DILocation(line: 0, scope: !220)
!226 = !DILocation(line: 88, column: 12, scope: !220)
!227 = !DILocation(line: 88, column: 52, scope: !220)
!228 = !DILocation(line: 88, column: 36, scope: !220)
!229 = !DILocation(line: 88, column: 34, scope: !220, atomGroup: 1, atomRank: 2)
!230 = !DILocation(line: 88, column: 5, scope: !220, atomGroup: 1, atomRank: 1)
!231 = distinct !DISubprogram(name: "asm_increment", scope: !14, file: !14, line: 91, type: !23, scopeLine: 91, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !232, keyInstructions: true)
!232 = !{!233, !234}
!233 = !DILocalVariable(name: "a", arg: 1, scope: !231, file: !14, line: 91, type: !8)
!234 = !DILocalVariable(name: "r", scope: !231, file: !14, line: 92, type: !8)
!235 = !DILocation(line: 0, scope: !231)
!236 = !DILocation(line: 93, column: 5, scope: !231, atomGroup: 1, atomRank: 1)
!237 = !{i64 2497}
!238 = !DILocation(line: 94, column: 5, scope: !231, atomGroup: 4, atomRank: 1)
!239 = distinct !DISubprogram(name: "negate", scope: !14, file: !14, line: 97, type: !240, scopeLine: 97, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !242, keyInstructions: true)
!240 = !DISubroutineType(types: !241)
!241 = !{!6, !6}
!242 = !{!243}
!243 = !DILocalVariable(name: "x", arg: 1, scope: !239, file: !14, line: 97, type: !6)
!244 = !DILocation(line: 0, scope: !239)
!245 = !DILocation(line: 97, column: 58, scope: !239, atomGroup: 1, atomRank: 2)
!246 = !DILocation(line: 97, column: 51, scope: !239, atomGroup: 1, atomRank: 1)
!247 = distinct !DISubprogram(name: "weak_function", scope: !14, file: !14, line: 99, type: !248, scopeLine: 99, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, keyInstructions: true)
!248 = !DISubroutineType(types: !249)
!249 = !{!8}
!250 = !DILocation(line: 99, column: 49, scope: !247, atomGroup: 1, atomRank: 1)
!251 = distinct !DISubprogram(name: "alias_target", scope: !14, file: !14, line: 103, type: !248, scopeLine: 103, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, keyInstructions: true)
!252 = !DILocation(line: 103, column: 33, scope: !251)
!253 = !DILocation(line: 103, column: 45, scope: !251)
!254 = !DILocation(line: 103, column: 43, scope: !251, atomGroup: 1, atomRank: 2)
!255 = !DILocation(line: 103, column: 26, scope: !251, atomGroup: 1, atomRank: 1)
!256 = distinct !DISubprogram(name: "helper", scope: !14, file: !14, line: 101, type: !23, scopeLine: 101, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagLocalToUnit | DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !257, keyInstructions: true)
!257 = !{!258}
!258 = !DILocalVariable(name: "x", arg: 1, scope: !256, file: !14, line: 101, type: !8)
!259 = !DILocation(line: 0, scope: !256)
!260 = !DILocation(line: 101, column: 63, scope: !256, atomGroup: 1, atomRank: 2)
!261 = !DILocation(line: 101, column: 54, scope: !256, atomGroup: 1, atomRank: 1)
!262 = distinct !DISubprogram(name: "kinds_main", scope: !14, file: !14, line: 112, type: !23, scopeLine: 112, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !263, keyInstructions: true)
!263 = !{!264, !265, !266, !267, !268}
!264 = !DILocalVariable(name: "argc", arg: 1, scope: !262, file: !14, line: 112, type: !8)
!265 = !DILocalVariable(name: "a", scope: !262, file: !14, line: 113, type: !114)
!266 = !DILocalVariable(name: "m", scope: !262, file: !14, line: 114, type: !114)
!267 = !DILocalVariable(name: "p", scope: !262, file: !14, line: 115, type: !199)
!268 = !DILocalVariable(name: "total", scope: !262, file: !14, line: 116, type: !8)
!269 = !DILocation(line: 0, scope: !262)
!270 = !DILocation(line: 114, column: 13, scope: !262, atomGroup: 2, atomRank: 2)
!271 = !DILocation(line: 115, column: 37, scope: !262)
!272 = !DILocation(line: 115, column: 21, scope: !262)
!273 = !DILocation(line: 115, column: 21, scope: !262, atomGroup: 3, atomRank: 2)
!274 = !DILocation(line: 116, column: 17, scope: !262)
!275 = !DILocation(line: 116, column: 41, scope: !262)
!276 = !DILocation(line: 117, column: 17, scope: !262)
!277 = !DILocation(line: 117, column: 61, scope: !262)
!278 = !DILocation(line: 117, column: 43, scope: !262)
!279 = !DILocation(line: 117, column: 34, scope: !262)
!280 = !DILocation(line: 118, column: 36, scope: !262)
!281 = !DILocation(line: 118, column: 42, scope: !262)
!282 = !DILocation(line: 118, column: 22, scope: !262)
!283 = !DILocation(line: 118, column: 17, scope: !262)
!284 = !DILocation(line: 119, column: 17, scope: !262)
!285 = !DILocation(line: 119, column: 28, scope: !262)
!286 = !DILocation(line: 119, column: 52, scope: !262)
!287 = !DILocation(line: 120, column: 29, scope: !262)
!288 = !DILocation(line: 120, column: 22, scope: !262)
!289 = !DILocation(line: 120, column: 17, scope: !262)
!290 = !DILocation(line: 120, column: 44, scope: !262)
!291 = !DILocation(line: 103, column: 33, scope: !251, inlinedAt: !292)
!292 = distinct !DILocation(line: 120, column: 62, scope: !262)
!293 = !DILocation(line: 103, column: 45, scope: !251, inlinedAt: !292)
!294 = !DILocation(line: 121, column: 52, scope: !262)
!295 = !DILocation(line: 121, column: 36, scope: !262)
!296 = !DILocation(line: 122, column: 17, scope: !262)
!297 = !DILocation(line: 122, column: 34, scope: !262)
!298 = !DILocation(line: 122, column: 49, scope: !262)
!299 = !{!300, !300, i64 0}
!300 = !{!"p1 int", !155, i64 0}
!301 = !DILocation(line: 122, column: 48, scope: !262)
!302 = !DILocation(line: 122, column: 66, scope: !262)
!303 = !DILocation(line: 123, column: 30, scope: !262)
!304 = !DILocation(line: 123, column: 17, scope: !262)
!305 = !{!64, !64, i64 0}
!306 = !DILocation(line: 116, column: 39, scope: !262)
!307 = !DILocation(line: 116, column: 62, scope: !262)
!308 = !DILocation(line: 117, column: 32, scope: !262)
!309 = !DILocation(line: 117, column: 67, scope: !262)
!310 = !DILocation(line: 118, column: 63, scope: !262)
!311 = !DILocation(line: 118, column: 69, scope: !262)
!312 = !DILocation(line: 119, column: 26, scope: !262)
!313 = !DILocation(line: 119, column: 45, scope: !262)
!314 = !DILocation(line: 119, column: 63, scope: !262)
!315 = !DILocation(line: 120, column: 42, scope: !262)
!316 = !DILocation(line: 103, column: 43, scope: !251, inlinedAt: !292, atomGroup: 1, atomRank: 2)
!317 = !DILocation(line: 120, column: 60, scope: !262)
!318 = !DILocation(line: 120, column: 79, scope: !262)
!319 = !DILocation(line: 121, column: 34, scope: !262)
!320 = !DILocation(line: 121, column: 63, scope: !262)
!321 = !DILocation(line: 122, column: 32, scope: !262)
!322 = !DILocation(line: 122, column: 46, scope: !262)
!323 = !DILocation(line: 122, column: 64, scope: !262)
!324 = !DILocation(line: 122, column: 77, scope: !262, atomGroup: 4, atomRank: 2)
!325 = !DILocation(line: 124, column: 5, scope: !262, atomGroup: 6, atomRank: 1)
!326 = distinct !DISubprogram(name: "resolve", scope: !14, file: !14, line: 107, type: !327, scopeLine: 107, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagLocalToUnit | DISPFlagDefinition | DISPFlagOptimized, unit: !2, keyInstructions: true)
!327 = !DISubroutineType(types: !328)
!328 = !{!10}
!329 = !DILocation(line: 107, column: 30, scope: !326, atomGroup: 1, atomRank: 1)
!330 = distinct !DISubprogram(name: "implementation", scope: !14, file: !14, line: 106, type: !248, scopeLine: 106, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagLocalToUnit | DISPFlagDefinition | DISPFlagOptimized, unit: !2, keyInstructions: true)
!331 = !DILocation(line: 106, column: 35, scope: !330, atomGroup: 1, atomRank: 1)
