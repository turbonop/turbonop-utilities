; ModuleID = 'kinds-thin.bc'
source_filename = "R:\\tools\\bitcode\\kinds.c"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

%struct.__va_list_tag = type { i32, i32, ptr, ptr }

@global_array = dso_local global <{ i32, i32, i32, [13 x i32] }> <{ i32 1, i32 2, i32 3, [13 x i32] zeroinitializer }>, align 16, !guid !0
@ptr_into_array = dso_local local_unnamed_addr global ptr getelementptr inbounds nuw (i8, ptr @global_array, i64 12), align 8, !guid !1
@in_section = dso_local local_unnamed_addr global i32 5, section ".mydata", align 4, !guid !2
@computed.labels = internal unnamed_addr constant [3 x ptr] [ptr blockaddress(@computed, %8), ptr blockaddress(@computed, %6), ptr blockaddress(@computed, %7)], align 16, !guid !3
@atomic_counter = dso_local global i32 0, align 4, !guid !4
@operations = dso_local local_unnamed_addr constant [2 x ptr] [ptr @helper, ptr @asm_increment], align 16, !guid !5
@external_value = external local_unnamed_addr global i32, align 4
@tls_counter = dso_local thread_local local_unnamed_addr global i32 0, align 4, !guid !6
@message = internal unnamed_addr constant [22 x i8] c"kinds of instructions\00", align 16, !guid !7

@alias_function = dso_local alias i32 (), ptr @alias_target

@ifunc_function = dso_local ifunc i32 (), ptr @resolve

; Function Attrs: nofree noinline norecurse nosync nounwind uwtable
define dso_local i32 @sum_va(i32 noundef %0, ...) local_unnamed_addr #0 !guid !19 {
  %2 = alloca [1 x %struct.__va_list_tag], align 16
  call void @llvm.lifetime.start.p0(ptr nonnull %2)
  call void @llvm.va_start.p0(ptr nonnull %2)
  %3 = icmp sgt i32 %0, 0
  br i1 %3, label %4, label %10

4:                                                ; preds = %1
  %5 = load i32, ptr %2, align 1
  %6 = getelementptr inbounds nuw i8, ptr %2, i64 8
  %7 = getelementptr inbounds nuw i8, ptr %2, i64 16
  %8 = load ptr, ptr %7, align 16
  %9 = load ptr, ptr %6, align 8
  br label %12

10:                                               ; preds = %24, %1
  %11 = phi i32 [ 0, %1 ], [ %29, %24 ]
  call void @llvm.va_end.p0(ptr nonnull %2)
  call void @llvm.lifetime.end.p0(ptr nonnull %2)
  ret i32 %11

12:                                               ; preds = %24, %4
  %13 = phi ptr [ %9, %4 ], [ %25, %24 ]
  %14 = phi i32 [ 0, %4 ], [ %30, %24 ]
  %15 = phi i32 [ 0, %4 ], [ %29, %24 ]
  %16 = phi i32 [ %5, %4 ], [ %26, %24 ]
  %17 = icmp ult i32 %16, 41
  br i1 %17, label %18, label %22

18:                                               ; preds = %12
  %19 = zext nneg i32 %16 to i64
  %20 = getelementptr i8, ptr %8, i64 %19
  %21 = add nuw nsw i32 %16, 8
  store i32 %21, ptr %2, align 16
  br label %24

22:                                               ; preds = %12
  %23 = getelementptr i8, ptr %13, i64 8
  store ptr %23, ptr %6, align 8
  br label %24

24:                                               ; preds = %22, %18
  %25 = phi ptr [ %13, %18 ], [ %23, %22 ]
  %26 = phi i32 [ %21, %18 ], [ %16, %22 ]
  %27 = phi ptr [ %20, %18 ], [ %13, %22 ]
  %28 = load i32, ptr %27, align 4, !tbaa !20
  %29 = add nsw i32 %28, %15
  %30 = add nuw nsw i32 %14, 1
  %31 = icmp eq i32 %30, %0
  br i1 %31, label %10, label %12, !llvm.loop !21
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
define dso_local <4 x float> @mix(<4 x float> noundef %0, <4 x float> noundef %1) local_unnamed_addr #3 !guid !23 {
  %3 = shufflevector <4 x float> %0, <4 x float> %1, <4 x i32> <i32 0, i32 5, i32 2, i32 7>
  %4 = shufflevector <4 x float> %0, <4 x float> poison, <4 x i32> <i32 1, i32 1, i32 1, i32 1>
  %5 = fmul <4 x float> %4, %3
  ret <4 x float> %5
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local i32 @dispatch(i32 noundef %0, i32 noundef %1, i32 noundef %2) local_unnamed_addr #4 !guid !24 {
  switch i32 %0, label %17 [
    i32 0, label %4
    i32 1, label %6
    i32 2, label %8
    i32 3, label %10
    i32 7, label %14
  ]

4:                                                ; preds = %3
  %5 = add nsw i32 %2, %1
  br label %17

6:                                                ; preds = %3
  %7 = sub nsw i32 %1, %2
  br label %17

8:                                                ; preds = %3
  %9 = mul nsw i32 %2, %1
  br label %17

10:                                               ; preds = %3
  %11 = icmp eq i32 %2, 0
  br i1 %11, label %17, label %12

12:                                               ; preds = %10
  %13 = sdiv i32 %1, %2
  br label %17

14:                                               ; preds = %3
  %15 = and i32 %2, 31
  %16 = shl i32 %1, %15
  br label %17

17:                                               ; preds = %14, %12, %10, %8, %6, %4, %3
  %18 = phi i32 [ 0, %10 ], [ %5, %4 ], [ %7, %6 ], [ %9, %8 ], [ %16, %14 ], [ %13, %12 ], [ -1, %3 ]
  ret i32 %18
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local range(i32 10, 31) i32 @computed(i32 noundef %0) local_unnamed_addr #4 !guid !25 {
  %2 = srem i32 %0, 3
  %3 = sext i32 %2 to i64
  %4 = getelementptr inbounds [8 x i8], ptr @computed.labels, i64 %3
  %5 = load ptr, ptr %4, align 8, !tbaa !26
  indirectbr ptr %5, [label %8, label %6, label %7]

6:                                                ; preds = %1
  br label %8

7:                                                ; preds = %1
  br label %8

8:                                                ; preds = %7, %6, %1
  %9 = phi i32 [ 30, %7 ], [ 20, %6 ], [ 10, %1 ]
  ret i32 %9
}

; Function Attrs: mustprogress noinline norecurse nounwind willreturn uwtable
define dso_local i32 @atomics(ptr nofree noundef captures(none) %0) local_unnamed_addr #5 !guid !28 {
  %2 = atomicrmw add ptr %0, i32 2 seq_cst, align 4
  %3 = cmpxchg ptr %0, i32 %2, i32 7 acq_rel acquire, align 4
  %4 = extractvalue { i32, i1 } %3, 0
  fence seq_cst
  %5 = load atomic i32, ptr %0 acquire, align 4
  %6 = add nsw i32 %5, 1
  store atomic i32 %6, ptr %0 release, align 4
  %7 = atomicrmw add ptr @atomic_counter, i32 1 seq_cst, align 4
  %8 = add nsw i32 %7, %4
  ret i32 %8
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local noundef double @convert(i64 noundef %0, float noundef %1, i8 noundef zeroext %2) local_unnamed_addr #4 !guid !29 {
  %4 = sitofp i64 %0 to double
  %5 = fpext float %1 to double
  %6 = fadd double %4, %5
  %7 = uitofp i8 %2 to double
  %8 = fadd double %6, %7
  %9 = trunc i64 %0 to i16
  %10 = sitofp i16 %9 to double
  %11 = fadd double %8, %10
  ret double %11
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local { i32, i64 } @make_pair(i32 noundef %0, i64 noundef %1) local_unnamed_addr #4 !guid !30 {
  %3 = shl nsw i64 %1, 1
  %4 = insertvalue { i32, i64 } poison, i32 %0, 0
  %5 = insertvalue { i32, i64 } %4, i64 %3, 1
  ret { i32, i64 } %5
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local noundef i32 @minimum(i32 noundef %0, i32 noundef %1) local_unnamed_addr #4 !guid !31 {
  %3 = tail call i32 @llvm.smin.i32(i32 %0, i32 %1)
  ret i32 %3
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local range(i32 0, 64) i32 @bits(i32 noundef %0) local_unnamed_addr #4 !guid !32 {
  %2 = tail call range(i32 0, 33) i32 @llvm.ctpop.i32(i32 %0)
  %3 = or i32 %0, 1
  %4 = tail call range(i32 0, 32) i32 @llvm.ctlz.i32(i32 %3, i1 true)
  %5 = add nuw nsw i32 %4, %2
  ret i32 %5
}

; Function Attrs: nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare i32 @llvm.ctpop.i32(i32) #6

; Function Attrs: nocallback nofree nosync nounwind speculatable willreturn memory(none)
declare i32 @llvm.ctlz.i32(i32, i1 immarg) #7

; Function Attrs: nofree noinline nosync nounwind memory(none) uwtable
define dso_local i32 @asm_increment(i32 noundef %0) #8 !guid !33 {
  %2 = tail call i32 asm "leal 1($1), $0", "=r,r,~{dirflag},~{fpsr},~{flags}"(i32 %0) #13, !srcloc !34
  ret i32 %2
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local noundef float @negate(float noundef %0) local_unnamed_addr #4 !guid !35 {
  %2 = fneg float %0
  ret float %2
}

; Function Attrs: nounwind uwtable
define weak dso_local i32 @weak_function() local_unnamed_addr #9 !guid !36 {
  ret i32 7
}

; Function Attrs: mustprogress nofree norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local i32 @alias_target() #10 !guid !37 {
  %1 = tail call i32 @helper(i32 noundef 1)
  %2 = tail call i32 @helper(i32 noundef 2)
  %3 = add nsw i32 %2, %1
  ret i32 %3
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind willreturn memory(none) uwtable
define internal i32 @helper(i32 noundef %0) #4 !guid !38 {
  %2 = mul nsw i32 %0, 3
  ret i32 %2
}

; Function Attrs: nounwind uwtable
define dso_local i32 @kinds_main(i32 noundef %0) local_unnamed_addr #11 !guid !39 {
  %2 = tail call i32 (i32, ...) @sum_va(i32 noundef 3, i32 noundef %0, i32 noundef 2, i32 noundef 3)
  %3 = and i32 %0, 7
  %4 = zext nneg i32 %3 to i64
  %5 = getelementptr inbounds nuw [4 x i8], ptr @global_array, i64 %4
  %6 = tail call i32 @atomics(ptr noundef nonnull %5)
  %7 = tail call i32 @weak_function()
  %8 = and i32 %0, 1
  %9 = icmp eq i32 %8, 0
  br i1 %9, label %10, label %12

10:                                               ; preds = %1
  %11 = tail call i32 @helper(i32 noundef %0) #14
  br label %14

12:                                               ; preds = %1
  %13 = tail call i32 @asm_increment(i32 noundef %0) #14
  br label %14

14:                                               ; preds = %12, %10
  %15 = phi i32 [ %11, %10 ], [ %13, %12 ]
  %16 = tail call i32 @dispatch(i32 noundef %0, i32 noundef 4, i32 noundef 5)
  %17 = tail call i32 @computed(i32 noundef %0)
  %18 = sext i32 %0 to i64
  %19 = tail call <4 x float> @mix(<4 x float> noundef <float 1.000000e+00, float 2.000000e+00, float 3.000000e+00, float 4.000000e+00>, <4 x float> noundef <float 1.000000e+00, float 2.000000e+00, float 3.000000e+00, float 4.000000e+00>)
  %20 = extractelement <4 x float> %19, i64 2
  %21 = trunc i32 %0 to i8
  %22 = tail call double @convert(i64 noundef %18, float noundef %20, i8 noundef zeroext %21)
  %23 = fptosi double %22 to i32
  %24 = tail call { i32, i64 } @make_pair(i32 noundef %0, i64 noundef %18)
  %25 = extractvalue { i32, i64 } %24, 0
  %26 = extractvalue { i32, i64 } %24, 1
  %27 = trunc i64 %26 to i32
  %28 = tail call i32 @minimum(i32 noundef %0, i32 noundef 9)
  %29 = tail call i32 @bits(i32 noundef %0)
  %30 = sitofp i32 %0 to float
  %31 = tail call float @negate(float noundef %30)
  %32 = fptosi float %31 to i32
  %33 = tail call i32 @helper(i32 noundef 2)
  %34 = tail call i32 @helper(i32 noundef 1)
  %35 = load i32, ptr @external_value, align 4, !tbaa !20
  %36 = tail call align 4 ptr @llvm.threadlocal.address.p0(ptr align 4 @tls_counter)
  %37 = load i32, ptr %36, align 4, !tbaa !20
  %38 = load ptr, ptr @ptr_into_array, align 8, !tbaa !40
  %39 = load i32, ptr %38, align 4, !tbaa !20
  %40 = load i32, ptr @in_section, align 4, !tbaa !20
  %41 = and i32 %0, 15
  %42 = zext nneg i32 %41 to i64
  %43 = getelementptr inbounds nuw i8, ptr @message, i64 %42
  %44 = load i8, ptr %43, align 1, !tbaa !42
  %45 = sext i8 %44 to i32
  %46 = add i32 %2, 2
  %47 = add i32 %46, %6
  %48 = add i32 %47, %7
  %49 = add i32 %48, %15
  %50 = add i32 %49, %16
  %51 = add i32 %50, %17
  %52 = add i32 %51, %23
  %53 = add i32 %52, %25
  %54 = add i32 %53, %28
  %55 = add i32 %54, %27
  %56 = add i32 %55, %29
  %57 = add i32 %56, %33
  %58 = add i32 %57, %32
  %59 = add i32 %58, %34
  %60 = add nsw i32 %59, %35
  %61 = add nsw i32 %60, %37
  %62 = add nsw i32 %61, %39
  %63 = add nsw i32 %62, %40
  %64 = add nsw i32 %63, %45
  ret i32 %64
}

; Function Attrs: nocallback nofree nosync nounwind speculatable willreturn memory(none)
declare nonnull ptr @llvm.threadlocal.address.p0(ptr nonnull) #7

; Function Attrs: disable_sanitizer_instrumentation nounwind uwtable
define internal ptr @resolve() #12 !guid !43 {
  ret ptr @implementation
}

; Function Attrs: mustprogress nofree norecurse nosync nounwind willreturn memory(none) uwtable
define internal noundef i32 @implementation() #10 !guid !44 {
  ret i32 2
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

!llvm.module.flags = !{!8, !9, !10, !11, !12}
!llvm.ident = !{!13}
!llvm.errno.tbaa = !{!14}

!0 = !{i64 -6472432288142568936}
!1 = !{i64 -4985384248457520330}
!2 = !{i64 8695581679798269916}
!3 = !{i64 -1023910730309115511}
!4 = !{i64 -2824719618367536262}
!5 = !{i64 -8471369405859620422}
!6 = !{i64 -85048640574315108}
!7 = !{i64 -5745644789867322683}
!8 = !{i32 2, !"debugger-tuning", !"gdb"}
!9 = !{i32 8, !"PIC Level", i32 2}
!10 = !{i32 7, !"PIE Level", i32 2}
!11 = !{i32 7, !"uwtable", i32 2}
!12 = !{i32 1, !"EnableSplitLTOUnit", i32 0}
!13 = !{!"clang version 24.0.0git (https://github.com/llvm/llvm-project.git c74b358f44c31e3615fe942556a41be8c9a8d877)"}
!14 = !{!15, !16, i64 0}
!15 = !{!"__libc_errno", !16, i64 0}
!16 = !{!"int", !17, i64 0}
!17 = !{!"omnipotent char", !18, i64 0}
!18 = !{!"Simple C/C++ TBAA"}
!19 = !{i64 -2136316013513105041}
!20 = !{!16, !16, i64 0}
!21 = distinct !{!21, !22}
!22 = !{!"llvm.loop.mustprogress"}
!23 = !{i64 3726853560884672730}
!24 = !{i64 -7762558725055652451}
!25 = !{i64 -674704507510378150}
!26 = !{!27, !27, i64 0}
!27 = !{!"any pointer", !17, i64 0}
!28 = !{i64 -2188192330089480378}
!29 = !{i64 3802914799201949233}
!30 = !{i64 1248870633037018039}
!31 = !{i64 -618017291214027805}
!32 = !{i64 5912776693900001740}
!33 = !{i64 -6545908829158019977}
!34 = !{i64 2497}
!35 = !{i64 -4076287070191087315}
!36 = !{i64 3605216326506308165}
!37 = !{i64 3459219637947183356}
!38 = !{i64 -169029177834163332}
!39 = !{i64 -4924455692130967663}
!40 = !{!41, !41, i64 0}
!41 = !{!"p1 int", !27, i64 0}
!42 = !{!17, !17, i64 0}
!43 = !{i64 -8904132962228568734}
!44 = !{i64 -6325313639286763892}

^0 = module: (path: "kinds-thin.bc", hash: (1954620022, 2926191473, 4126710086, 801775324, 460362818))
^1 = gv: (name: "llvm.smin.i32") ; guid = 47936390007264708
^2 = gv: (name: "llvm.va_start.p0") ; guid = 1116014281982105379
^3 = gv: (name: "make_pair", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 4, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 1248870633037018039
^4 = gv: (name: "alias_target", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 4, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 0, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0), calls: ((callee: ^34, tail: 1))))) ; guid = 3459219637947183356
^5 = gv: (name: "weak_function", summaries: (function: (module: ^0, flags: (linkage: weak, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 1, funcFlags: (readNone: 0, readOnly: 0, noRecurse: 0, returnDoesNotAlias: 0, noInline: 0, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 3605216326506308165
^6 = gv: (name: "mix", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 4, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 3726853560884672730
^7 = gv: (name: "convert", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 9, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 3802914799201949233
^8 = gv: (name: "llvm.lifetime.start.p0") ; guid = 4736283418132453162
^9 = gv: (name: "llvm.va_end.p0") ; guid = 5090975557526790353
^10 = gv: (name: "bits", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 5, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 5912776693900001740
^11 = gv: (name: "alias_function", summaries: (alias: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), aliasee: ^4))) ; guid = 7689840652111387103
^12 = gv: (name: "llvm.ctlz.i32") ; guid = 8421475089036034803
^13 = gv: (name: "in_section", summaries: (variable: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), varFlags: (readonly: 1, writeonly: 1, constant: 0)))) ; guid = 8695581679798269916
^14 = gv: (name: "llvm.threadlocal.address.p0") ; guid = 8720525458955629832
^15 = gv: (name: "resolve", summaries: (function: (module: ^0, flags: (linkage: internal, visibility: default, notEligibleToImport: 0, live: 1, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 1, funcFlags: (readNone: 0, readOnly: 0, noRecurse: 0, returnDoesNotAlias: 0, noInline: 0, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0), refs: (^21)))) ; guid = 9542611111480982882
^16 = gv: (name: "operations", summaries: (variable: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), varFlags: (readonly: 1, writeonly: 0, constant: 1), refs: (^19, ^34)))) ; guid = 9975374667849931194
^17 = gv: (name: "dispatch", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 16, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 10684185348653899165
^18 = gv: (name: "llvm.ctpop.i32") ; guid = 10956606204485869642
^19 = gv: (name: "asm_increment", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 2, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 0, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 1, mustBeUnreachable: 0)))) ; guid = 11900835244551531639
^20 = gv: (name: "global_array", summaries: (variable: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), varFlags: (readonly: 1, writeonly: 1, constant: 0)))) ; guid = 11974311785566982680
^21 = gv: (name: "implementation", summaries: (function: (module: ^0, flags: (linkage: internal, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 1, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 0, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 12121430434422787724
^22 = gv: (name: "message", summaries: (variable: (module: ^0, flags: (linkage: internal, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), varFlags: (readonly: 1, writeonly: 0, constant: 1)))) ; guid = 12701099283842228933
^23 = gv: (name: "llvm.lifetime.end.p0") ; guid = 13244677477227836793
^24 = gv: (name: "ptr_into_array", summaries: (variable: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), varFlags: (readonly: 1, writeonly: 1, constant: 0), refs: (^20)))) ; guid = 13461359825252031286
^25 = gv: (name: "kinds_main", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 64, funcFlags: (readNone: 0, readOnly: 0, noRecurse: 0, returnDoesNotAlias: 0, noInline: 0, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0), calls: ((callee: ^29, tail: 1), (callee: ^28, tail: 1), (callee: ^5, tail: 1), (callee: ^34, tail: 1), (callee: ^19, tail: 1), (callee: ^17, tail: 1), (callee: ^32, tail: 1), (callee: ^6, tail: 1), (callee: ^7, tail: 1), (callee: ^3, tail: 1), (callee: ^33, tail: 1), (callee: ^10, tail: 1), (callee: ^26, tail: 1)), refs: (^20, ^35, ^22, readonly ^30, readonly ^24, readonly ^13)))) ; guid = 13522288381578583953
^26 = gv: (name: "negate", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 2, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 14370457003518464301
^27 = gv: (name: "atomic_counter", summaries: (variable: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), varFlags: (readonly: 1, writeonly: 1, constant: 0)))) ; guid = 15622024455342015354
^28 = gv: (name: "atomics", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 10, funcFlags: (readNone: 0, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0), refs: (^27)))) ; guid = 16258551743620071238
^29 = gv: (name: "sum_va", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 37, funcFlags: (readNone: 0, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 16310428060196446575
^30 = gv: (name: "external_value") ; guid = 16518904081887295106
^31 = gv: (name: "computed.labels", summaries: (variable: (module: ^0, flags: (linkage: internal, visibility: default, notEligibleToImport: 1, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), varFlags: (readonly: 1, writeonly: 0, constant: 1)))) ; guid = 17422833343400436105
^32 = gv: (name: "computed", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 1, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 9, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0), refs: (^31)))) ; guid = 17772039566199173466
^33 = gv: (name: "minimum", summaries: (function: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 2, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 17828726782495523811
^34 = gv: (name: "helper", summaries: (function: (module: ^0, flags: (linkage: internal, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), insts: 2, funcFlags: (readNone: 1, readOnly: 0, noRecurse: 1, returnDoesNotAlias: 0, noInline: 1, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0)))) ; guid = 18277714895875388284
^35 = gv: (name: "tls_counter", summaries: (variable: (module: ^0, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 0, dsoLocal: 1, canAutoHide: 0, importType: definition, noRenameOnPromotion: 0), varFlags: (readonly: 1, writeonly: 1, constant: 0)))) ; guid = 18361695433135236508
^36 = blockcount: 0
