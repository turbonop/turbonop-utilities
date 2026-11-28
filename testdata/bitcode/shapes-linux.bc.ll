; ModuleID = 'shapes-linux.bc'
source_filename = "R:\\tools\\bitcode\\shapes.cpp"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

%struct.Circle = type { %struct.Shape, double }
%struct.Shape = type { ptr }
%struct.Failure = type { i32 }
%struct.Square = type { %struct.Shape, double }

$_ZN6SquareC2Ed = comdat any

$_ZNK6Square4areaEv = comdat any

$_ZN6SquareD2Ev = comdat any

$_ZN6CircleC2Ed = comdat any

$_ZN6CircleD2Ev = comdat any

$_Z8clamp_toIiET_S0_S0_S0_ = comdat any

$_Z8clamp_toIlET_S0_S0_S0_ = comdat any

$_ZNK6Square4nameEv = comdat any

$_ZN5ShapeC2Ev = comdat any

$_ZN6SquareD0Ev = comdat any

$_ZN5ShapeD2Ev = comdat any

$_ZN5ShapeD0Ev = comdat any

$_ZNK5Shape4nameEv = comdat any

$_ZN6CircleD0Ev = comdat any

$_ZNK6Circle4areaEv = comdat any

$_ZTI7Failure = comdat any

$_ZTS7Failure = comdat any

$_ZTV6Square = comdat any

$_ZTI6Square = comdat any

$_ZTS6Square = comdat any

$_ZTI5Shape = comdat any

$_ZTS5Shape = comdat any

$_ZTV5Shape = comdat any

$_ZTV6Circle = comdat any

$_ZTI6Circle = comdat any

$_ZTS6Circle = comdat any

@_ZN6Square9destroyedE = dso_local global i32 0, align 4, !dbg !0
@counter = dso_local global i32 0, align 4, !dbg !11
@_ZTI7Failure = linkonce_odr dso_local constant { ptr, ptr } { ptr getelementptr inbounds (i8, ptr @_ZTVN10__cxxabiv117__class_type_infoE, i64 16), ptr @_ZTS7Failure }, comdat, align 8
@_ZTVN10__cxxabiv117__class_type_infoE = external global [0 x ptr]
@_ZTS7Failure = linkonce_odr dso_local constant [9 x i8] c"7Failure\00", comdat, align 1
@_ZL4unit = internal global %struct.Circle zeroinitializer, align 8, !dbg !13
@__dso_handle = external hidden global i8
@_ZL5table = internal constant [2 x ptr] [ptr @_Z7guardedi, ptr @_ZN6detail5twiceEi], align 16, !dbg !86
@_ZTV6Square = linkonce_odr dso_local constant { [6 x ptr] } { [6 x ptr] [ptr null, ptr @_ZTI6Square, ptr @_ZN6SquareD2Ev, ptr @_ZN6SquareD0Ev, ptr @_ZNK6Square4areaEv, ptr @_ZNK6Square4nameEv] }, comdat, align 8, !dbg !51
@_ZTI6Square = linkonce_odr dso_local constant { ptr, ptr, ptr } { ptr getelementptr inbounds (i8, ptr @_ZTVN10__cxxabiv120__si_class_type_infoE, i64 16), ptr @_ZTS6Square, ptr @_ZTI5Shape }, comdat, align 8
@_ZTVN10__cxxabiv120__si_class_type_infoE = external global [0 x ptr]
@_ZTS6Square = linkonce_odr dso_local constant [8 x i8] c"6Square\00", comdat, align 1
@_ZTI5Shape = linkonce_odr dso_local constant { ptr, ptr } { ptr getelementptr inbounds (i8, ptr @_ZTVN10__cxxabiv117__class_type_infoE, i64 16), ptr @_ZTS5Shape }, comdat, align 8
@_ZTS5Shape = linkonce_odr dso_local constant [7 x i8] c"5Shape\00", comdat, align 1
@_ZTV5Shape = linkonce_odr dso_local constant { [6 x ptr] } { [6 x ptr] [ptr null, ptr @_ZTI5Shape, ptr @_ZN5ShapeD2Ev, ptr @_ZN5ShapeD0Ev, ptr @__cxa_pure_virtual, ptr @_ZNK5Shape4nameEv] }, comdat, align 8, !dbg !75
@.str = private unnamed_addr constant [6 x i8] c"shape\00", align 1, !dbg !78
@_ZTV6Circle = linkonce_odr dso_local constant { [6 x ptr] } { [6 x ptr] [ptr null, ptr @_ZTI6Circle, ptr @_ZN6CircleD2Ev, ptr @_ZN6CircleD0Ev, ptr @_ZNK6Circle4areaEv, ptr @_ZNK5Shape4nameEv] }, comdat, align 8, !dbg !83
@_ZTI6Circle = linkonce_odr dso_local constant { ptr, ptr, ptr } { ptr getelementptr inbounds (i8, ptr @_ZTVN10__cxxabiv120__si_class_type_infoE, i64 16), ptr @_ZTS6Circle, ptr @_ZTI5Shape }, comdat, align 8
@_ZTS6Circle = linkonce_odr dso_local constant [8 x i8] c"6Circle\00", comdat, align 1
@.str.1 = private unnamed_addr constant [7 x i8] c"square\00", align 1, !dbg !95
@llvm.global_ctors = appending global [1 x { i32, ptr, ptr }] [{ i32, ptr, ptr } { i32 65535, ptr @_GLOBAL__sub_I_shapes.cpp, ptr null }]

; Function Attrs: mustprogress noinline optnone uwtable
define dso_local void @_Z9may_throwi(i32 noundef %0) #0 !dbg !108 {
  %2 = alloca i32, align 4
  store i32 %0, ptr %2, align 4
    #dbg_declare(ptr %2, !112, !DIExpression(), !113)
  %3 = load i32, ptr %2, align 4, !dbg !114
  %4 = icmp sgt i32 %3, 3, !dbg !116
  br i1 %4, label %5, label %9, !dbg !116

5:                                                ; preds = %1
  %6 = call ptr @__cxa_allocate_exception(i64 4) #5, !dbg !117
  %7 = getelementptr inbounds nuw %struct.Failure, ptr %6, i32 0, i32 0, !dbg !118
  %8 = load i32, ptr %2, align 4, !dbg !119
  store i32 %8, ptr %7, align 16, !dbg !118
  call void @__cxa_throw(ptr %6, ptr @_ZTI7Failure, ptr null) #7, !dbg !117
  unreachable, !dbg !117

9:                                                ; preds = %1
  %10 = load i32, ptr @counter, align 4, !dbg !120
  %11 = add nsw i32 %10, 1, !dbg !120
  store i32 %11, ptr @counter, align 4, !dbg !120
  ret void, !dbg !121
}

declare ptr @__cxa_allocate_exception(i64)

declare void @__cxa_throw(ptr, ptr, ptr)

; Function Attrs: mustprogress noinline optnone uwtable
define dso_local noundef i32 @_Z7guardedi(i32 noundef %0) #0 personality ptr @__gxx_personality_v0 !dbg !122 {
  %2 = alloca i32, align 4
  %3 = alloca i32, align 4
  %4 = alloca %struct.Square, align 8
  %5 = alloca ptr, align 8
  %6 = alloca i32, align 4
  %7 = alloca ptr, align 8
  store i32 %0, ptr %3, align 4
    #dbg_declare(ptr %3, !123, !DIExpression(), !124)
    #dbg_declare(ptr %4, !125, !DIExpression(), !127)
  %8 = load i32, ptr %3, align 4, !dbg !128
  %9 = sitofp i32 %8 to double, !dbg !128
  invoke void @_ZN6SquareC2Ed(ptr noundef nonnull align 8 dereferenceable(16) %4, double noundef %9)
          to label %10 unwind label %16, !dbg !127

10:                                               ; preds = %1
  %11 = load i32, ptr %3, align 4, !dbg !129
  invoke void @_Z9may_throwi(i32 noundef %11)
          to label %12 unwind label %20, !dbg !130

12:                                               ; preds = %10
  %13 = invoke noundef double @_ZNK6Square4areaEv(ptr noundef nonnull align 8 dereferenceable(16) %4)
          to label %14 unwind label %20, !dbg !131

14:                                               ; preds = %12
  %15 = fptosi double %13 to i32, !dbg !132
  store i32 %15, ptr %2, align 4, !dbg !133
  call void @_ZN6SquareD2Ev(ptr noundef nonnull align 8 dead_on_return(16) dereferenceable(16) %4) #5, !dbg !134
  br label %40

16:                                               ; preds = %1
  %17 = landingpad { ptr, i32 }
          catch ptr @_ZTI7Failure
          catch ptr null, !dbg !135
  %18 = extractvalue { ptr, i32 } %17, 0, !dbg !135
  store ptr %18, ptr %5, align 8, !dbg !135
  %19 = extractvalue { ptr, i32 } %17, 1, !dbg !135
  store i32 %19, ptr %6, align 4, !dbg !135
  br label %24, !dbg !135

20:                                               ; preds = %12, %10
  %21 = landingpad { ptr, i32 }
          catch ptr @_ZTI7Failure
          catch ptr null, !dbg !135
  %22 = extractvalue { ptr, i32 } %21, 0, !dbg !135
  store ptr %22, ptr %5, align 8, !dbg !135
  %23 = extractvalue { ptr, i32 } %21, 1, !dbg !135
  store i32 %23, ptr %6, align 4, !dbg !135
  call void @_ZN6SquareD2Ev(ptr noundef nonnull align 8 dead_on_return(16) dereferenceable(16) %4) #5, !dbg !134
  br label %24, !dbg !134

24:                                               ; preds = %20, %16
  %25 = load i32, ptr %6, align 4, !dbg !136
  %26 = call i32 @llvm.eh.typeid.for.p0(ptr @_ZTI7Failure), !dbg !136
  %27 = icmp eq i32 %25, %26, !dbg !136
  br i1 %27, label %28, label %35, !dbg !136

28:                                               ; preds = %24
    #dbg_declare(ptr %7, !137, !DIExpression(), !140)
  %29 = load ptr, ptr %5, align 8, !dbg !141
  %30 = call ptr @__cxa_begin_catch(ptr %29) #5, !dbg !141
  store ptr %30, ptr %7, align 8, !dbg !141
  %31 = load ptr, ptr %7, align 8, !dbg !142, !nonnull !111, !align !144
  %32 = getelementptr inbounds nuw %struct.Failure, ptr %31, i32 0, i32 0, !dbg !145
  %33 = load i32, ptr %32, align 4, !dbg !145
  %34 = sub nsw i32 0, %33, !dbg !146
  store i32 %34, ptr %2, align 4, !dbg !147
  call void @__cxa_end_catch(), !dbg !148
  br label %40

35:                                               ; preds = %24
  %36 = load ptr, ptr %5, align 8, !dbg !149
  %37 = call ptr @__cxa_begin_catch(ptr %36) #5, !dbg !149
  store i32 -1, ptr %2, align 4, !dbg !150
  call void @__cxa_end_catch(), !dbg !152
  br label %40

38:                                               ; No predecessors!
  call void @llvm.trap(), !dbg !136
  unreachable, !dbg !136

39:                                               ; No predecessors!
  unreachable, !dbg !136

40:                                               ; preds = %28, %35, %14
  %41 = load i32, ptr %2, align 4, !dbg !153
  ret i32 %41, !dbg !153
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN6SquareC2Ed(ptr noundef nonnull align 8 dereferenceable(16) %0, double noundef %1) unnamed_addr #1 comdat align 2 !dbg !154 {
  %3 = alloca ptr, align 8
  %4 = alloca double, align 8
  store ptr %0, ptr %3, align 8
    #dbg_declare(ptr %3, !155, !DIExpression(), !157)
  store double %1, ptr %4, align 8
    #dbg_declare(ptr %4, !158, !DIExpression(), !159)
  %5 = load ptr, ptr %3, align 8
  call void @_ZN5ShapeC2Ev(ptr noundef nonnull align 8 dereferenceable(8) %5) #5, !dbg !160
  store ptr getelementptr inbounds inrange(-16, 32) (i8, ptr @_ZTV6Square, i64 16), ptr %5, align 8, !dbg !161
  %6 = getelementptr inbounds nuw %struct.Square, ptr %5, i32 0, i32 1, !dbg !162
  %7 = load double, ptr %4, align 8, !dbg !163
  store double %7, ptr %6, align 8, !dbg !162
  ret void, !dbg !164
}

declare i32 @__gxx_personality_v0(...)

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local noundef double @_ZNK6Square4areaEv(ptr noundef nonnull align 8 dereferenceable(16) %0) unnamed_addr #1 comdat align 2 !dbg !165 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !166, !DIExpression(), !168)
  %3 = load ptr, ptr %2, align 8
  %4 = getelementptr inbounds nuw %struct.Square, ptr %3, i32 0, i32 1, !dbg !169
  %5 = load double, ptr %4, align 8, !dbg !169
  %6 = getelementptr inbounds nuw %struct.Square, ptr %3, i32 0, i32 1, !dbg !170
  %7 = load double, ptr %6, align 8, !dbg !170
  %8 = fmul double %5, %7, !dbg !171
  ret double %8, !dbg !172
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN6SquareD2Ev(ptr noundef nonnull align 8 dead_on_return(16) dereferenceable(16) %0) unnamed_addr #1 comdat align 2 !dbg !173 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !174, !DIExpression(), !175)
  %3 = load ptr, ptr %2, align 8
  store ptr getelementptr inbounds inrange(-16, 32) (i8, ptr @_ZTV6Square, i64 16), ptr %3, align 8, !dbg !176
  %4 = load i32, ptr @_ZN6Square9destroyedE, align 4, !dbg !177
  %5 = add nsw i32 %4, 1, !dbg !177
  store i32 %5, ptr @_ZN6Square9destroyedE, align 4, !dbg !177
  call void @_ZN5ShapeD2Ev(ptr noundef nonnull align 8 dead_on_return(8) dereferenceable(8) %3) #5, !dbg !179
  ret void, !dbg !180
}

; Function Attrs: nounwind memory(none)
declare i32 @llvm.eh.typeid.for.p0(ptr) #2

declare ptr @__cxa_begin_catch(ptr)

declare void @__cxa_end_catch()

; Function Attrs: cold noreturn nounwind memory(inaccessiblemem: write)
declare void @llvm.trap() #3

; Function Attrs: noinline uwtable
define internal void @__cxx_global_var_init() #4 section ".text.startup" !dbg !181 {
  call void @_ZN6CircleC2Ed(ptr noundef nonnull align 8 dereferenceable(16) @_ZL4unit, double noundef 1.000000e+00), !dbg !184
  %1 = call i32 @__cxa_atexit(ptr @_ZN6CircleD2Ev, ptr @_ZL4unit, ptr @__dso_handle) #5, !dbg !185
  ret void, !dbg !186
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN6CircleC2Ed(ptr noundef nonnull align 8 dereferenceable(16) %0, double noundef %1) unnamed_addr #1 comdat align 2 !dbg !187 {
  %3 = alloca ptr, align 8
  %4 = alloca double, align 8
  store ptr %0, ptr %3, align 8
    #dbg_declare(ptr %3, !188, !DIExpression(), !190)
  store double %1, ptr %4, align 8
    #dbg_declare(ptr %4, !191, !DIExpression(), !192)
  %5 = load ptr, ptr %3, align 8
  call void @_ZN5ShapeC2Ev(ptr noundef nonnull align 8 dereferenceable(8) %5) #5, !dbg !193
  store ptr getelementptr inbounds inrange(-16, 32) (i8, ptr @_ZTV6Circle, i64 16), ptr %5, align 8, !dbg !194
  %6 = getelementptr inbounds nuw %struct.Circle, ptr %5, i32 0, i32 1, !dbg !195
  %7 = load double, ptr %4, align 8, !dbg !196
  store double %7, ptr %6, align 8, !dbg !195
  ret void, !dbg !197
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN6CircleD2Ev(ptr noundef nonnull align 8 dead_on_return(16) dereferenceable(16) %0) unnamed_addr #1 comdat align 2 !dbg !198 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !202, !DIExpression(), !203)
  %3 = load ptr, ptr %2, align 8
  call void @_ZN5ShapeD2Ev(ptr noundef nonnull align 8 dead_on_return(8) dereferenceable(8) %3) #5, !dbg !204
  ret void, !dbg !206
}

; Function Attrs: nounwind
declare i32 @__cxa_atexit(ptr, ptr, ptr) #5

; Function Attrs: mustprogress noinline optnone uwtable
define dso_local noundef double @_Z5totalPP5Shapei(ptr noundef %0, i32 noundef %1) #0 !dbg !207 {
  %3 = alloca ptr, align 8
  %4 = alloca i32, align 4
  %5 = alloca double, align 8
  %6 = alloca i32, align 4
  store ptr %0, ptr %3, align 8
    #dbg_declare(ptr %3, !212, !DIExpression(), !213)
  store i32 %1, ptr %4, align 4
    #dbg_declare(ptr %4, !214, !DIExpression(), !215)
    #dbg_declare(ptr %5, !216, !DIExpression(), !217)
  store double 0.000000e+00, ptr %5, align 8, !dbg !217
    #dbg_declare(ptr %6, !218, !DIExpression(), !220)
  store i32 0, ptr %6, align 4, !dbg !220
  br label %7, !dbg !221

7:                                                ; preds = %23, %2
  %8 = load i32, ptr %6, align 4, !dbg !222
  %9 = load i32, ptr %4, align 4, !dbg !224
  %10 = icmp slt i32 %8, %9, !dbg !225
  br i1 %10, label %11, label %26, !dbg !226

11:                                               ; preds = %7
  %12 = load ptr, ptr %3, align 8, !dbg !227
  %13 = load i32, ptr %6, align 4, !dbg !228
  %14 = sext i32 %13 to i64, !dbg !227
  %15 = getelementptr inbounds ptr, ptr %12, i64 %14, !dbg !227
  %16 = load ptr, ptr %15, align 8, !dbg !227
  %17 = load ptr, ptr %16, align 8, !dbg !229
  %18 = getelementptr inbounds ptr, ptr %17, i64 2, !dbg !229
  %19 = load ptr, ptr %18, align 8, !dbg !229
  %20 = call noundef double %19(ptr noundef nonnull align 8 dereferenceable(8) %16), !dbg !229
  %21 = load double, ptr %5, align 8, !dbg !230
  %22 = fadd double %21, %20, !dbg !230
  store double %22, ptr %5, align 8, !dbg !230
  br label %23, !dbg !231

23:                                               ; preds = %11
  %24 = load i32, ptr %6, align 4, !dbg !232
  %25 = add nsw i32 %24, 1, !dbg !232
  store i32 %25, ptr %6, align 4, !dbg !232
  br label %7, !dbg !233, !llvm.loop !234

26:                                               ; preds = %7
  %27 = load double, ptr %5, align 8, !dbg !237
  ret double %27, !dbg !238
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define dso_local noundef i32 @_ZN6detail5twiceEi(i32 noundef %0) #1 !dbg !239 {
  %2 = alloca i32, align 4
  store i32 %0, ptr %2, align 4
    #dbg_declare(ptr %2, !241, !DIExpression(), !242)
  %3 = load i32, ptr %2, align 4, !dbg !243
  %4 = mul nsw i32 %3, 2, !dbg !244
  ret i32 %4, !dbg !245
}

; Function Attrs: mustprogress noinline optnone uwtable
define dso_local i32 @shapes_main(i32 noundef %0) #0 personality ptr @__gxx_personality_v0 !dbg !246 {
  %2 = alloca i32, align 4
  %3 = alloca %struct.Square, align 8
  %4 = alloca [2 x ptr], align 16
  %5 = alloca ptr, align 8
  %6 = alloca i32, align 4
  store i32 %0, ptr %2, align 4
    #dbg_declare(ptr %2, !247, !DIExpression(), !248)
    #dbg_declare(ptr %3, !249, !DIExpression(), !250)
  %7 = load i32, ptr %2, align 4, !dbg !251
  %8 = sitofp i32 %7 to double, !dbg !251
  call void @_ZN6SquareC2Ed(ptr noundef nonnull align 8 dereferenceable(16) %3, double noundef %8), !dbg !250
    #dbg_declare(ptr %4, !252, !DIExpression(), !254)
  store ptr %3, ptr %4, align 8, !dbg !255
  %9 = getelementptr inbounds ptr, ptr %4, i64 1, !dbg !255
  store ptr @_ZL4unit, ptr %9, align 8, !dbg !255
  %10 = getelementptr inbounds [2 x ptr], ptr %4, i64 0, i64 0, !dbg !256
  %11 = invoke noundef double @_Z5totalPP5Shapei(ptr noundef %10, i32 noundef 2)
          to label %12 unwind label %39, !dbg !257

12:                                               ; preds = %1
  %13 = fptosi double %11 to i32, !dbg !257
  %14 = invoke noundef i32 @_Z8clamp_toIiET_S0_S0_S0_(i32 noundef %13, i32 noundef 0, i32 noundef 100)
          to label %15 unwind label %39, !dbg !258

15:                                               ; preds = %12
  %16 = load i32, ptr %2, align 4, !dbg !259
  %17 = sext i32 %16 to i64, !dbg !259
  %18 = invoke noundef i64 @_Z8clamp_toIlET_S0_S0_S0_(i64 noundef %17, i64 noundef 1, i64 noundef 5)
          to label %19 unwind label %39, !dbg !260

19:                                               ; preds = %15
  %20 = trunc i64 %18 to i32, !dbg !260
  %21 = add nsw i32 %14, %20, !dbg !261
  %22 = load i32, ptr %2, align 4, !dbg !262
  %23 = and i32 %22, 1, !dbg !263
  %24 = sext i32 %23 to i64, !dbg !264
  %25 = getelementptr inbounds [2 x ptr], ptr @_ZL5table, i64 0, i64 %24, !dbg !264
  %26 = load ptr, ptr %25, align 8, !dbg !264
  %27 = load i32, ptr %2, align 4, !dbg !265
  %28 = invoke noundef i32 %26(i32 noundef %27)
          to label %29 unwind label %39, !dbg !264

29:                                               ; preds = %19
  %30 = add nsw i32 %21, %28, !dbg !266
  %31 = invoke noundef ptr @_ZNK6Square4nameEv(ptr noundef nonnull align 8 dereferenceable(16) %3)
          to label %32 unwind label %39, !dbg !267

32:                                               ; preds = %29
  %33 = getelementptr inbounds i8, ptr %31, i64 0, !dbg !267
  %34 = load i8, ptr %33, align 1, !dbg !267
  %35 = sext i8 %34 to i32, !dbg !268
  %36 = icmp eq i32 %35, 115, !dbg !269
  %37 = zext i1 %36 to i32, !dbg !270
  %38 = add nsw i32 %30, %37, !dbg !271
  call void @_ZN6SquareD2Ev(ptr noundef nonnull align 8 dead_on_return(16) dereferenceable(16) %3) #5, !dbg !272
  ret i32 %38, !dbg !272

39:                                               ; preds = %29, %19, %15, %12, %1
  %40 = landingpad { ptr, i32 }
          cleanup, !dbg !272
  %41 = extractvalue { ptr, i32 } %40, 0, !dbg !272
  store ptr %41, ptr %5, align 8, !dbg !272
  %42 = extractvalue { ptr, i32 } %40, 1, !dbg !272
  store i32 %42, ptr %6, align 4, !dbg !272
  call void @_ZN6SquareD2Ev(ptr noundef nonnull align 8 dead_on_return(16) dereferenceable(16) %3) #5, !dbg !272
  br label %43, !dbg !272

43:                                               ; preds = %39
  %44 = load ptr, ptr %5, align 8, !dbg !272
  %45 = load i32, ptr %6, align 4, !dbg !272
  %46 = insertvalue { ptr, i32 } poison, ptr %44, 0, !dbg !272
  %47 = insertvalue { ptr, i32 } %46, i32 %45, 1, !dbg !272
  resume { ptr, i32 } %47, !dbg !272
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local noundef i32 @_Z8clamp_toIiET_S0_S0_S0_(i32 noundef %0, i32 noundef %1, i32 noundef %2) #1 comdat !dbg !273 {
  %4 = alloca i32, align 4
  %5 = alloca i32, align 4
  %6 = alloca i32, align 4
  store i32 %0, ptr %4, align 4
    #dbg_declare(ptr %4, !278, !DIExpression(), !279)
  store i32 %1, ptr %5, align 4
    #dbg_declare(ptr %5, !280, !DIExpression(), !281)
  store i32 %2, ptr %6, align 4
    #dbg_declare(ptr %6, !282, !DIExpression(), !283)
  %7 = load i32, ptr %4, align 4, !dbg !284
  %8 = load i32, ptr %5, align 4, !dbg !285
  %9 = icmp slt i32 %7, %8, !dbg !286
  br i1 %9, label %10, label %12, !dbg !284

10:                                               ; preds = %3
  %11 = load i32, ptr %5, align 4, !dbg !287
  br label %22, !dbg !284

12:                                               ; preds = %3
  %13 = load i32, ptr %4, align 4, !dbg !288
  %14 = load i32, ptr %6, align 4, !dbg !289
  %15 = icmp sgt i32 %13, %14, !dbg !290
  br i1 %15, label %16, label %18, !dbg !288

16:                                               ; preds = %12
  %17 = load i32, ptr %6, align 4, !dbg !291
  br label %20, !dbg !288

18:                                               ; preds = %12
  %19 = load i32, ptr %4, align 4, !dbg !292
  br label %20, !dbg !288

20:                                               ; preds = %18, %16
  %21 = phi i32 [ %17, %16 ], [ %19, %18 ], !dbg !288
  br label %22, !dbg !284

22:                                               ; preds = %20, %10
  %23 = phi i32 [ %11, %10 ], [ %21, %20 ], !dbg !284
  ret i32 %23, !dbg !293
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local noundef i64 @_Z8clamp_toIlET_S0_S0_S0_(i64 noundef %0, i64 noundef %1, i64 noundef %2) #1 comdat !dbg !294 {
  %4 = alloca i64, align 8
  %5 = alloca i64, align 8
  %6 = alloca i64, align 8
  store i64 %0, ptr %4, align 8
    #dbg_declare(ptr %4, !300, !DIExpression(), !301)
  store i64 %1, ptr %5, align 8
    #dbg_declare(ptr %5, !302, !DIExpression(), !303)
  store i64 %2, ptr %6, align 8
    #dbg_declare(ptr %6, !304, !DIExpression(), !305)
  %7 = load i64, ptr %4, align 8, !dbg !306
  %8 = load i64, ptr %5, align 8, !dbg !307
  %9 = icmp slt i64 %7, %8, !dbg !308
  br i1 %9, label %10, label %12, !dbg !306

10:                                               ; preds = %3
  %11 = load i64, ptr %5, align 8, !dbg !309
  br label %22, !dbg !306

12:                                               ; preds = %3
  %13 = load i64, ptr %4, align 8, !dbg !310
  %14 = load i64, ptr %6, align 8, !dbg !311
  %15 = icmp sgt i64 %13, %14, !dbg !312
  br i1 %15, label %16, label %18, !dbg !310

16:                                               ; preds = %12
  %17 = load i64, ptr %6, align 8, !dbg !313
  br label %20, !dbg !310

18:                                               ; preds = %12
  %19 = load i64, ptr %4, align 8, !dbg !314
  br label %20, !dbg !310

20:                                               ; preds = %18, %16
  %21 = phi i64 [ %17, %16 ], [ %19, %18 ], !dbg !310
  br label %22, !dbg !306

22:                                               ; preds = %20, %10
  %23 = phi i64 [ %11, %10 ], [ %21, %20 ], !dbg !306
  ret i64 %23, !dbg !315
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local noundef ptr @_ZNK6Square4nameEv(ptr noundef nonnull align 8 dereferenceable(16) %0) unnamed_addr #1 comdat align 2 !dbg !316 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !317, !DIExpression(), !318)
  %3 = load ptr, ptr %2, align 8
  ret ptr @.str.1, !dbg !319
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN5ShapeC2Ev(ptr noundef nonnull align 8 dereferenceable(8) %0) unnamed_addr #1 comdat align 2 !dbg !320 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !322, !DIExpression(), !323)
  %3 = load ptr, ptr %2, align 8
  store ptr getelementptr inbounds inrange(-16, 32) (i8, ptr @_ZTV5Shape, i64 16), ptr %3, align 8, !dbg !324
  ret void, !dbg !324
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN6SquareD0Ev(ptr noundef nonnull align 8 dereferenceable(16) %0) unnamed_addr #1 comdat align 2 !dbg !325 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !326, !DIExpression(), !327)
  %3 = load ptr, ptr %2, align 8
  call void @_ZN6SquareD2Ev(ptr noundef nonnull align 8 dead_on_return(16) dereferenceable(16) %3) #5, !dbg !328
  call void @_ZdlPvm(ptr noundef %3, i64 noundef 16) #8, !dbg !328
  ret void, !dbg !329
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN5ShapeD2Ev(ptr noundef nonnull align 8 dead_on_return(8) dereferenceable(8) %0) unnamed_addr #1 comdat align 2 !dbg !330 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !331, !DIExpression(), !332)
  %3 = load ptr, ptr %2, align 8
  ret void, !dbg !333
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN5ShapeD0Ev(ptr noundef nonnull align 8 dereferenceable(8) %0) unnamed_addr #1 comdat align 2 !dbg !334 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !335, !DIExpression(), !336)
  %3 = load ptr, ptr %2, align 8
  call void @llvm.trap(), !dbg !337
  unreachable, !dbg !337
}

declare void @__cxa_pure_virtual() unnamed_addr

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local noundef ptr @_ZNK5Shape4nameEv(ptr noundef nonnull align 8 dereferenceable(8) %0) unnamed_addr #1 comdat align 2 !dbg !338 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !339, !DIExpression(), !341)
  %3 = load ptr, ptr %2, align 8
  ret ptr @.str, !dbg !342
}

; Function Attrs: nobuiltin nounwind
declare void @_ZdlPvm(ptr noundef, i64 noundef) #6

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local void @_ZN6CircleD0Ev(ptr noundef nonnull align 8 dereferenceable(16) %0) unnamed_addr #1 comdat align 2 !dbg !343 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !344, !DIExpression(), !345)
  %3 = load ptr, ptr %2, align 8
  call void @_ZN6CircleD2Ev(ptr noundef nonnull align 8 dead_on_return(16) dereferenceable(16) %3) #5, !dbg !346
  call void @_ZdlPvm(ptr noundef %3, i64 noundef 16) #8, !dbg !346
  ret void, !dbg !346
}

; Function Attrs: mustprogress noinline nounwind optnone uwtable
define linkonce_odr dso_local noundef double @_ZNK6Circle4areaEv(ptr noundef nonnull align 8 dereferenceable(16) %0) unnamed_addr #1 comdat align 2 !dbg !347 {
  %2 = alloca ptr, align 8
  store ptr %0, ptr %2, align 8
    #dbg_declare(ptr %2, !348, !DIExpression(), !350)
  %3 = load ptr, ptr %2, align 8
  %4 = getelementptr inbounds nuw %struct.Circle, ptr %3, i32 0, i32 1, !dbg !351
  %5 = load double, ptr %4, align 8, !dbg !351
  %6 = fmul double 3.141590e+00, %5, !dbg !352
  %7 = getelementptr inbounds nuw %struct.Circle, ptr %3, i32 0, i32 1, !dbg !353
  %8 = load double, ptr %7, align 8, !dbg !353
  %9 = fmul double %6, %8, !dbg !354
  ret double %9, !dbg !355
}

; Function Attrs: noinline uwtable
define internal void @_GLOBAL__sub_I_shapes.cpp() #4 section ".text.startup" !dbg !356 {
  call void @__cxx_global_var_init(), !dbg !358
  ret void
}

attributes #0 = { mustprogress noinline optnone uwtable "frame-pointer"="all" "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #1 = { mustprogress noinline nounwind optnone uwtable "frame-pointer"="all" "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #2 = { nounwind memory(none) }
attributes #3 = { cold noreturn nounwind memory(inaccessiblemem: write) }
attributes #4 = { noinline uwtable "frame-pointer"="all" "min-legal-vector-width"="0" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #5 = { nounwind }
attributes #6 = { nobuiltin nounwind "frame-pointer"="all" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" "target-features"="+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87" "tune-cpu"="generic" }
attributes #7 = { noreturn }
attributes #8 = { builtin nounwind }

!llvm.dbg.cu = !{!2}
!llvm.module.flags = !{!100, !101, !102, !103, !104, !105, !106}
!llvm.ident = !{!107}

!0 = !DIGlobalVariableExpression(var: !1, expr: !DIExpression())
!1 = distinct !DIGlobalVariable(name: "destroyed", linkageName: "_ZN6Square9destroyedE", scope: !2, file: !6, line: 21, type: !9, isLocal: false, isDefinition: true, declaration: !59)
!2 = distinct !DICompileUnit(language: DW_LANG_C_plus_plus_14, file: !3, producer: "clang version 24.0.0git (https://github.com/llvm/llvm-project.git c74b358f44c31e3615fe942556a41be8c9a8d877)", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug, retainedTypes: !4, globals: !10, splitDebugInlining: false, nameTableKind: None)
!3 = !DIFile(filename: "R:\\tools\\bitcode\\shapes.cpp", directory: "R:\\target\\fixtures\\bitcode", checksumkind: CSK_MD5, checksum: "34131411e08f0b076252a6c353f4ff80")
!4 = !{!5, !9}
!5 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "Failure", file: !6, line: 33, size: 32, flags: DIFlagTypePassByValue, elements: !7, identifier: "_ZTS7Failure")
!6 = !DIFile(filename: "R:\\tools\\bitcode\\shapes.cpp", directory: "", checksumkind: CSK_MD5, checksum: "34131411e08f0b076252a6c353f4ff80")
!7 = !{!8}
!8 = !DIDerivedType(tag: DW_TAG_member, name: "code", scope: !5, file: !6, line: 34, baseType: !9, size: 32)
!9 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
!10 = !{!0, !11, !13, !51, !75, !78, !83, !86, !95}
!11 = !DIGlobalVariableExpression(var: !12, expr: !DIExpression())
!12 = distinct !DIGlobalVariable(name: "counter", scope: !2, file: !6, line: 37, type: !9, isLocal: false, isDefinition: true)
!13 = !DIGlobalVariableExpression(var: !14, expr: !DIExpression())
!14 = distinct !DIGlobalVariable(name: "unit", linkageName: "_ZL4unit", scope: !2, file: !6, line: 57, type: !15, isLocal: true, isDefinition: true)
!15 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "Circle", file: !6, line: 23, size: 128, flags: DIFlagTypePassByReference | DIFlagNonTrivial, elements: !16, vtableHolder: !18, identifier: "_ZTS6Circle")
!16 = !{!17, !41, !42, !46}
!17 = !DIDerivedType(tag: DW_TAG_inheritance, scope: !15, baseType: !18, extraData: i32 0)
!18 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "Shape", file: !6, line: 6, size: 64, flags: DIFlagTypePassByReference | DIFlagNonTrivial, elements: !19, vtableHolder: !18, identifier: "_ZTS5Shape")
!19 = !{!20, !25, !29, !35}
!20 = !DIDerivedType(tag: DW_TAG_member, name: "_vptr$Shape", scope: !6, file: !6, baseType: !21, size: 64, flags: DIFlagArtificial)
!21 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !22, size: 64)
!22 = !DIDerivedType(tag: DW_TAG_pointer_type, name: "__vtbl_ptr_type", baseType: !23, size: 64)
!23 = !DISubroutineType(types: !24)
!24 = !{!9}
!25 = !DISubprogram(name: "~Shape", linkageName: "_ZN5ShapeD4Ev", scope: !18, file: !6, line: 7, type: !26, scopeLine: 7, containingType: !18, virtualIndex: 0, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual)
!26 = !DISubroutineType(types: !27)
!27 = !{null, !28}
!28 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !18, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!29 = !DISubprogram(name: "area", linkageName: "_ZNK5Shape4areaEv", scope: !18, file: !6, line: 8, type: !30, scopeLine: 8, containingType: !18, virtualIndex: 2, flags: DIFlagPrototyped, spFlags: DISPFlagPureVirtual)
!30 = !DISubroutineType(types: !31)
!31 = !{!32, !33}
!32 = !DIBasicType(name: "double", size: 64, encoding: DW_ATE_float)
!33 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !34, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!34 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !18)
!35 = !DISubprogram(name: "name", linkageName: "_ZNK5Shape4nameEv", scope: !18, file: !6, line: 9, type: !36, scopeLine: 9, containingType: !18, virtualIndex: 3, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual)
!36 = !DISubroutineType(types: !37)
!37 = !{!38, !33}
!38 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !39, size: 64)
!39 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !40)
!40 = !DIBasicType(name: "char", size: 8, encoding: DW_ATE_signed_char)
!41 = !DIDerivedType(tag: DW_TAG_member, name: "radius", scope: !15, file: !6, line: 24, baseType: !32, size: 64, offset: 64)
!42 = !DISubprogram(name: "Circle", linkageName: "_ZN6CircleC4Ed", scope: !15, file: !6, line: 25, type: !43, scopeLine: 25, flags: DIFlagExplicit | DIFlagPrototyped, spFlags: 0)
!43 = !DISubroutineType(types: !44)
!44 = !{null, !45, !32}
!45 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !15, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!46 = !DISubprogram(name: "area", linkageName: "_ZNK6Circle4areaEv", scope: !15, file: !6, line: 26, type: !47, scopeLine: 26, containingType: !15, virtualIndex: 2, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual)
!47 = !DISubroutineType(types: !48)
!48 = !{!32, !49}
!49 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !50, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!50 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !15)
!51 = !DIGlobalVariableExpression(var: !52, expr: !DIExpression())
!52 = distinct !DIGlobalVariable(name: "__clang_vtable", linkageName: "_ZTV6Square", scope: !2, file: !6, type: !53, isLocal: false, isDefinition: true, declaration: !54, align: 64)
!53 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: null, size: 64)
!54 = !DIDerivedType(tag: DW_TAG_variable, name: "__clang_vtable", scope: !55, file: !6, baseType: !53, flags: DIFlagPrivate | DIFlagArtificial | DIFlagStaticMember)
!55 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "Square", file: !6, line: 12, size: 128, flags: DIFlagTypePassByReference | DIFlagNonTrivial, elements: !56, vtableHolder: !18, identifier: "_ZTS6Square")
!56 = !{!57, !58, !59, !60, !64, !67, !72}
!57 = !DIDerivedType(tag: DW_TAG_inheritance, scope: !55, baseType: !18, extraData: i32 0)
!58 = !DIDerivedType(tag: DW_TAG_member, name: "side", scope: !55, file: !6, line: 13, baseType: !32, size: 64, offset: 64)
!59 = !DIDerivedType(tag: DW_TAG_variable, name: "destroyed", scope: !55, file: !6, line: 18, baseType: !9, flags: DIFlagStaticMember)
!60 = !DISubprogram(name: "Square", linkageName: "_ZN6SquareC4Ed", scope: !55, file: !6, line: 14, type: !61, scopeLine: 14, flags: DIFlagExplicit | DIFlagPrototyped, spFlags: 0)
!61 = !DISubroutineType(types: !62)
!62 = !{null, !63, !32}
!63 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !55, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!64 = !DISubprogram(name: "~Square", linkageName: "_ZN6SquareD4Ev", scope: !55, file: !6, line: 15, type: !65, scopeLine: 15, containingType: !55, virtualIndex: 0, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual)
!65 = !DISubroutineType(types: !66)
!66 = !{null, !63}
!67 = !DISubprogram(name: "area", linkageName: "_ZNK6Square4areaEv", scope: !55, file: !6, line: 16, type: !68, scopeLine: 16, containingType: !55, virtualIndex: 2, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual)
!68 = !DISubroutineType(types: !69)
!69 = !{!32, !70}
!70 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !71, size: 64, flags: DIFlagArtificial | DIFlagObjectPointer)
!71 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !55)
!72 = !DISubprogram(name: "name", linkageName: "_ZNK6Square4nameEv", scope: !55, file: !6, line: 17, type: !73, scopeLine: 17, containingType: !55, virtualIndex: 3, flags: DIFlagPrototyped, spFlags: DISPFlagVirtual)
!73 = !DISubroutineType(types: !74)
!74 = !{!38, !70}
!75 = !DIGlobalVariableExpression(var: !76, expr: !DIExpression())
!76 = distinct !DIGlobalVariable(name: "__clang_vtable", linkageName: "_ZTV5Shape", scope: !2, file: !6, type: !53, isLocal: false, isDefinition: true, declaration: !77, align: 64)
!77 = !DIDerivedType(tag: DW_TAG_variable, name: "__clang_vtable", scope: !18, file: !6, baseType: !53, flags: DIFlagPrivate | DIFlagArtificial | DIFlagStaticMember)
!78 = !DIGlobalVariableExpression(var: !79, expr: !DIExpression())
!79 = distinct !DIGlobalVariable(scope: null, file: !6, line: 9, type: !80, isLocal: true, isDefinition: true)
!80 = !DICompositeType(tag: DW_TAG_array_type, baseType: !39, size: 48, elements: !81)
!81 = !{!82}
!82 = !DISubrange(count: 6)
!83 = !DIGlobalVariableExpression(var: !84, expr: !DIExpression())
!84 = distinct !DIGlobalVariable(name: "__clang_vtable", linkageName: "_ZTV6Circle", scope: !2, file: !6, type: !53, isLocal: false, isDefinition: true, declaration: !85, align: 64)
!85 = !DIDerivedType(tag: DW_TAG_variable, name: "__clang_vtable", scope: !15, file: !6, baseType: !53, flags: DIFlagPrivate | DIFlagArtificial | DIFlagStaticMember)
!86 = !DIGlobalVariableExpression(var: !87, expr: !DIExpression())
!87 = distinct !DIGlobalVariable(name: "table", linkageName: "_ZL5table", scope: !2, file: !6, line: 70, type: !88, isLocal: true, isDefinition: true)
!88 = !DICompositeType(tag: DW_TAG_array_type, baseType: !89, size: 128, elements: !93)
!89 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !90)
!90 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !91, size: 64)
!91 = !DISubroutineType(types: !92)
!92 = !{!9, !9}
!93 = !{!94}
!94 = !DISubrange(count: 2)
!95 = !DIGlobalVariableExpression(var: !96, expr: !DIExpression())
!96 = distinct !DIGlobalVariable(scope: null, file: !6, line: 17, type: !97, isLocal: true, isDefinition: true)
!97 = !DICompositeType(tag: DW_TAG_array_type, baseType: !39, size: 56, elements: !98)
!98 = !{!99}
!99 = !DISubrange(count: 7)
!100 = !{i32 7, !"Dwarf Version", i32 5}
!101 = !{i32 2, !"Debug Info Version", i32 3}
!102 = !{i32 2, !"debugger-tuning", !"gdb"}
!103 = !{i32 8, !"PIC Level", i32 2}
!104 = !{i32 7, !"PIE Level", i32 2}
!105 = !{i32 7, !"uwtable", i32 2}
!106 = !{i32 7, !"frame-pointer", i32 2}
!107 = !{!"clang version 24.0.0git (https://github.com/llvm/llvm-project.git c74b358f44c31e3615fe942556a41be8c9a8d877)"}
!108 = distinct !DISubprogram(name: "may_throw", linkageName: "_Z9may_throwi", scope: !6, file: !6, line: 39, type: !109, scopeLine: 39, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, retainedNodes: !111)
!109 = !DISubroutineType(types: !110)
!110 = !{null, !9}
!111 = !{}
!112 = !DILocalVariable(name: "x", arg: 1, scope: !108, file: !6, line: 39, type: !9)
!113 = !DILocation(line: 39, column: 46, scope: !108)
!114 = !DILocation(line: 40, column: 9, scope: !115)
!115 = distinct !DILexicalBlock(scope: !108, file: !6, line: 40, column: 9)
!116 = !DILocation(line: 40, column: 11, scope: !115)
!117 = !DILocation(line: 41, column: 9, scope: !115)
!118 = !DILocation(line: 41, column: 22, scope: !115)
!119 = !DILocation(line: 41, column: 23, scope: !115)
!120 = !DILocation(line: 42, column: 5, scope: !108)
!121 = !DILocation(line: 43, column: 1, scope: !108)
!122 = distinct !DISubprogram(name: "guarded", linkageName: "_Z7guardedi", scope: !6, file: !6, line: 45, type: !91, scopeLine: 45, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, retainedNodes: !111)
!123 = !DILocalVariable(name: "x", arg: 1, scope: !122, file: !6, line: 45, type: !9)
!124 = !DILocation(line: 45, column: 17, scope: !122)
!125 = !DILocalVariable(name: "s", scope: !126, file: !6, line: 47, type: !55)
!126 = distinct !DILexicalBlock(scope: !122, file: !6, line: 46, column: 9)
!127 = !DILocation(line: 47, column: 16, scope: !126)
!128 = !DILocation(line: 47, column: 18, scope: !126)
!129 = !DILocation(line: 48, column: 19, scope: !126)
!130 = !DILocation(line: 48, column: 9, scope: !126)
!131 = !DILocation(line: 49, column: 35, scope: !126)
!132 = !DILocation(line: 49, column: 33, scope: !126)
!133 = !DILocation(line: 49, column: 9, scope: !126)
!134 = !DILocation(line: 50, column: 5, scope: !122)
!135 = !DILocation(line: 55, column: 1, scope: !126)
!136 = !DILocation(line: 50, column: 5, scope: !126)
!137 = !DILocalVariable(name: "f", scope: !122, file: !6, line: 50, type: !138)
!138 = !DIDerivedType(tag: DW_TAG_reference_type, baseType: !139, size: 64)
!139 = !DIDerivedType(tag: DW_TAG_const_type, baseType: !5)
!140 = !DILocation(line: 50, column: 29, scope: !122)
!141 = !DILocation(line: 50, column: 32, scope: !122)
!142 = !DILocation(line: 51, column: 17, scope: !143)
!143 = distinct !DILexicalBlock(scope: !122, file: !6, line: 50, column: 32)
!144 = !{i64 4}
!145 = !DILocation(line: 51, column: 19, scope: !143)
!146 = !DILocation(line: 51, column: 16, scope: !143)
!147 = !DILocation(line: 51, column: 9, scope: !143)
!148 = !DILocation(line: 52, column: 5, scope: !143)
!149 = !DILocation(line: 52, column: 19, scope: !122)
!150 = !DILocation(line: 53, column: 9, scope: !151)
!151 = distinct !DILexicalBlock(scope: !122, file: !6, line: 52, column: 19)
!152 = !DILocation(line: 54, column: 5, scope: !151)
!153 = !DILocation(line: 55, column: 1, scope: !122)
!154 = distinct !DISubprogram(name: "Square", linkageName: "_ZN6SquareC2Ed", scope: !55, file: !6, line: 14, type: !61, scopeLine: 14, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !60, retainedNodes: !111)
!155 = !DILocalVariable(name: "this", arg: 1, scope: !154, type: !156, flags: DIFlagArtificial | DIFlagObjectPointer)
!156 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !55, size: 64)
!157 = !DILocation(line: 0, scope: !154)
!158 = !DILocalVariable(name: "s", arg: 2, scope: !154, file: !6, line: 14, type: !32)
!159 = !DILocation(line: 14, column: 28, scope: !154)
!160 = !DILocation(line: 14, column: 14, scope: !154)
!161 = !DILocation(line: 14, column: 41, scope: !154)
!162 = !DILocation(line: 14, column: 33, scope: !154)
!163 = !DILocation(line: 14, column: 38, scope: !154)
!164 = !DILocation(line: 14, column: 42, scope: !154)
!165 = distinct !DISubprogram(name: "area", linkageName: "_ZNK6Square4areaEv", scope: !55, file: !6, line: 16, type: !68, scopeLine: 16, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !67, retainedNodes: !111)
!166 = !DILocalVariable(name: "this", arg: 1, scope: !165, type: !167, flags: DIFlagArtificial | DIFlagObjectPointer)
!167 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !71, size: 64)
!168 = !DILocation(line: 0, scope: !165)
!169 = !DILocation(line: 16, column: 43, scope: !165)
!170 = !DILocation(line: 16, column: 50, scope: !165)
!171 = !DILocation(line: 16, column: 48, scope: !165)
!172 = !DILocation(line: 16, column: 36, scope: !165)
!173 = distinct !DISubprogram(name: "~Square", linkageName: "_ZN6SquareD2Ev", scope: !55, file: !6, line: 15, type: !65, scopeLine: 15, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !64, retainedNodes: !111)
!174 = !DILocalVariable(name: "this", arg: 1, scope: !173, type: !156, flags: DIFlagArtificial | DIFlagObjectPointer)
!175 = !DILocation(line: 0, scope: !173)
!176 = !DILocation(line: 15, column: 24, scope: !173)
!177 = !DILocation(line: 15, column: 26, scope: !178)
!178 = distinct !DILexicalBlock(scope: !173, file: !6, line: 15, column: 24)
!179 = !DILocation(line: 15, column: 39, scope: !178)
!180 = !DILocation(line: 15, column: 39, scope: !173)
!181 = distinct !DISubprogram(name: "__cxx_global_var_init", scope: !6, file: !6, type: !182, flags: DIFlagArtificial, spFlags: DISPFlagLocalToUnit | DISPFlagDefinition, unit: !2)
!182 = !DISubroutineType(types: !183)
!183 = !{null}
!184 = !DILocation(line: 57, column: 15, scope: !181)
!185 = !DILocation(line: 0, scope: !181)
!186 = !DILocation(line: 57, column: 20, scope: !181)
!187 = distinct !DISubprogram(name: "Circle", linkageName: "_ZN6CircleC2Ed", scope: !15, file: !6, line: 25, type: !43, scopeLine: 25, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !42, retainedNodes: !111)
!188 = !DILocalVariable(name: "this", arg: 1, scope: !187, type: !189, flags: DIFlagArtificial | DIFlagObjectPointer)
!189 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !15, size: 64)
!190 = !DILocation(line: 0, scope: !187)
!191 = !DILocalVariable(name: "r", arg: 2, scope: !187, file: !6, line: 25, type: !32)
!192 = !DILocation(line: 25, column: 28, scope: !187)
!193 = !DILocation(line: 25, column: 14, scope: !187)
!194 = !DILocation(line: 25, column: 43, scope: !187)
!195 = !DILocation(line: 25, column: 33, scope: !187)
!196 = !DILocation(line: 25, column: 40, scope: !187)
!197 = !DILocation(line: 25, column: 44, scope: !187)
!198 = distinct !DISubprogram(name: "~Circle", linkageName: "_ZN6CircleD2Ev", scope: !15, file: !6, line: 23, type: !199, scopeLine: 23, flags: DIFlagArtificial | DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !201, retainedNodes: !111)
!199 = !DISubroutineType(types: !200)
!200 = !{null, !45}
!201 = !DISubprogram(name: "~Circle", linkageName: "_ZN6CircleD4Ev", scope: !15, type: !199, containingType: !15, virtualIndex: 0, flags: DIFlagArtificial | DIFlagPrototyped, spFlags: DISPFlagVirtual)
!202 = !DILocalVariable(name: "this", arg: 1, scope: !198, type: !189, flags: DIFlagArtificial | DIFlagObjectPointer)
!203 = !DILocation(line: 0, scope: !198)
!204 = !DILocation(line: 23, column: 8, scope: !205)
!205 = distinct !DILexicalBlock(scope: !198, file: !6, line: 23, column: 8)
!206 = !DILocation(line: 23, column: 8, scope: !198)
!207 = distinct !DISubprogram(name: "total", linkageName: "_Z5totalPP5Shapei", scope: !6, file: !6, line: 59, type: !208, scopeLine: 59, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, retainedNodes: !111)
!208 = !DISubroutineType(types: !209)
!209 = !{!32, !210, !9}
!210 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !211, size: 64)
!211 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !18, size: 64)
!212 = !DILocalVariable(name: "shapes", arg: 1, scope: !207, file: !6, line: 59, type: !210)
!213 = !DILocation(line: 59, column: 22, scope: !207)
!214 = !DILocalVariable(name: "n", arg: 2, scope: !207, file: !6, line: 59, type: !9)
!215 = !DILocation(line: 59, column: 34, scope: !207)
!216 = !DILocalVariable(name: "t", scope: !207, file: !6, line: 60, type: !32)
!217 = !DILocation(line: 60, column: 12, scope: !207)
!218 = !DILocalVariable(name: "i", scope: !219, file: !6, line: 61, type: !9)
!219 = distinct !DILexicalBlock(scope: !207, file: !6, line: 61, column: 5)
!220 = !DILocation(line: 61, column: 14, scope: !219)
!221 = !DILocation(line: 61, column: 10, scope: !219)
!222 = !DILocation(line: 61, column: 21, scope: !223)
!223 = distinct !DILexicalBlock(scope: !219, file: !6, line: 61, column: 5)
!224 = !DILocation(line: 61, column: 25, scope: !223)
!225 = !DILocation(line: 61, column: 23, scope: !223)
!226 = !DILocation(line: 61, column: 5, scope: !219)
!227 = !DILocation(line: 62, column: 14, scope: !223)
!228 = !DILocation(line: 62, column: 21, scope: !223)
!229 = !DILocation(line: 62, column: 25, scope: !223)
!230 = !DILocation(line: 62, column: 11, scope: !223)
!231 = !DILocation(line: 62, column: 9, scope: !223)
!232 = !DILocation(line: 61, column: 28, scope: !223)
!233 = !DILocation(line: 61, column: 5, scope: !223)
!234 = distinct !{!234, !226, !235, !236}
!235 = !DILocation(line: 62, column: 30, scope: !219)
!236 = !{!"llvm.loop.mustprogress"}
!237 = !DILocation(line: 63, column: 12, scope: !207)
!238 = !DILocation(line: 63, column: 5, scope: !207)
!239 = distinct !DISubprogram(name: "twice", linkageName: "_ZN6detail5twiceEi", scope: !240, file: !6, line: 67, type: !91, scopeLine: 67, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, retainedNodes: !111)
!240 = !DINamespace(name: "detail", scope: null)
!241 = !DILocalVariable(name: "x", arg: 1, scope: !239, file: !6, line: 67, type: !9)
!242 = !DILocation(line: 67, column: 15, scope: !239)
!243 = !DILocation(line: 67, column: 27, scope: !239)
!244 = !DILocation(line: 67, column: 29, scope: !239)
!245 = !DILocation(line: 67, column: 20, scope: !239)
!246 = distinct !DISubprogram(name: "shapes_main", scope: !6, file: !6, line: 72, type: !91, scopeLine: 72, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, retainedNodes: !111)
!247 = !DILocalVariable(name: "argc", arg: 1, scope: !246, file: !6, line: 72, type: !9)
!248 = !DILocation(line: 72, column: 32, scope: !246)
!249 = !DILocalVariable(name: "sq", scope: !246, file: !6, line: 73, type: !55)
!250 = !DILocation(line: 73, column: 12, scope: !246)
!251 = !DILocation(line: 73, column: 15, scope: !246)
!252 = !DILocalVariable(name: "list", scope: !246, file: !6, line: 74, type: !253)
!253 = !DICompositeType(tag: DW_TAG_array_type, baseType: !211, size: 128, elements: !93)
!254 = !DILocation(line: 74, column: 12, scope: !246)
!255 = !DILocation(line: 74, column: 21, scope: !246)
!256 = !DILocation(line: 75, column: 49, scope: !246)
!257 = !DILocation(line: 75, column: 43, scope: !246)
!258 = !DILocation(line: 75, column: 12, scope: !246)
!259 = !DILocation(line: 76, column: 44, scope: !246)
!260 = !DILocation(line: 76, column: 29, scope: !246)
!261 = !DILocation(line: 75, column: 68, scope: !246)
!262 = !DILocation(line: 77, column: 18, scope: !246)
!263 = !DILocation(line: 77, column: 23, scope: !246)
!264 = !DILocation(line: 77, column: 12, scope: !246)
!265 = !DILocation(line: 77, column: 28, scope: !246)
!266 = !DILocation(line: 76, column: 57, scope: !246)
!267 = !DILocation(line: 77, column: 40, scope: !246)
!268 = !DILocation(line: 77, column: 37, scope: !246)
!269 = !DILocation(line: 77, column: 50, scope: !246)
!270 = !DILocation(line: 77, column: 36, scope: !246)
!271 = !DILocation(line: 77, column: 34, scope: !246)
!272 = !DILocation(line: 78, column: 1, scope: !246)
!273 = distinct !DISubprogram(name: "clamp_to<int>", linkageName: "_Z8clamp_toIiET_S0_S0_S0_", scope: !6, file: !6, line: 29, type: !274, scopeLine: 29, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, templateParams: !276, retainedNodes: !111)
!274 = !DISubroutineType(types: !275)
!275 = !{!9, !9, !9, !9}
!276 = !{!277}
!277 = !DITemplateTypeParameter(name: "T", type: !9)
!278 = !DILocalVariable(name: "v", arg: 1, scope: !273, file: !6, line: 29, type: !9)
!279 = !DILocation(line: 29, column: 36, scope: !273)
!280 = !DILocalVariable(name: "lo", arg: 2, scope: !273, file: !6, line: 29, type: !9)
!281 = !DILocation(line: 29, column: 41, scope: !273)
!282 = !DILocalVariable(name: "hi", arg: 3, scope: !273, file: !6, line: 29, type: !9)
!283 = !DILocation(line: 29, column: 47, scope: !273)
!284 = !DILocation(line: 30, column: 12, scope: !273)
!285 = !DILocation(line: 30, column: 16, scope: !273)
!286 = !DILocation(line: 30, column: 14, scope: !273)
!287 = !DILocation(line: 30, column: 21, scope: !273)
!288 = !DILocation(line: 30, column: 27, scope: !273)
!289 = !DILocation(line: 30, column: 31, scope: !273)
!290 = !DILocation(line: 30, column: 29, scope: !273)
!291 = !DILocation(line: 30, column: 36, scope: !273)
!292 = !DILocation(line: 30, column: 41, scope: !273)
!293 = !DILocation(line: 30, column: 5, scope: !273)
!294 = distinct !DISubprogram(name: "clamp_to<long>", linkageName: "_Z8clamp_toIlET_S0_S0_S0_", scope: !6, file: !6, line: 29, type: !295, scopeLine: 29, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, templateParams: !298, retainedNodes: !111)
!295 = !DISubroutineType(types: !296)
!296 = !{!297, !297, !297, !297}
!297 = !DIBasicType(name: "long", size: 64, encoding: DW_ATE_signed)
!298 = !{!299}
!299 = !DITemplateTypeParameter(name: "T", type: !297)
!300 = !DILocalVariable(name: "v", arg: 1, scope: !294, file: !6, line: 29, type: !297)
!301 = !DILocation(line: 29, column: 36, scope: !294)
!302 = !DILocalVariable(name: "lo", arg: 2, scope: !294, file: !6, line: 29, type: !297)
!303 = !DILocation(line: 29, column: 41, scope: !294)
!304 = !DILocalVariable(name: "hi", arg: 3, scope: !294, file: !6, line: 29, type: !297)
!305 = !DILocation(line: 29, column: 47, scope: !294)
!306 = !DILocation(line: 30, column: 12, scope: !294)
!307 = !DILocation(line: 30, column: 16, scope: !294)
!308 = !DILocation(line: 30, column: 14, scope: !294)
!309 = !DILocation(line: 30, column: 21, scope: !294)
!310 = !DILocation(line: 30, column: 27, scope: !294)
!311 = !DILocation(line: 30, column: 31, scope: !294)
!312 = !DILocation(line: 30, column: 29, scope: !294)
!313 = !DILocation(line: 30, column: 36, scope: !294)
!314 = !DILocation(line: 30, column: 41, scope: !294)
!315 = !DILocation(line: 30, column: 5, scope: !294)
!316 = distinct !DISubprogram(name: "name", linkageName: "_ZNK6Square4nameEv", scope: !55, file: !6, line: 17, type: !73, scopeLine: 17, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !72, retainedNodes: !111)
!317 = !DILocalVariable(name: "this", arg: 1, scope: !316, type: !167, flags: DIFlagArtificial | DIFlagObjectPointer)
!318 = !DILocation(line: 0, scope: !316)
!319 = !DILocation(line: 17, column: 41, scope: !316)
!320 = distinct !DISubprogram(name: "Shape", linkageName: "_ZN5ShapeC2Ev", scope: !18, file: !6, line: 6, type: !26, scopeLine: 6, flags: DIFlagArtificial | DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !321, retainedNodes: !111)
!321 = !DISubprogram(name: "Shape", linkageName: "_ZN5ShapeC4Ev", scope: !18, type: !26, flags: DIFlagArtificial | DIFlagPrototyped, spFlags: 0)
!322 = !DILocalVariable(name: "this", arg: 1, scope: !320, type: !211, flags: DIFlagArtificial | DIFlagObjectPointer)
!323 = !DILocation(line: 0, scope: !320)
!324 = !DILocation(line: 6, column: 8, scope: !320)
!325 = distinct !DISubprogram(name: "~Square", linkageName: "_ZN6SquareD0Ev", scope: !55, file: !6, line: 15, type: !65, scopeLine: 15, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !64, retainedNodes: !111)
!326 = !DILocalVariable(name: "this", arg: 1, scope: !325, type: !156, flags: DIFlagArtificial | DIFlagObjectPointer)
!327 = !DILocation(line: 0, scope: !325)
!328 = !DILocation(line: 15, column: 24, scope: !325)
!329 = !DILocation(line: 15, column: 39, scope: !325)
!330 = distinct !DISubprogram(name: "~Shape", linkageName: "_ZN5ShapeD2Ev", scope: !18, file: !6, line: 7, type: !26, scopeLine: 7, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !25, retainedNodes: !111)
!331 = !DILocalVariable(name: "this", arg: 1, scope: !330, type: !211, flags: DIFlagArtificial | DIFlagObjectPointer)
!332 = !DILocation(line: 0, scope: !330)
!333 = !DILocation(line: 7, column: 23, scope: !330)
!334 = distinct !DISubprogram(name: "~Shape", linkageName: "_ZN5ShapeD0Ev", scope: !18, file: !6, line: 7, type: !26, scopeLine: 7, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !25, retainedNodes: !111)
!335 = !DILocalVariable(name: "this", arg: 1, scope: !334, type: !211, flags: DIFlagArtificial | DIFlagObjectPointer)
!336 = !DILocation(line: 0, scope: !334)
!337 = !DILocation(line: 7, column: 22, scope: !334)
!338 = distinct !DISubprogram(name: "name", linkageName: "_ZNK5Shape4nameEv", scope: !18, file: !6, line: 9, type: !36, scopeLine: 9, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !35, retainedNodes: !111)
!339 = !DILocalVariable(name: "this", arg: 1, scope: !338, type: !340, flags: DIFlagArtificial | DIFlagObjectPointer)
!340 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !34, size: 64)
!341 = !DILocation(line: 0, scope: !338)
!342 = !DILocation(line: 9, column: 40, scope: !338)
!343 = distinct !DISubprogram(name: "~Circle", linkageName: "_ZN6CircleD0Ev", scope: !15, file: !6, line: 23, type: !199, scopeLine: 23, flags: DIFlagArtificial | DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !201, retainedNodes: !111)
!344 = !DILocalVariable(name: "this", arg: 1, scope: !343, type: !189, flags: DIFlagArtificial | DIFlagObjectPointer)
!345 = !DILocation(line: 0, scope: !343)
!346 = !DILocation(line: 23, column: 8, scope: !343)
!347 = distinct !DISubprogram(name: "area", linkageName: "_ZNK6Circle4areaEv", scope: !15, file: !6, line: 26, type: !47, scopeLine: 26, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !2, declaration: !46, retainedNodes: !111)
!348 = !DILocalVariable(name: "this", arg: 1, scope: !347, type: !349, flags: DIFlagArtificial | DIFlagObjectPointer)
!349 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !50, size: 64)
!350 = !DILocation(line: 0, scope: !347)
!351 = !DILocation(line: 26, column: 53, scope: !347)
!352 = !DILocation(line: 26, column: 51, scope: !347)
!353 = !DILocation(line: 26, column: 62, scope: !347)
!354 = !DILocation(line: 26, column: 60, scope: !347)
!355 = !DILocation(line: 26, column: 36, scope: !347)
!356 = distinct !DISubprogram(linkageName: "_GLOBAL__sub_I_shapes.cpp", scope: !6, file: !6, type: !357, flags: DIFlagArtificial, spFlags: DISPFlagLocalToUnit | DISPFlagDefinition, unit: !2)
!357 = !DISubroutineType(types: !111)
!358 = !DILocation(line: 0, scope: !356)
