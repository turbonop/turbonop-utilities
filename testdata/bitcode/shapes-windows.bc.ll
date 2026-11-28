; ModuleID = 'shapes-windows.bc'
source_filename = "R:\\tools\\bitcode\\shapes.cpp"
target datalayout = "e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-windows-msvc19.52.37029"

%rtti.TypeDescriptor13 = type { ptr, ptr, [14 x i8] }
%eh.CatchableType = type { i32, i32, i32, i32, i32, i32, i32 }
%eh.CatchableTypeArray.1 = type { i32, [1 x i32] }
%eh.ThrowInfo = type { i32, i32, i32, i32 }
%struct.Circle = type { %struct.Shape, double }
%struct.Shape = type { ptr }
%rtti.CompleteObjectLocator = type { i32, i32, i32, i32, i32, i32 }
%rtti.TypeDescriptor12 = type { ptr, ptr, [13 x i8] }
%rtti.ClassHierarchyDescriptor = type { i32, i32, i32, i32 }
%rtti.BaseClassDescriptor = type { i32, i32, i32, i32, i32, i32, i32 }
%rtti.TypeDescriptor11 = type { ptr, ptr, [12 x i8] }
%struct.Failure = type { i32 }
%struct.Square = type { %struct.Shape, double }

$"?area@Square@@UEBANXZ" = comdat any

$"?name@Square@@UEBAPEBDXZ" = comdat any

$"?name@Shape@@UEBAPEBDXZ" = comdat any

$"??_GShape@@UEAAPEAXI@Z" = comdat any

$"??_GSquare@@UEAAPEAXI@Z" = comdat any

$"?area@Circle@@UEBANXZ" = comdat any

$"??_GCircle@@UEAAPEAXI@Z" = comdat any

$"??_R0?AUFailure@@@8" = comdat any

$"_CT??_R0?AUFailure@@@84" = comdat any

$"_CTA1?AUFailure@@" = comdat any

$"_TI1?AUFailure@@" = comdat any

$"??_7Square@@6B@" = comdat largest

$"??_R4Square@@6B@" = comdat any

$"??_R0?AUSquare@@@8" = comdat any

$"??_R3Square@@8" = comdat any

$"??_R2Square@@8" = comdat any

$"??_R1A@?0A@EA@Square@@8" = comdat any

$"??_R1A@?0A@EA@Shape@@8" = comdat any

$"??_R0?AUShape@@@8" = comdat any

$"??_R3Shape@@8" = comdat any

$"??_R2Shape@@8" = comdat any

$"??_7Shape@@6B@" = comdat largest

$"??_R4Shape@@6B@" = comdat any

$"??_C@_05KFCIHKGL@shape?$AA@" = comdat any

$"??_7Circle@@6B@" = comdat largest

$"??_R4Circle@@6B@" = comdat any

$"??_R0?AUCircle@@@8" = comdat any

$"??_R3Circle@@8" = comdat any

$"??_R2Circle@@8" = comdat any

$"??_R1A@?0A@EA@Circle@@8" = comdat any

$"??_C@_06ICDKOIFG@square?$AA@" = comdat any

@"?destroyed@Square@@2HA" = dso_local local_unnamed_addr global i32 0, align 4, !dbg !0
@"?counter@@3HA" = dso_local local_unnamed_addr global i32 0, align 4, !dbg !11
@"??_7type_info@@6B@" = external constant ptr
@"??_R0?AUFailure@@@8" = linkonce_odr global %rtti.TypeDescriptor13 { ptr @"??_7type_info@@6B@", ptr null, [14 x i8] c".?AUFailure@@\00" }, comdat
@__ImageBase = external dso_local constant i8
@"_CT??_R0?AUFailure@@@84" = linkonce_odr unnamed_addr constant %eh.CatchableType { i32 0, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R0?AUFailure@@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 0, i32 -1, i32 0, i32 4, i32 0 }, section ".xdata", comdat
@"_CTA1?AUFailure@@" = linkonce_odr unnamed_addr constant %eh.CatchableTypeArray.1 { i32 1, [1 x i32] [i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"_CT??_R0?AUFailure@@@84" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32)] }, section ".xdata", comdat
@"_TI1?AUFailure@@" = linkonce_odr unnamed_addr constant %eh.ThrowInfo { i32 0, i32 0, i32 0, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"_CTA1?AUFailure@@" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, section ".xdata", comdat
@unit = internal global %struct.Circle { %struct.Shape { ptr @"??_7Circle@@6B@" }, double 1.000000e+00 }, align 8, !dbg !13
@table = internal unnamed_addr constant [2 x ptr] [ptr @"?guarded@@YAHH@Z", ptr @"?twice@detail@@YAHH@Z"], align 8, !dbg !54
@0 = private constant { [4 x ptr] } { [4 x ptr] [ptr @"??_R4Square@@6B@", ptr @"??_ESquare@@UEAAPEAXI@Z", ptr @"?area@Square@@UEBANXZ", ptr @"?name@Square@@UEBAPEBDXZ"] }, comdat($"??_7Square@@6B@")
@"??_R4Square@@6B@" = linkonce_odr constant %rtti.CompleteObjectLocator { i32 1, i32 0, i32 0, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R0?AUSquare@@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R3Square@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R4Square@@6B@" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_R0?AUSquare@@@8" = linkonce_odr global %rtti.TypeDescriptor12 { ptr @"??_7type_info@@6B@", ptr null, [13 x i8] c".?AUSquare@@\00" }, comdat
@"??_R3Square@@8" = linkonce_odr constant %rtti.ClassHierarchyDescriptor { i32 0, i32 0, i32 2, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R2Square@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_R2Square@@8" = linkonce_odr constant [3 x i32] [i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R1A@?0A@EA@Square@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R1A@?0A@EA@Shape@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 0], comdat
@"??_R1A@?0A@EA@Square@@8" = linkonce_odr constant %rtti.BaseClassDescriptor { i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R0?AUSquare@@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 1, i32 0, i32 -1, i32 0, i32 64, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R3Square@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_R1A@?0A@EA@Shape@@8" = linkonce_odr constant %rtti.BaseClassDescriptor { i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R0?AUShape@@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 0, i32 0, i32 -1, i32 0, i32 64, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R3Shape@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_R0?AUShape@@@8" = linkonce_odr global %rtti.TypeDescriptor11 { ptr @"??_7type_info@@6B@", ptr null, [12 x i8] c".?AUShape@@\00" }, comdat
@"??_R3Shape@@8" = linkonce_odr constant %rtti.ClassHierarchyDescriptor { i32 0, i32 0, i32 1, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R2Shape@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_R2Shape@@8" = linkonce_odr constant [2 x i32] [i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R1A@?0A@EA@Shape@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 0], comdat
@1 = private constant { [4 x ptr] } { [4 x ptr] [ptr @"??_R4Shape@@6B@", ptr @"??_EShape@@UEAAPEAXI@Z", ptr @_purecall, ptr @"?name@Shape@@UEBAPEBDXZ"] }, comdat($"??_7Shape@@6B@")
@"??_R4Shape@@6B@" = linkonce_odr constant %rtti.CompleteObjectLocator { i32 1, i32 0, i32 0, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R0?AUShape@@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R3Shape@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R4Shape@@6B@" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_C@_05KFCIHKGL@shape?$AA@" = linkonce_odr dso_local unnamed_addr constant [6 x i8] c"shape\00", comdat, align 4, !dbg !49
@2 = private constant { [4 x ptr] } { [4 x ptr] [ptr @"??_R4Circle@@6B@", ptr @"??_ECircle@@UEAAPEAXI@Z", ptr @"?area@Circle@@UEBANXZ", ptr @"?name@Shape@@UEBAPEBDXZ"] }, comdat($"??_7Circle@@6B@")
@"??_R4Circle@@6B@" = linkonce_odr constant %rtti.CompleteObjectLocator { i32 1, i32 0, i32 0, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R0?AUCircle@@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R3Circle@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R4Circle@@6B@" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_R0?AUCircle@@@8" = linkonce_odr global %rtti.TypeDescriptor12 { ptr @"??_7type_info@@6B@", ptr null, [13 x i8] c".?AUCircle@@\00" }, comdat
@"??_R3Circle@@8" = linkonce_odr constant %rtti.ClassHierarchyDescriptor { i32 0, i32 0, i32 2, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R2Circle@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_R2Circle@@8" = linkonce_odr constant [3 x i32] [i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R1A@?0A@EA@Circle@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R1A@?0A@EA@Shape@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 0], comdat
@"??_R1A@?0A@EA@Circle@@8" = linkonce_odr constant %rtti.BaseClassDescriptor { i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R0?AUCircle@@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 1, i32 0, i32 -1, i32 0, i32 64, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @"??_R3Circle@@8" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, comdat
@"??_C@_06ICDKOIFG@square?$AA@" = linkonce_odr dso_local unnamed_addr constant [7 x i8] c"square\00", comdat, align 4, !dbg !63
@llvm.global_ctors = appending global [0 x { i32, ptr, ptr }] zeroinitializer

@"??_7Square@@6B@" = alias ptr, getelementptr inbounds (i8, ptr @0, i64 8)
@"??_7Shape@@6B@" = alias ptr, getelementptr inbounds (i8, ptr @1, i64 8)
@"??_EShape@@UEAAPEAXI@Z" = weak dso_local unnamed_addr alias ptr (ptr, i32), ptr @"??_GShape@@UEAAPEAXI@Z"
@"??_ESquare@@UEAAPEAXI@Z" = weak dso_local unnamed_addr alias ptr (ptr, i32), ptr @"??_GSquare@@UEAAPEAXI@Z"
@"??_7Circle@@6B@" = alias ptr, getelementptr inbounds (i8, ptr @2, i64 8)
@"??_ECircle@@UEAAPEAXI@Z" = weak dso_local unnamed_addr alias ptr (ptr, i32), ptr @"??_GCircle@@UEAAPEAXI@Z"

; Function Attrs: mustprogress noinline uwtable
define dso_local void @"?may_throw@@YAXH@Z"(i32 noundef %0) local_unnamed_addr #0 !dbg !95 {
  %2 = alloca %struct.Failure, align 4
    #dbg_value(i32 %0, !99, !DIExpression(), !100)
  %3 = icmp sgt i32 %0, 3, !dbg !101
  br i1 %3, label %4, label %5, !dbg !101

4:                                                ; preds = %1
  store i32 %0, ptr %2, align 4, !dbg !102
  call void @_CxxThrowException(ptr nonnull %2, ptr nonnull @"_TI1?AUFailure@@") #9, !dbg !102
  unreachable, !dbg !102

5:                                                ; preds = %1
  %6 = load i32, ptr @"?counter@@3HA", align 4, !dbg !104
  %7 = add i32 %6, 1, !dbg !104
  store i32 %7, ptr @"?counter@@3HA", align 4, !dbg !104
  ret void, !dbg !105
}

declare dso_local void @_CxxThrowException(ptr, ptr) local_unnamed_addr

; Function Attrs: mustprogress uwtable
define dso_local noundef i32 @"?guarded@@YAHH@Z"(i32 noundef %0) #1 personality ptr @__CxxFrameHandler3 !dbg !106 {
    #dbg_value(ptr poison, !114, !DIExpression(), !118)
    #dbg_value(ptr poison, !114, !DIExpression(), !120)
    #dbg_value(ptr poison, !122, !DIExpression(), !126)
  %2 = alloca ptr, align 8, !DIAssignID !128
    #dbg_assign(i1 poison, !111, !DIExpression(), !128, ptr %2, !DIExpression(), !129)
    #dbg_value(i32 %0, !108, !DIExpression(), !129)
    #dbg_value(ptr @"??_7Square@@6B@", !109, !DIExpression(DW_OP_LLVM_fragment, 0, 64), !130)
    #dbg_value(double poison, !109, !DIExpression(DW_OP_LLVM_fragment, 64, 64), !130)
  invoke void @"?may_throw@@YAXH@Z"(i32 noundef %0)
          to label %3 unwind label %9, !dbg !131

3:                                                ; preds = %1
  %4 = sitofp i32 %0 to double, !dbg !132
    #dbg_value(double %4, !109, !DIExpression(DW_OP_LLVM_fragment, 64, 64), !130)
    #dbg_value(ptr undef, !122, !DIExpression(), !126)
  %5 = fmul double %4, %4, !dbg !133
  %6 = fptosi double %5 to i32, !dbg !134
    #dbg_value(ptr undef, !114, !DIExpression(), !120)
    #dbg_value(ptr @"??_7Square@@6B@", !109, !DIExpression(DW_OP_LLVM_fragment, 0, 64), !130)
  %7 = load i32, ptr @"?destroyed@Square@@2HA", align 4, !dbg !135
  %8 = add i32 %7, 1, !dbg !135
  store i32 %8, ptr @"?destroyed@Square@@2HA", align 4, !dbg !135
  br label %22

9:                                                ; preds = %1
  %10 = cleanuppad within none [], !dbg !137
    #dbg_value(ptr undef, !114, !DIExpression(), !118)
    #dbg_value(ptr @"??_7Square@@6B@", !109, !DIExpression(DW_OP_LLVM_fragment, 0, 64), !130)
  %11 = load i32, ptr @"?destroyed@Square@@2HA", align 4, !dbg !138
  %12 = add i32 %11, 1, !dbg !138
  store i32 %12, ptr @"?destroyed@Square@@2HA", align 4, !dbg !138
  cleanupret from %10 unwind label %13, !dbg !137

13:                                               ; preds = %9
  %14 = catchswitch within none [label %15, label %20] unwind to caller, !dbg !137

15:                                               ; preds = %13
  %16 = catchpad within %14 [ptr @"??_R0?AUFailure@@@8", i32 8, ptr %2], !dbg !137
  %17 = load ptr, ptr %2, align 8, !dbg !139, !nonnull !141, !align !142
  %18 = load i32, ptr %17, align 4, !dbg !139
  %19 = sub i32 0, %18, !dbg !139
  catchret from %16 to label %22, !dbg !143

20:                                               ; preds = %13
  %21 = catchpad within %14 [ptr null, i32 64, ptr null], !dbg !137
  catchret from %21 to label %22, !dbg !144

22:                                               ; preds = %15, %20, %3
  %23 = phi i32 [ %6, %3 ], [ %19, %15 ], [ -1, %20 ], !dbg !129
  ret i32 %23, !dbg !146
}

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(argmem: readwrite)
declare void @llvm.lifetime.start.p0(ptr captures(none)) #2

declare dso_local i32 @__CxxFrameHandler3(...)

; Function Attrs: mustprogress nounwind uwtable
define linkonce_odr dso_local noundef double @"?area@Square@@UEBANXZ"(ptr noundef nonnull align 8 dereferenceable(16) %0) unnamed_addr #3 comdat align 2 !dbg !123 {
    #dbg_value(ptr %0, !122, !DIExpression(), !147)
  %2 = getelementptr inbounds nuw i8, ptr %0, i64 8, !dbg !148
  %3 = load double, ptr %2, align 8, !dbg !148
  %4 = fmul double %3, %3, !dbg !148
  ret double %4, !dbg !148
}

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(argmem: readwrite)
declare void @llvm.lifetime.end.p0(ptr captures(none)) #2

; Function Attrs: mustprogress uwtable
define dso_local noundef double @"?total@@YANPEAPEAUShape@@H@Z"(ptr nofree noundef readonly captures(none) %0, i32 noundef %1) local_unnamed_addr #1 !dbg !149 {
    #dbg_value(i32 %1, !155, !DIExpression(), !160)
    #dbg_value(ptr %0, !156, !DIExpression(), !160)
    #dbg_value(double 0.000000e+00, !157, !DIExpression(), !160)
    #dbg_value(i32 0, !158, !DIExpression(), !161)
  %3 = icmp sgt i32 %1, 0, !dbg !162
  br i1 %3, label %4, label %6, !dbg !162

4:                                                ; preds = %2
  %5 = zext nneg i32 %1 to i64, !dbg !162
  br label %8, !dbg !162

6:                                                ; preds = %8, %2
  %7 = phi double [ 0.000000e+00, %2 ], [ %17, %8 ], !dbg !160
  ret double %7, !dbg !163

8:                                                ; preds = %4, %8
  %9 = phi i64 [ 0, %4 ], [ %18, %8 ]
  %10 = phi double [ 0.000000e+00, %4 ], [ %17, %8 ]
    #dbg_value(i64 %9, !158, !DIExpression(), !161)
    #dbg_value(double %10, !157, !DIExpression(), !160)
  %11 = getelementptr inbounds nuw [8 x i8], ptr %0, i64 %9, !dbg !164
  %12 = load ptr, ptr %11, align 8, !dbg !164
  %13 = load ptr, ptr %12, align 8, !dbg !164
  %14 = getelementptr inbounds nuw i8, ptr %13, i64 8, !dbg !164
  %15 = load ptr, ptr %14, align 8, !dbg !164
  %16 = tail call noundef double %15(ptr noundef nonnull align 8 dereferenceable(8) %12), !dbg !164
  %17 = fadd double %10, %16, !dbg !164
    #dbg_value(double %17, !157, !DIExpression(), !160)
  %18 = add nuw nsw i64 %9, 1, !dbg !166
    #dbg_value(i64 %18, !158, !DIExpression(), !161)
  %19 = icmp eq i64 %18, %5, !dbg !162
  br i1 %19, label %6, label %8, !dbg !162, !llvm.loop !167
}

; Function Attrs: mustprogress nofree norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local noundef range(i32 0, -1) i32 @"?twice@detail@@YAHH@Z"(i32 noundef %0) #4 !dbg !171 {
    #dbg_value(i32 %0, !174, !DIExpression(), !175)
  %2 = shl i32 %0, 1, !dbg !176
  ret i32 %2, !dbg !176
}

; Function Attrs: mustprogress uwtable
define dso_local i32 @shapes_main(i32 noundef %0) local_unnamed_addr #1 personality ptr @__CxxFrameHandler3 !dbg !177 {
  %2 = alloca %struct.Square, align 8, !DIAssignID !183
    #dbg_assign(i1 poison, !180, !DIExpression(), !183, ptr %2, !DIExpression(), !184)
    #dbg_value(ptr poison, !156, !DIExpression(), !185)
    #dbg_value(i32 %0, !179, !DIExpression(), !184)
  call void @llvm.lifetime.start.p0(ptr nonnull %2), !dbg !187
  %3 = sitofp i32 %0 to double, !dbg !187
    #dbg_value(double %3, !188, !DIExpression(), !192)
    #dbg_value(ptr %2, !191, !DIExpression(), !192)
  store ptr @"??_7Square@@6B@", ptr %2, align 8, !dbg !194, !DIAssignID !195
    #dbg_assign(ptr @"??_7Square@@6B@", !180, !DIExpression(DW_OP_LLVM_fragment, 0, 64), !195, ptr %2, !DIExpression(), !184)
  %4 = getelementptr inbounds nuw i8, ptr %2, i64 8, !dbg !194
  store double %3, ptr %4, align 8, !dbg !194, !DIAssignID !196
    #dbg_assign(double %3, !180, !DIExpression(DW_OP_LLVM_fragment, 64, 64), !196, ptr %4, !DIExpression(), !184)
    #dbg_value(ptr %2, !181, !DIExpression(DW_OP_LLVM_fragment, 0, 64), !184)
    #dbg_value(ptr @unit, !181, !DIExpression(DW_OP_LLVM_fragment, 64, 64), !184)
    #dbg_value(i32 2, !155, !DIExpression(), !185)
    #dbg_value(ptr undef, !156, !DIExpression(), !185)
    #dbg_value(double 0.000000e+00, !157, !DIExpression(), !185)
    #dbg_value(i32 0, !158, !DIExpression(), !197)
  br label %5, !dbg !198

5:                                                ; preds = %13, %1
  %6 = phi i1 [ false, %1 ], [ true, %13 ]
  %7 = phi ptr [ %2, %1 ], [ @unit, %13 ]
  %8 = phi double [ 0.000000e+00, %1 ], [ %14, %13 ]
    #dbg_value(i64 poison, !158, !DIExpression(), !197)
    #dbg_value(double %8, !157, !DIExpression(), !185)
  %9 = load ptr, ptr %7, align 8, !dbg !199
  %10 = getelementptr inbounds nuw i8, ptr %9, i64 8, !dbg !199
  %11 = load ptr, ptr %10, align 8, !dbg !199
  %12 = invoke noundef double %11(ptr noundef nonnull align 8 dereferenceable(8) %7)
          to label %13 unwind label %32, !dbg !199, !inline_history !200

13:                                               ; preds = %5
  %14 = fadd double %8, %12, !dbg !199
    #dbg_value(double %14, !157, !DIExpression(), !185)
    #dbg_value(i64 poison, !158, !DIExpression(DW_OP_plus_uconst, 1, DW_OP_stack_value), !197)
  br i1 %6, label %15, label %5, !dbg !198, !llvm.loop !201

15:                                               ; preds = %13
    #dbg_value(i32 100, !204, !DIExpression(), !213)
    #dbg_value(i32 0, !209, !DIExpression(), !213)
    #dbg_value(i32 poison, !210, !DIExpression(), !213)
    #dbg_value(i32 5, !215, !DIExpression(), !225)
    #dbg_value(i32 1, !221, !DIExpression(), !225)
    #dbg_value(i32 %0, !222, !DIExpression(), !225)
  %16 = and i32 %0, 1, !dbg !227
  %17 = zext nneg i32 %16 to i64, !dbg !227
  %18 = getelementptr inbounds nuw [8 x i8], ptr @table, i64 %17, !dbg !227
  %19 = load ptr, ptr %18, align 8, !dbg !227
  %20 = invoke noundef i32 %19(i32 noundef %0)
          to label %21 unwind label %32, !dbg !227

21:                                               ; preds = %15
  %22 = call i32 @llvm.smax.i32(i32 %0, i32 1), !dbg !228
  %23 = call i32 @llvm.smin.i32(i32 %22, i32 5), !dbg !228
  %24 = fptosi double %14 to i32, !dbg !227
    #dbg_value(i32 %24, !210, !DIExpression(), !213)
  %25 = call i32 @llvm.smax.i32(i32 %24, i32 0), !dbg !229
  %26 = call i32 @llvm.smin.i32(i32 %25, i32 100), !dbg !229
  %27 = add nuw nsw i32 %23, 1, !dbg !227
  %28 = add nuw nsw i32 %27, %26, !dbg !227
  %29 = add i32 %28, %20, !dbg !227
    #dbg_value(ptr %2, !114, !DIExpression(), !230)
  store ptr @"??_7Square@@6B@", ptr %2, align 8, !dbg !232, !DIAssignID !233
    #dbg_assign(ptr @"??_7Square@@6B@", !180, !DIExpression(DW_OP_LLVM_fragment, 0, 64), !233, ptr %2, !DIExpression(), !184)
  %30 = load i32, ptr @"?destroyed@Square@@2HA", align 4, !dbg !234
  %31 = add i32 %30, 1, !dbg !234
  store i32 %31, ptr @"?destroyed@Square@@2HA", align 4, !dbg !234
  call void @llvm.lifetime.end.p0(ptr nonnull %2), !dbg !235
  ret i32 %29, !dbg !235

32:                                               ; preds = %5, %15
  %33 = cleanuppad within none [], !dbg !235
    #dbg_value(ptr %2, !114, !DIExpression(), !236)
  store ptr @"??_7Square@@6B@", ptr %2, align 8, !dbg !238, !DIAssignID !239
    #dbg_assign(ptr @"??_7Square@@6B@", !180, !DIExpression(DW_OP_LLVM_fragment, 0, 64), !239, ptr %2, !DIExpression(), !184)
  %34 = load i32, ptr @"?destroyed@Square@@2HA", align 4, !dbg !240
  %35 = add i32 %34, 1, !dbg !240
  store i32 %35, ptr @"?destroyed@Square@@2HA", align 4, !dbg !240
  cleanupret from %33 unwind to caller, !dbg !235
}

; Function Attrs: mustprogress nounwind uwtable
define linkonce_odr dso_local noundef ptr @"?name@Square@@UEBAPEBDXZ"(ptr noundef nonnull align 8 dereferenceable(16) %0) unnamed_addr #3 comdat align 2 !dbg !241 {
    #dbg_value(ptr %0, !243, !DIExpression(), !244)
  ret ptr @"??_C@_06ICDKOIFG@square?$AA@", !dbg !245
}

declare dso_local void @_purecall() unnamed_addr

; Function Attrs: mustprogress nounwind uwtable
define linkonce_odr dso_local noundef ptr @"?name@Shape@@UEBAPEBDXZ"(ptr noundef nonnull align 8 dereferenceable(8) %0) unnamed_addr #3 comdat align 2 !dbg !246 {
    #dbg_value(ptr %0, !248, !DIExpression(), !250)
  ret ptr @"??_C@_05KFCIHKGL@shape?$AA@", !dbg !251
}

; Function Attrs: mustprogress nounwind uwtable
define linkonce_odr dso_local noundef ptr @"??_GShape@@UEAAPEAXI@Z"(ptr noundef nonnull align 8 dereferenceable(8) %0, i32 noundef %1) unnamed_addr #3 comdat align 2 !dbg !252 {
    #dbg_value(i32 %1, !257, !DIExpression(), !259)
    #dbg_value(ptr %0, !258, !DIExpression(), !259)
  tail call void @llvm.trap(), !dbg !260
  unreachable, !dbg !260
}

; Function Attrs: cold noreturn nounwind memory(inaccessiblemem: write)
declare void @llvm.trap() #5

; Function Attrs: mustprogress nounwind uwtable
define linkonce_odr dso_local noundef ptr @"??_GSquare@@UEAAPEAXI@Z"(ptr noundef nonnull align 8 dereferenceable(16) %0, i32 noundef %1) unnamed_addr #3 comdat align 2 !dbg !261 {
    #dbg_value(i32 %1, !265, !DIExpression(), !267)
    #dbg_value(ptr %0, !266, !DIExpression(), !267)
    #dbg_value(ptr %0, !114, !DIExpression(), !268)
  store ptr @"??_7Square@@6B@", ptr %0, align 8, !dbg !270
  %3 = load i32, ptr @"?destroyed@Square@@2HA", align 4, !dbg !271
  %4 = add i32 %3, 1, !dbg !271
  store i32 %4, ptr @"?destroyed@Square@@2HA", align 4, !dbg !271
  %5 = and i32 %1, 1, !dbg !272
  %6 = icmp eq i32 %5, 0, !dbg !272
  br i1 %6, label %8, label %7, !dbg !272

7:                                                ; preds = %2
  tail call void @"??3@YAXPEAX_K@Z"(ptr noundef %0, i64 noundef 16) #10, !dbg !272
  br label %8, !dbg !272

8:                                                ; preds = %7, %2
  ret ptr %0, !dbg !272
}

; Function Attrs: nobuiltin nounwind
declare dso_local void @"??3@YAXPEAX_K@Z"(ptr noundef, i64 noundef) local_unnamed_addr #6

; Function Attrs: mustprogress nounwind uwtable
define linkonce_odr dso_local noundef double @"?area@Circle@@UEBANXZ"(ptr noundef nonnull align 8 dereferenceable(16) %0) unnamed_addr #3 comdat align 2 !dbg !273 {
    #dbg_value(ptr %0, !275, !DIExpression(), !277)
  %2 = getelementptr inbounds nuw i8, ptr %0, i64 8, !dbg !278
  %3 = load double, ptr %2, align 8, !dbg !278
  %4 = fmul double %3, 3.141590e+00, !dbg !278
  %5 = fmul double %3, %4, !dbg !278
  ret double %5, !dbg !278
}

; Function Attrs: inlinehint mustprogress nounwind uwtable
define linkonce_odr dso_local noundef ptr @"??_GCircle@@UEAAPEAXI@Z"(ptr noundef nonnull align 8 dereferenceable(16) %0, i32 noundef %1) unnamed_addr #7 comdat align 2 !dbg !279 {
    #dbg_value(i32 %1, !286, !DIExpression(), !289)
    #dbg_value(ptr %0, !287, !DIExpression(), !289)
  %3 = and i32 %1, 1, !dbg !290
  %4 = icmp eq i32 %3, 0, !dbg !290
  br i1 %4, label %6, label %5, !dbg !290

5:                                                ; preds = %2
  tail call void @"??3@YAXPEAX_K@Z"(ptr noundef %0, i64 noundef 16) #10, !dbg !290
  br label %6, !dbg !290

6:                                                ; preds = %5, %2
  ret ptr %0, !dbg !290
}

; Function Attrs: nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare i32 @llvm.smin.i32(i32, i32) #8

; Function Attrs: nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare i32 @llvm.smax.i32(i32, i32) #8

attributes #0 = { mustprogress noinline uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #1 = { mustprogress uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #2 = { nocallback nofree nosync nounwind willreturn memory(argmem: readwrite) }
attributes #3 = { mustprogress nounwind uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #4 = { mustprogress nofree norecurse nosync nounwind willreturn memory(none) uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #5 = { cold noreturn nounwind memory(inaccessiblemem: write) }
attributes #6 = { nobuiltin nounwind "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #7 = { inlinehint mustprogress nounwind uwtable "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #8 = { nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none) }
attributes #9 = { noreturn }
attributes #10 = { builtin nounwind }

!llvm.dbg.cu = !{!2}
!llvm.module.flags = !{!88, !89, !90, !91, !92, !93}
!llvm.ident = !{!94}

!0 = !DIGlobalVariableExpression(var: !1, expr: !DIExpression())
!1 = distinct !DIGlobalVariable(name: "destroyed", linkageName: "?destroyed@Square@@2HA", scope: !2, file: !6, line: 21, type: !9, isLocal: false, isDefinition: true, declaration: !68)
!2 = distinct !DICompileUnit(language: DW_LANG_C_plus_plus_14, file: !3, producer: "clang version 24.0.0git (https://github.com/llvm/llvm-project.git c74b358f44c31e3615fe942556a41be8c9a8d877)", isOptimized: true, runtimeVersion: 0, emissionKind: FullDebug, retainedTypes: !4, globals: !10, splitDebugInlining: false, nameTableKind: None)
!3 = !DIFile(filename: "R:\\tools\\bitcode\\shapes.cpp", directory: "R:\\target\\fixtures\\bitcode", checksumkind: CSK_MD5, checksum: "34131411e08f0b076252a6c353f4ff80")
!4 = !{!5, !9}
!5 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "Failure", file: !6, line: 33, size: 32, flags: DIFlagTypePassByValue, elements: !7, identifier: ".?AUFailure@@")
!6 = !DIFile(filename: "R:\\tools\\bitcode\\shapes.cpp", directory: "", checksumkind: CSK_MD5, checksum: "34131411e08f0b076252a6c353f4ff80")
!7 = !{!8}
!8 = !DIDerivedType(tag: DW_TAG_member, name: "code", scope: !5, file: !6, line: 34, baseType: !9, size: 32)
!9 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
!10 = !{!0, !11, !13, !49, !54, !63}
!11 = !DIGlobalVariableExpression(var: !12, expr: !DIExpression())
!12 = distinct !DIGlobalVariable(name: "counter", linkageName: "?counter@@3HA", scope: !2, file: !6, line: 37, type: !9, isLocal: false, isDefinition: true)
!13 = !DIGlobalVariableExpression(var: !14, expr: !DIExpression())
!14 = distinct !DIGlobalVariable(name: "unit", scope: !2, file: !6, line: 57, type: !15, isLocal: true, isDefinition: true)
!15 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "Circle", file: !6, line: 23, size: 128, flags: DIFlagTypePassByReference | DIFlagNonTrivial, elements: !16, vtableHolder: !18, identifier: ".?AUCircle@@")
!16 = !{!17, !20, !39, !40, !44}
!17 = !DIDerivedType(tag: DW_TAG_inheritance, scope: !15, baseType: !18, extraData: i32 0)
!18 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "Shape", file: !6, line: 6, size: 64, flags: DIFlagTypePassByReference | DIFlagNonTrivial, elements: !19, vtableHolder: !18, identifier: ".?AUShape@@")
!19 = !{!20, !21, !23, !27, !33}
!20 = !DIDerivedType(tag: DW_TAG_pointer_type, name: "__vtbl_ptr_type", baseType: null, size: 192)
!21 = !DIDerivedType(tag: DW_TAG_member, name: "_vptr$Shape", scope: !6, file: !6, baseType: !22, size: 64, flags: DIFlagArtificial)
!22 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !20, size: 64)
!23 = !DISubprogram(name: "~Shape", scope: !18, file: !6, line: 7, type: !24, scopeLine: 7, containingType: !18, virtualIndex: 0, flags: DIFlagPrototyped | DIFlagIntroducedVirtual, spFlags: DISPFlagVirtual | DISPFlagOptimized)
!24 = !DISubroutineType(types: !25)
!25 = !{null, !26}
!26 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !18, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!27 = !DISubprogram(name: "area", linkageName: "?area@Shape@@UEBANXZ", scope: !18, file: !6, line: 8, type: !28, scopeLine: 8, containingType: !18, virtualIndex: 1, flags: DIFlagPrototyped | DIFlagIntroducedVirtual, spFlags: DISPFlagPureVirtual | DISPFlagOptimized)
!28 = !DISubroutineType(types: !29)
!29 = !{!30, !31}
!30 = !DIBasicType(name: "double", size: 64, encoding: DW_ATE_float)
!31 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !32, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!32 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !18)
!33 = !DISubprogram(name: "name", linkageName: "?name@Shape@@UEBAPEBDXZ", scope: !18, file: !6, line: 9, type: !34, scopeLine: 9, containingType: !18, virtualIndex: 2, flags: DIFlagPrototyped | DIFlagIntroducedVirtual, spFlags: DISPFlagVirtual | DISPFlagOptimized)
!34 = !DISubroutineType(types: !35)
!35 = !{!36, !31}
!36 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !37, size: 64)
!37 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !38)
!38 = !DIBasicType(name: "char", size: 8, encoding: DW_ATE_signed_char)
!39 = !DIDerivedType(tag: DW_TAG_member, name: "radius", scope: !15, file: !6, line: 24, baseType: !30, size: 64, offset: 64)
!40 = !DISubprogram(name: "Circle", scope: !15, file: !6, line: 25, type: !41, scopeLine: 25, flags: DIFlagExplicit | DIFlagPrototyped, spFlags: DISPFlagOptimized)
!41 = !DISubroutineType(types: !42)
!42 = !{null, !43, !30}
!43 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !15, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!44 = !DISubprogram(name: "area", linkageName: "?area@Circle@@UEBANXZ", scope: !15, file: !6, line: 26, type: !45, scopeLine: 26, containingType: !15, virtualIndex: 1, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual | DISPFlagOptimized)
!45 = !DISubroutineType(types: !46)
!46 = !{!30, !47}
!47 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !48, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!48 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !15)
!49 = !DIGlobalVariableExpression(var: !50, expr: !DIExpression())
!50 = distinct !DIGlobalVariable(scope: null, file: !6, line: 9, type: !51, isLocal: true, isDefinition: true)
!51 = !DICompositeType(tag: DW_TAG_array_type, baseType: !37, size: 48, elements: !52)
!52 = !{!53}
!53 = !DISubrange(count: 6)
!54 = !DIGlobalVariableExpression(var: !55, expr: !DIExpression())
!55 = distinct !DIGlobalVariable(name: "table", scope: !2, file: !6, line: 70, type: !56, isLocal: true, isDefinition: true)
!56 = !DICompositeType(tag: DW_TAG_array_type, baseType: !57, size: 128, elements: !61)
!57 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !58)
!58 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !59, size: 64)
!59 = !DISubroutineType(types: !60)
!60 = !{!9, !9}
!61 = !{!62}
!62 = !DISubrange(count: 2)
!63 = !DIGlobalVariableExpression(var: !64, expr: !DIExpression())
!64 = distinct !DIGlobalVariable(scope: null, file: !6, line: 17, type: !65, isLocal: true, isDefinition: true)
!65 = !DICompositeType(tag: DW_TAG_array_type, baseType: !37, size: 56, elements: !66)
!66 = !{!67}
!67 = !DISubrange(count: 7)
!68 = !DIDerivedType(tag: DW_TAG_member, name: "destroyed", scope: !69, file: !6, line: 18, baseType: !9, flags: DIFlagStaticMember)
!69 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "Square", file: !6, line: 12, size: 128, flags: DIFlagTypePassByReference | DIFlagNonTrivial, elements: !70, vtableHolder: !18, identifier: ".?AUSquare@@")
!70 = !{!71, !20, !72, !68, !73, !77, !80, !85}
!71 = !DIDerivedType(tag: DW_TAG_inheritance, scope: !69, baseType: !18, extraData: i32 0)
!72 = !DIDerivedType(tag: DW_TAG_member, name: "side", scope: !69, file: !6, line: 13, baseType: !30, size: 64, offset: 64)
!73 = !DISubprogram(name: "Square", scope: !69, file: !6, line: 14, type: !74, scopeLine: 14, flags: DIFlagExplicit | DIFlagPrototyped, spFlags: DISPFlagOptimized)
!74 = !DISubroutineType(types: !75)
!75 = !{null, !76, !30}
!76 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !69, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!77 = !DISubprogram(name: "~Square", scope: !69, file: !6, line: 15, type: !78, scopeLine: 15, containingType: !69, virtualIndex: 0, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual | DISPFlagOptimized)
!78 = !DISubroutineType(types: !79)
!79 = !{null, !76}
!80 = !DISubprogram(name: "area", linkageName: "?area@Square@@UEBANXZ", scope: !69, file: !6, line: 16, type: !81, scopeLine: 16, containingType: !69, virtualIndex: 1, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual | DISPFlagOptimized)
!81 = !DISubroutineType(types: !82)
!82 = !{!30, !83}
!83 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !84, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!84 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !69)
!85 = !DISubprogram(name: "name", linkageName: "?name@Square@@UEBAPEBDXZ", scope: !69, file: !6, line: 17, type: !86, scopeLine: 17, containingType: !69, virtualIndex: 2, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual | DISPFlagOptimized)
!86 = !DISubroutineType(types: !87)
!87 = !{!36, !83}
!88 = !{i32 2, !"CodeView", i32 1}
!89 = !{i32 2, !"Debug Info Version", i32 3}
!90 = !{i32 8, !"PIC Level", i32 2}
!91 = !{i32 7, !"uwtable", i32 2}
!92 = !{i32 1, !"MaxTLSAlign", i32 65536}
!93 = !{i32 7, !"debug-info-assignment-tracking", i1 true}
!94 = !{!"clang version 24.0.0git (https://github.com/llvm/llvm-project.git c74b358f44c31e3615fe942556a41be8c9a8d877)"}
!95 = distinct !DISubprogram(name: "may_throw", linkageName: "?may_throw@@YAXH@Z", scope: !6, file: !6, line: 39, type: !96, scopeLine: 39, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !98)
!96 = !DISubroutineType(types: !97)
!97 = !{null, !9}
!98 = !{!99}
!99 = !DILocalVariable(name: "x", arg: 1, scope: !95, file: !6, line: 39, type: !9)
!100 = !DILocation(line: 0, scope: !95)
!101 = !DILocation(line: 40, scope: !95)
!102 = !DILocation(line: 41, scope: !103)
!103 = distinct !DILexicalBlock(scope: !95, file: !6, line: 40)
!104 = !DILocation(line: 42, scope: !95)
!105 = !DILocation(line: 43, scope: !95)
!106 = distinct !DISubprogram(name: "guarded", linkageName: "?guarded@@YAHH@Z", scope: !6, file: !6, line: 45, type: !59, scopeLine: 45, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !107)
!107 = !{!108, !109, !111}
!108 = !DILocalVariable(name: "x", arg: 1, scope: !106, file: !6, line: 45, type: !9)
!109 = !DILocalVariable(name: "s", scope: !110, file: !6, line: 47, type: !69)
!110 = distinct !DILexicalBlock(scope: !106, file: !6, line: 46)
!111 = !DILocalVariable(name: "f", scope: !106, file: !6, line: 50, type: !112)
!112 = !DIDerivedType(tag: DW_TAG_reference_type, baseType: !113, size: 64)
!113 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !5)
!114 = !DILocalVariable(name: "this", arg: 1, scope: !115, type: !117, flags: DIFlagArtificial | DIFlagObjectPointer)
!115 = distinct !DISubprogram(name: "~Square", linkageName: "??1Square@@UEAA@XZ", scope: !69, file: !6, line: 15, type: !78, scopeLine: 15, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !77, retainedNodes: !116)
!116 = !{!114}
!117 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !69, size: 64)
!118 = distinct !DILocation(line: 0, scope: !115, inlinedAt: !119)
!119 = distinct !DILocation(line: 50, scope: !110)
!120 = distinct !DILocation(line: 0, scope: !115, inlinedAt: !121)
!121 = distinct !DILocation(line: 50, scope: !110)
!122 = !DILocalVariable(name: "this", arg: 1, scope: !123, type: !125, flags: DIFlagArtificial | DIFlagObjectPointer)
!123 = distinct !DISubprogram(name: "area", linkageName: "?area@Square@@UEBANXZ", scope: !69, file: !6, line: 16, type: !81, scopeLine: 16, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !80, retainedNodes: !124)
!124 = !{!122}
!125 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !84, size: 64)
!126 = distinct !DILocation(line: 0, scope: !123, inlinedAt: !127)
!127 = distinct !DILocation(line: 49, scope: !110)
!128 = distinct !DIAssignID()
!129 = !DILocation(line: 0, scope: !106)
!130 = !DILocation(line: 0, scope: !110)
!131 = !DILocation(line: 48, scope: !110)
!132 = !DILocation(line: 47, scope: !110)
!133 = !DILocation(line: 16, scope: !123, inlinedAt: !127)
!134 = !DILocation(line: 49, scope: !110)
!135 = !DILocation(line: 15, scope: !136, inlinedAt: !121)
!136 = distinct !DILexicalBlock(scope: !115, file: !6, line: 15)
!137 = !DILocation(line: 50, scope: !110)
!138 = !DILocation(line: 15, scope: !136, inlinedAt: !119)
!139 = !DILocation(line: 51, scope: !140)
!140 = distinct !DILexicalBlock(scope: !106, file: !6, line: 50)
!141 = !{}
!142 = !{i64 4}
!143 = !DILocation(line: 52, scope: !140)
!144 = !DILocation(line: 54, scope: !145)
!145 = distinct !DILexicalBlock(scope: !106, file: !6, line: 52)
!146 = !DILocation(line: 55, scope: !106)
!147 = !DILocation(line: 0, scope: !123)
!148 = !DILocation(line: 16, scope: !123)
!149 = distinct !DISubprogram(name: "total", linkageName: "?total@@YANPEAPEAUShape@@H@Z", scope: !6, file: !6, line: 59, type: !150, scopeLine: 59, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !154)
!150 = !DISubroutineType(types: !151)
!151 = !{!30, !152, !9}
!152 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !153, size: 64)
!153 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !18, size: 64)
!154 = !{!155, !156, !157, !158}
!155 = !DILocalVariable(name: "n", arg: 2, scope: !149, file: !6, line: 59, type: !9)
!156 = !DILocalVariable(name: "shapes", arg: 1, scope: !149, file: !6, line: 59, type: !152)
!157 = !DILocalVariable(name: "t", scope: !149, file: !6, line: 60, type: !30)
!158 = !DILocalVariable(name: "i", scope: !159, file: !6, line: 61, type: !9)
!159 = distinct !DILexicalBlock(scope: !149, file: !6, line: 61)
!160 = !DILocation(line: 0, scope: !149)
!161 = !DILocation(line: 0, scope: !159)
!162 = !DILocation(line: 61, scope: !159)
!163 = !DILocation(line: 63, scope: !149)
!164 = !DILocation(line: 62, scope: !165)
!165 = distinct !DILexicalBlock(scope: !159, file: !6, line: 61)
!166 = !DILocation(line: 61, scope: !165)
!167 = distinct !{!167, !162, !168, !169, !170}
!168 = !DILocation(line: 62, scope: !159)
!169 = !{!"llvm.loop.mustprogress"}
!170 = !{!"llvm.loop.unroll.disable"}
!171 = distinct !DISubprogram(name: "twice", linkageName: "?twice@detail@@YAHH@Z", scope: !172, file: !6, line: 67, type: !59, scopeLine: 67, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !173)
!172 = !DINamespace(name: "detail", scope: null)
!173 = !{!174}
!174 = !DILocalVariable(name: "x", arg: 1, scope: !171, file: !6, line: 67, type: !9)
!175 = !DILocation(line: 0, scope: !171)
!176 = !DILocation(line: 67, scope: !171)
!177 = distinct !DISubprogram(name: "shapes_main", scope: !6, file: !6, line: 72, type: !59, scopeLine: 72, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, retainedNodes: !178)
!178 = !{!179, !180, !181}
!179 = !DILocalVariable(name: "argc", arg: 1, scope: !177, file: !6, line: 72, type: !9)
!180 = !DILocalVariable(name: "sq", scope: !177, file: !6, line: 73, type: !69)
!181 = !DILocalVariable(name: "list", scope: !177, file: !6, line: 74, type: !182)
!182 = !DICompositeType(tag: DW_TAG_array_type, baseType: !153, size: 128, elements: !61)
!183 = distinct !DIAssignID()
!184 = !DILocation(line: 0, scope: !177)
!185 = distinct !DILocation(line: 0, scope: !149, inlinedAt: !186)
!186 = distinct !DILocation(line: 75, scope: !177)
!187 = !DILocation(line: 73, scope: !177)
!188 = !DILocalVariable(name: "s", arg: 2, scope: !189, file: !6, line: 14, type: !30)
!189 = distinct !DISubprogram(name: "Square", linkageName: "??0Square@@QEAA@N@Z", scope: !69, file: !6, line: 14, type: !74, scopeLine: 14, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !73, retainedNodes: !190)
!190 = !{!188, !191}
!191 = !DILocalVariable(name: "this", arg: 1, scope: !189, type: !117, flags: DIFlagArtificial | DIFlagObjectPointer)
!192 = distinct !DILocation(line: 0, scope: !189, inlinedAt: !193)
!193 = distinct !DILocation(line: 73, scope: !177)
!194 = !DILocation(line: 14, scope: !189, inlinedAt: !193)
!195 = distinct !DIAssignID()
!196 = distinct !DIAssignID()
!197 = distinct !DILocation(line: 0, scope: !159, inlinedAt: !186)
!198 = !DILocation(line: 61, scope: !159, inlinedAt: !186)
!199 = !DILocation(line: 62, scope: !165, inlinedAt: !186)
!200 = !{ptr @"?total@@YANPEAPEAUShape@@H@Z"}
!201 = distinct !{!201, !202, !203, !169, !170}
!202 = distinct !DILocation(line: 61, scope: !159, inlinedAt: !186)
!203 = distinct !DILocation(line: 62, scope: !159, inlinedAt: !186)
!204 = !DILocalVariable(name: "hi", arg: 3, scope: !205, file: !6, line: 29, type: !9)
!205 = distinct !DISubprogram(name: "clamp_to<int>", linkageName: "??$clamp_to@H@@YAHHHH@Z", scope: !6, file: !6, line: 29, type: !206, scopeLine: 29, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, templateParams: !211, retainedNodes: !208)
!206 = !DISubroutineType(types: !207)
!207 = !{!9, !9, !9, !9}
!208 = !{!204, !209, !210}
!209 = !DILocalVariable(name: "lo", arg: 2, scope: !205, file: !6, line: 29, type: !9)
!210 = !DILocalVariable(name: "v", arg: 1, scope: !205, file: !6, line: 29, type: !9)
!211 = !{!212}
!212 = !DITemplateTypeParameter(name: "T", type: !9)
!213 = distinct !DILocation(line: 0, scope: !205, inlinedAt: !214)
!214 = distinct !DILocation(line: 75, scope: !177)
!215 = !DILocalVariable(name: "hi", arg: 3, scope: !216, file: !6, line: 29, type: !219)
!216 = distinct !DISubprogram(name: "clamp_to<long>", linkageName: "??$clamp_to@J@@YAJJJJ@Z", scope: !6, file: !6, line: 29, type: !217, scopeLine: 29, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, templateParams: !223, retainedNodes: !220)
!217 = !DISubroutineType(types: !218)
!218 = !{!219, !219, !219, !219}
!219 = !DIBasicType(name: "long", size: 32, encoding: DW_ATE_signed)
!220 = !{!215, !221, !222}
!221 = !DILocalVariable(name: "lo", arg: 2, scope: !216, file: !6, line: 29, type: !219)
!222 = !DILocalVariable(name: "v", arg: 1, scope: !216, file: !6, line: 29, type: !219)
!223 = !{!224}
!224 = !DITemplateTypeParameter(name: "T", type: !219)
!225 = distinct !DILocation(line: 0, scope: !216, inlinedAt: !226)
!226 = distinct !DILocation(line: 75, scope: !177)
!227 = !DILocation(line: 75, scope: !177)
!228 = !DILocation(line: 30, scope: !216, inlinedAt: !226)
!229 = !DILocation(line: 30, scope: !205, inlinedAt: !214)
!230 = distinct !DILocation(line: 0, scope: !115, inlinedAt: !231)
!231 = distinct !DILocation(line: 78, scope: !177)
!232 = !DILocation(line: 15, scope: !115, inlinedAt: !231)
!233 = distinct !DIAssignID()
!234 = !DILocation(line: 15, scope: !136, inlinedAt: !231)
!235 = !DILocation(line: 78, scope: !177)
!236 = distinct !DILocation(line: 0, scope: !115, inlinedAt: !237)
!237 = distinct !DILocation(line: 78, scope: !177)
!238 = !DILocation(line: 15, scope: !115, inlinedAt: !237)
!239 = distinct !DIAssignID()
!240 = !DILocation(line: 15, scope: !136, inlinedAt: !237)
!241 = distinct !DISubprogram(name: "name", linkageName: "?name@Square@@UEBAPEBDXZ", scope: !69, file: !6, line: 17, type: !86, scopeLine: 17, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !85, retainedNodes: !242)
!242 = !{!243}
!243 = !DILocalVariable(name: "this", arg: 1, scope: !241, type: !125, flags: DIFlagArtificial | DIFlagObjectPointer)
!244 = !DILocation(line: 0, scope: !241)
!245 = !DILocation(line: 17, scope: !241)
!246 = distinct !DISubprogram(name: "name", linkageName: "?name@Shape@@UEBAPEBDXZ", scope: !18, file: !6, line: 9, type: !34, scopeLine: 9, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !33, retainedNodes: !247)
!247 = !{!248}
!248 = !DILocalVariable(name: "this", arg: 1, scope: !246, type: !249, flags: DIFlagArtificial | DIFlagObjectPointer)
!249 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !32, size: 64)
!250 = !DILocation(line: 0, scope: !246)
!251 = !DILocation(line: 9, scope: !246)
!252 = distinct !DISubprogram(name: "~Shape", linkageName: "??_GShape@@UEAAPEAXI@Z", scope: !18, file: !6, line: 7, type: !253, scopeLine: 7, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !23, retainedNodes: !256)
!253 = !DISubroutineType(types: !254)
!254 = !{!255, !26, !9}
!255 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: null, size: 64)
!256 = !{!257, !258}
!257 = !DILocalVariable(name: "should_call_delete", arg: 2, scope: !252, type: !9, flags: DIFlagArtificial)
!258 = !DILocalVariable(name: "this", arg: 1, scope: !252, type: !153, flags: DIFlagArtificial | DIFlagObjectPointer)
!259 = !DILocation(line: 0, scope: !252)
!260 = !DILocation(line: 7, scope: !252)
!261 = distinct !DISubprogram(name: "~Square", linkageName: "??_GSquare@@UEAAPEAXI@Z", scope: !69, file: !6, line: 15, type: !262, scopeLine: 15, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !77, retainedNodes: !264)
!262 = !DISubroutineType(types: !263)
!263 = !{!255, !76, !9}
!264 = !{!265, !266}
!265 = !DILocalVariable(name: "should_call_delete", arg: 2, scope: !261, type: !9, flags: DIFlagArtificial)
!266 = !DILocalVariable(name: "this", arg: 1, scope: !261, type: !117, flags: DIFlagArtificial | DIFlagObjectPointer)
!267 = !DILocation(line: 0, scope: !261)
!268 = distinct !DILocation(line: 0, scope: !115, inlinedAt: !269)
!269 = distinct !DILocation(line: 15, scope: !261)
!270 = !DILocation(line: 15, scope: !115, inlinedAt: !269)
!271 = !DILocation(line: 15, scope: !136, inlinedAt: !269)
!272 = !DILocation(line: 15, scope: !261)
!273 = distinct !DISubprogram(name: "area", linkageName: "?area@Circle@@UEBANXZ", scope: !15, file: !6, line: 26, type: !45, scopeLine: 26, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !44, retainedNodes: !274)
!274 = !{!275}
!275 = !DILocalVariable(name: "this", arg: 1, scope: !273, type: !276, flags: DIFlagArtificial | DIFlagObjectPointer)
!276 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !48, size: 64)
!277 = !DILocation(line: 0, scope: !273)
!278 = !DILocation(line: 26, scope: !273)
!279 = distinct !DISubprogram(name: "~Circle", linkageName: "??_GCircle@@UEAAPEAXI@Z", scope: !15, file: !6, line: 23, type: !280, scopeLine: 23, flags: DIFlagArtificial | DIFlagPrototyped, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2, declaration: !282, retainedNodes: !285)
!280 = !DISubroutineType(types: !281)
!281 = !{!255, !43, !9}
!282 = !DISubprogram(name: "~Circle", scope: !15, type: !283, containingType: !15, virtualIndex: 0, flags: DIFlagArtificial | DIFlagPrototyped, spFlags: DISPFlagVirtual | DISPFlagOptimized)
!283 = !DISubroutineType(types: !284)
!284 = !{null, !43}
!285 = !{!286, !287}
!286 = !DILocalVariable(name: "should_call_delete", arg: 2, scope: !279, type: !9, flags: DIFlagArtificial)
!287 = !DILocalVariable(name: "this", arg: 1, scope: !279, type: !288, flags: DIFlagArtificial | DIFlagObjectPointer)
!288 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !15, size: 64)
!289 = !DILocation(line: 0, scope: !279)
!290 = !DILocation(line: 23, scope: !279)
