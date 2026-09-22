; ModuleID = 'caller.6e0b9be8a6fe9f88-cgu.0'
source_filename = "caller.6e0b9be8a6fe9f88-cgu.0"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128-Fn32"
target triple = "aarch64-unknown-scarlet"

; Function Attrs: noredzone nounwind uwtable
define noundef zeroext i1 @overflow_check() unnamed_addr #0 {
start:
  %_1 = tail call noundef ptr @calloc(i64 noundef -1, i64 noundef 2) #2
  %_0 = icmp eq ptr %_1, null
  ret i1 %_0
}

; Function Attrs: noinline noredzone nounwind uwtable
declare noundef ptr @calloc(i64 noundef, i64 noundef) unnamed_addr #1

attributes #0 = { noredzone nounwind uwtable "probe-stack"="inline-asm" "target-cpu"="generic" "target-features"="+v8a,+outline-atomics,+strict-align,+neon,+fp-armv8" }
attributes #1 = { noinline noredzone nounwind uwtable "no-builtins" "probe-stack"="inline-asm" "target-cpu"="generic" "target-features"="+v8a,+outline-atomics,+strict-align,+neon,+fp-armv8" }
attributes #2 = { noinline nounwind }

!llvm.module.flags = !{!0}
!llvm.ident = !{!1}

!0 = !{i32 8, !"PIC Level", i32 2}
!1 = !{!"rustc version 1.94.0-nightly (39c689a48 2026-09-20)"}
