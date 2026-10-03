//! Native IEEE f32/f64 math adapters to the Rust libm 0.2.16.
#[unsafe(no_mangle)]
pub extern "C" fn acos(x: f64) -> f64 { libm::acos(x) }
#[unsafe(no_mangle)]
pub extern "C" fn acosf(x: f32) -> f32 { libm::acosf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn asin(x: f64) -> f64 { libm::asin(x) }
#[unsafe(no_mangle)]
pub extern "C" fn asinf(x: f32) -> f32 { libm::asinf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn atan(x: f64) -> f64 { libm::atan(x) }
#[unsafe(no_mangle)]
pub extern "C" fn atanf(x: f32) -> f32 { libm::atanf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn acosh(x: f64) -> f64 { libm::acosh(x) }
#[unsafe(no_mangle)]
pub extern "C" fn acoshf(x: f32) -> f32 { libm::acoshf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn asinh(x: f64) -> f64 { libm::asinh(x) }
#[unsafe(no_mangle)]
pub extern "C" fn asinhf(x: f32) -> f32 { libm::asinhf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn atanh(x: f64) -> f64 { libm::atanh(x) }
#[unsafe(no_mangle)]
pub extern "C" fn atanhf(x: f32) -> f32 { libm::atanhf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn cos(x: f64) -> f64 { libm::cos(x) }
#[unsafe(no_mangle)]
pub extern "C" fn cosf(x: f32) -> f32 { libm::cosf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn sin(x: f64) -> f64 { libm::sin(x) }
#[unsafe(no_mangle)]
pub extern "C" fn sinf(x: f32) -> f32 { libm::sinf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn tan(x: f64) -> f64 { libm::tan(x) }
#[unsafe(no_mangle)]
pub extern "C" fn tanf(x: f32) -> f32 { libm::tanf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn cosh(x: f64) -> f64 { libm::cosh(x) }
#[unsafe(no_mangle)]
pub extern "C" fn coshf(x: f32) -> f32 { libm::coshf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn sinh(x: f64) -> f64 { libm::sinh(x) }
#[unsafe(no_mangle)]
pub extern "C" fn sinhf(x: f32) -> f32 { libm::sinhf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn tanh(x: f64) -> f64 { libm::tanh(x) }
#[unsafe(no_mangle)]
pub extern "C" fn tanhf(x: f32) -> f32 { libm::tanhf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn exp(x: f64) -> f64 { libm::exp(x) }
#[unsafe(no_mangle)]
pub extern "C" fn expf(x: f32) -> f32 { libm::expf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn exp2(x: f64) -> f64 { libm::exp2(x) }
#[unsafe(no_mangle)]
pub extern "C" fn exp2f(x: f32) -> f32 { libm::exp2f(x) }
#[unsafe(no_mangle)]
pub extern "C" fn expm1(x: f64) -> f64 { libm::expm1(x) }
#[unsafe(no_mangle)]
pub extern "C" fn expm1f(x: f32) -> f32 { libm::expm1f(x) }
#[unsafe(no_mangle)]
pub extern "C" fn log(x: f64) -> f64 { libm::log(x) }
#[unsafe(no_mangle)]
pub extern "C" fn logf(x: f32) -> f32 { libm::logf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn log2(x: f64) -> f64 { libm::log2(x) }
#[unsafe(no_mangle)]
pub extern "C" fn log2f(x: f32) -> f32 { libm::log2f(x) }
#[unsafe(no_mangle)]
pub extern "C" fn log10(x: f64) -> f64 { libm::log10(x) }
#[unsafe(no_mangle)]
pub extern "C" fn log10f(x: f32) -> f32 { libm::log10f(x) }
#[unsafe(no_mangle)]
pub extern "C" fn log1p(x: f64) -> f64 { libm::log1p(x) }
#[unsafe(no_mangle)]
pub extern "C" fn log1pf(x: f32) -> f32 { libm::log1pf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn sqrt(x: f64) -> f64 { libm::sqrt(x) }
#[unsafe(no_mangle)]
pub extern "C" fn sqrtf(x: f32) -> f32 { libm::sqrtf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn cbrt(x: f64) -> f64 { libm::cbrt(x) }
#[unsafe(no_mangle)]
pub extern "C" fn cbrtf(x: f32) -> f32 { libm::cbrtf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn ceil(x: f64) -> f64 { libm::ceil(x) }
#[unsafe(no_mangle)]
pub extern "C" fn ceilf(x: f32) -> f32 { libm::ceilf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn floor(x: f64) -> f64 { libm::floor(x) }
#[unsafe(no_mangle)]
pub extern "C" fn floorf(x: f32) -> f32 { libm::floorf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn trunc(x: f64) -> f64 { libm::trunc(x) }
#[unsafe(no_mangle)]
pub extern "C" fn truncf(x: f32) -> f32 { libm::truncf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn round(x: f64) -> f64 { libm::round(x) }
#[unsafe(no_mangle)]
pub extern "C" fn roundf(x: f32) -> f32 { libm::roundf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn rint(x: f64) -> f64 { libm::rint(x) }
#[unsafe(no_mangle)]
pub extern "C" fn rintf(x: f32) -> f32 { libm::rintf(x) }
#[unsafe(no_mangle)]
pub extern "C" fn atan2(x: f64, y: f64) -> f64 { libm::atan2(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn atan2f(x: f32, y: f32) -> f32 { libm::atan2f(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn pow(x: f64, y: f64) -> f64 { libm::pow(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn powf(x: f32, y: f32) -> f32 { libm::powf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn hypot(x: f64, y: f64) -> f64 { libm::hypot(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn hypotf(x: f32, y: f32) -> f32 { libm::hypotf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn fmod(x: f64, y: f64) -> f64 { libm::fmod(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn fmodf(x: f32, y: f32) -> f32 { libm::fmodf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn remainder(x: f64, y: f64) -> f64 { libm::remainder(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn remainderf(x: f32, y: f32) -> f32 { libm::remainderf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn copysign(x: f64, y: f64) -> f64 { libm::copysign(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn copysignf(x: f32, y: f32) -> f32 { libm::copysignf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn nextafter(x: f64, y: f64) -> f64 { libm::nextafter(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn nextafterf(x: f32, y: f32) -> f32 { libm::nextafterf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn fdim(x: f64, y: f64) -> f64 { libm::fdim(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn fdimf(x: f32, y: f32) -> f32 { libm::fdimf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn fmax(x: f64, y: f64) -> f64 { libm::fmax(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn fmaxf(x: f32, y: f32) -> f32 { libm::fmaxf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn fmin(x: f64, y: f64) -> f64 { libm::fmin(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn fminf(x: f32, y: f32) -> f32 { libm::fminf(x,y) }
#[unsafe(no_mangle)]
pub extern "C" fn ldexp(x:f64, e:i32)->f64 { libm::ldexp(x,e) }
#[unsafe(no_mangle)]
pub extern "C" fn ldexpf(x:f32, e:i32)->f32 { libm::ldexpf(x,e) }
#[unsafe(no_mangle)]
pub extern "C" fn scalbn(x:f64, e:i32)->f64 { libm::scalbn(x,e) }
#[unsafe(no_mangle)]
pub extern "C" fn scalbnf(x:f32, e:i32)->f32 { libm::scalbnf(x,e) }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn frexp(x:f64, out:*mut i32)->f64 { let (fraction,whole)=libm::frexp(x); unsafe { *out=whole; } fraction }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn frexpf(x:f32, out:*mut i32)->f32 { let (fraction,whole)=libm::frexpf(x); unsafe { *out=whole; } fraction }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn modf(x:f64, out:*mut f64)->f64 { let (fraction,whole)=libm::modf(x); unsafe { *out=whole; } fraction }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn modff(x:f32, out:*mut f32)->f32 { let (fraction,whole)=libm::modff(x); unsafe { *out=whole; } fraction }
