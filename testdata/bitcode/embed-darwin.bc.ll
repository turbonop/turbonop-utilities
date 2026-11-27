; ModuleID = 'embed-darwin.bc'
source_filename = "R:\\tools\\bitcode\\embed.c"
target datalayout = "e-m:o-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-apple-macosx11.0.0"

@embedded_counter = local_unnamed_addr global i32 1, align 4, !guid !0
@embedded_name = internal unnamed_addr constant [9 x i8] c"embedded\00", align 1, !guid !1

; Function Attrs: mustprogress nofree norecurse nosync nounwind ssp willreturn memory(readwrite, argmem: none, inaccessiblemem: none, target_mem: none) uwtable
define i32 @embedded_entry(i32 noundef %0) local_unnamed_addr #0 !guid !12 {
  %2 = tail call fastcc i32 @embedded_helper(i32 noundef %0)
  %3 = load i32, ptr @embedded_counter, align 4, !tbaa !13
  %4 = add nsw i32 %3, %2
  store i32 %4, ptr @embedded_counter, align 4, !tbaa !13
  ret i32 %4
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind ssp willreturn memory(none) uwtable
define internal fastcc i32 @embedded_helper(i32 noundef %0) unnamed_addr #1 !guid !14 {
  %2 = mul nsw i32 %0, 5
  %3 = and i32 %0, 7
  %4 = zext nneg i32 %3 to i64
  %5 = getelementptr inbounds nuw i8, ptr @embedded_name, i64 %4
  %6 = load i8, ptr %5, align 1, !tbaa !15
  %7 = sext i8 %6 to i32
  %8 = add nsw i32 %2, %7
  ret i32 %8
}

attributes #0 = { mustprogress nofree norecurse nosync nounwind ssp willreturn memory(readwrite, argmem: none, inaccessiblemem: none, target_mem: none) uwtable "frame-pointer"="all" "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="penryn" "target-features"="+cmov,+cx16,+cx8,+fxsr,+mmx,+sahf,+sse,+sse2,+sse3,+sse4.1,+ssse3,+x87" "tune-cpu"="generic" }
attributes #1 = { mustprogress nofree noinline norecurse nosync nounwind ssp willreturn memory(none) uwtable "frame-pointer"="all" "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="penryn" "target-features"="+cmov,+cx16,+cx8,+fxsr,+mmx,+sahf,+sse,+sse2,+sse3,+sse4.1,+ssse3,+x87" "tune-cpu"="generic" }

!llvm.module.flags = !{!2, !3, !4, !5}
!llvm.ident = !{!6}
!llvm.errno.tbaa = !{!7}

!0 = !{i64 -3225532396765872866}
!1 = !{i64 3937707057232105678}
!2 = !{i32 2, !"debugger-tuning", !"lldb"}
!3 = !{i32 8, !"PIC Level", i32 2}
!4 = !{i32 7, !"uwtable", i32 2}
!5 = !{i32 7, !"frame-pointer", i32 2}
!6 = !{!"clang version 24.0.0git (https://github.com/llvm/llvm-project.git c74b358f44c31e3615fe942556a41be8c9a8d877)"}
!7 = !{!8, !9, i64 0}
!8 = !{!"__libc_errno", !9, i64 0}
!9 = !{!"int", !10, i64 0}
!10 = !{!"omnipotent char", !11, i64 0}
!11 = !{!"Simple C/C++ TBAA"}
!12 = !{i64 4878288566385211576}
!13 = !{!9, !9, i64 0}
!14 = !{i64 -927682466668206962}
!15 = !{!10, !10, i64 0}
