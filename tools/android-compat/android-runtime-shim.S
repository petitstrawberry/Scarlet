// Android's framework power-hint client waits for a device Power HAL. Scarlet
// owns suspend and power policy outside Android, so report that optional hint
// API as unsupported without blocking system_server startup.

.text
.p2align 2
.global _ZN7android5power18PowerHalController8setBoostENS_8hardware5power5BoostEi
.type _ZN7android5power18PowerHalController8setBoostENS_8hardware5power5BoostEi,%function
_ZN7android5power18PowerHalController8setBoostENS_8hardware5power5BoostEi:
    stp xzr, xzr, [x8]
    stp xzr, xzr, [x8, #16]
    ret
.size _ZN7android5power18PowerHalController8setBoostENS_8hardware5power5BoostEi, .-_ZN7android5power18PowerHalController8setBoostENS_8hardware5power5BoostEi

.p2align 2
.global _ZN7android5power18PowerHalController7setModeENS_8hardware5power4ModeEb
.type _ZN7android5power18PowerHalController7setModeENS_8hardware5power4ModeEb,%function
_ZN7android5power18PowerHalController7setModeENS_8hardware5power4ModeEb:
    stp xzr, xzr, [x8]
    stp xzr, xzr, [x8, #16]
    ret
.size _ZN7android5power18PowerHalController7setModeENS_8hardware5power4ModeEb, .-_ZN7android5power18PowerHalController7setModeENS_8hardware5power4ModeEb

.p2align 2
.global _ZN7android21SurfaceComposerClient16notifyPowerBoostEi
.type _ZN7android21SurfaceComposerClient16notifyPowerBoostEi,%function
_ZN7android21SurfaceComposerClient16notifyPowerBoostEi:
    ret
.size _ZN7android21SurfaceComposerClient16notifyPowerBoostEi, .-_ZN7android21SurfaceComposerClient16notifyPowerBoostEi
