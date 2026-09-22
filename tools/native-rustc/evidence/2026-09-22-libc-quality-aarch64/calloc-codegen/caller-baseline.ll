; ModuleID = 'caller.6e0b9be8a6fe9f88-cgu.0'
source_filename = "caller.6e0b9be8a6fe9f88-cgu.0"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128-Fn32"
target triple = "aarch64-unknown-scarlet"

; Function Attrs: mustprogress nofree norecurse noredzone nosync nounwind willreturn memory(none) uwtable
define noundef zeroext i1 @overflow_check() unnamed_addr #0 {
start:
  ret i1 false
}

attributes #0 = { mustprogress nofree norecurse noredzone nosync nounwind willreturn memory(none) uwtable "probe-stack"="inline-asm" "target-cpu"="generic" "target-features"="+v8a,+outline-atomics,+strict-align,+neon,+fp-armv8" }

!llvm.module.flags = !{!0}
!llvm.ident = !{!1}

!0 = !{i32 8, !"PIC Level", i32 2}
!1 = !{!"rustc version 1.94.0-nightly (39c689a48 2026-09-20)"}
