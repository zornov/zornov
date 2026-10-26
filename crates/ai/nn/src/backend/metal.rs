// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Metal backend: hand-written Objective-C runtime FFI (no third-party
//! crates) driving compute kernels compiled from MSL source at runtime.
//!
//! What runs on the GPU (everything else returns `None` and falls back to the
//! CPU kernel for that op):
//! * `MatMul` (any rank, batch dims broadcast, 1-D operands) and `Gemm`
//!   (transA / transB / alpha / beta / bias C broadcast as scalar, row,
//!   column or full matrix): a 64×64-tile GEMM built on `simdgroup_float8x8`
//!   (Apple7+ GPUs), with a plain 16×16 tiled kernel for skinny problems
//!   (M or N < 32) and GPUs without simdgroup matrices. Edges are handled by
//!   zero-padded tile loads and bounds-checked stores, so any size works;
//!   row-major operands with K, N and strides divisible by 4 take a float4
//!   load path. The next K-slab is prefetched into registers while the
//!   current one is multiplied.
//! * `Conv` 1-D / 2-D (stride, padding incl. `auto_pad`, dilation, groups) as
//!   an implicit GEMM: the im2col matrix is never materialised, the B-tile
//!   loader gathers input pixels directly. Pointwise (1×1, stride 1, no pad,
//!   group 1) convs run as a plain batched GEMM. Depthwise convs decline (CPU).
//! * Element-wise `Add` / `Sub` / `Mul` / `Div` (f32, same shape or operands
//!   whose non-broadcast dims form one contiguous block: scalar, row, channel
//!   bias, outer products) and `Relu`, `Sigmoid`, `Tanh`, `Gelu` (erf or
//!   tanh), `Erf`, `LeakyRelu`, `Clip`.
//!
//! Set `STUDIO_METAL_DEBUG=1` to log compile errors, failures and per-dispatch
//! GPU times to stderr; `STUDIO_NO_METAL=1` disables the device and
//! `STUDIO_METAL_NO_SIMDGROUP=1` forces the plain tiled GEMM.
//!
//! Ops below `min_flops` (GEMM / conv FLOPs, element count for element-wise
//! ops) decline. Every Metal failure (nil object, compile error, command
//! buffer error) also declines, so the CPU always produces a result; nothing
//! here panics.
//!
//! Memory: Apple silicon has unified memory, so all buffers use shared
//! storage. Large page-aligned host vectors (outputs are allocated that way)
//! are wrapped with `newBufferWithBytesNoCopy`; everything else is copied into
//! pooled scratch buffers. Each op is one command buffer, waited on
//! synchronously; a mutex around the command queue serialises submissions.
//!
//! Weight cache: constant weights (MatMul / Gemm B, Conv W) are uploaded once
//! per [`MetalBackend`] and reused. An operand is only treated as constant
//! when the session handed the op a pre-packed weight (`ctx.cache`), which it
//! does only for session constants; the cache lives in the backend, which the
//! session owns, so cached constants outlive their entries and their storage
//! never moves. The key is (data pointer, byte length, FNV hash of sampled
//! bytes), the hash guarding against a pointer being reused by different data
//! if the backend were ever shared beyond one session's constants.

use super::Backend;
use crate::error::Result;
use crate::onnx::Node;
use crate::ops::elementwise::{broadcast_shape, bstrides};
use crate::ops::{OpCtx, Prepacked};
use crate::tensor::{Data, Tensor, numel};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Mutex, OnceLock};

// ------------------------------------------------------------------ FFI

type Id = *mut c_void;
type Sel = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct MtlSize {
    w: usize,
    h: usize,
    d: usize,
}

fn mtl_size(w: usize, h: usize, d: usize) -> MtlSize {
    MtlSize { w, h, d }
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

unsafe extern "C" {
    fn getpagesize() -> i32;
}

/// `msg!(receiver, "selector:", arg => Type, ...; ReturnType)` — calls
/// `objc_msgSend` cast to the exact C signature.
macro_rules! msg {
    ($obj:expr, $sel:literal $(, $arg:expr => $t:ty)* ; $ret:ty) => {{
        let receiver: Id = $obj;
        // SAFETY: objc_msgSend is called through a pointer of the method's
        // exact C ABI signature, which is how the Objective-C runtime expects
        // to be called on arm64 / x86_64 (no struct returns are used).
        let f = unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret>(
                objc_msgSend as unsafe extern "C" fn(),
            )
        };
        unsafe { f(receiver, sel(concat!($sel, "\0")) $(, $arg)*) }
    }};
}

/// `name` must be NUL-terminated.
fn sel(name: &str) -> Sel {
    debug_assert!(name.ends_with('\0'));
    unsafe { sel_registerName(name.as_ptr() as *const c_char) }
}

fn class(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

/// An owned (+1) Objective-C object reference, released on drop.
struct Obj(Id);

// SAFETY: only Metal objects documented as thread-safe (device, pipeline
// states, buffers, command queue) are wrapped, and the queue is only used
// under a mutex.
unsafe impl Send for Obj {}
unsafe impl Sync for Obj {}

impl Obj {
    /// Takes ownership of a +1 reference (`new…` / `alloc` methods).
    fn owned(id: Id) -> Option<Obj> {
        (!id.is_null()).then_some(Obj(id))
    }
    fn id(&self) -> Id {
        self.0
    }
}

impl Clone for Obj {
    fn clone(&self) -> Obj {
        let _: Id = msg!(self.0, "retain"; Id);
        Obj(self.0)
    }
}

impl Drop for Obj {
    fn drop(&mut self) {
        msg!(self.0, "release"; ());
    }
}

/// An autorelease pool scope.
struct ArPool(*mut c_void);

impl ArPool {
    fn new() -> ArPool {
        ArPool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for ArPool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

/// Autoreleased NSString (call inside an [`ArPool`]).
fn ns_string(s: &str) -> Id {
    let Ok(c) = CString::new(s) else { return std::ptr::null_mut() };
    msg!(class(c"NSString"), "stringWithUTF8String:", c.as_ptr() => *const c_char; Id)
}

fn rust_string(ns: Id) -> String {
    if ns.is_null() {
        return String::new();
    }
    let p = msg!(ns, "UTF8String"; *const c_char);
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

fn error_text(err: Id) -> String {
    if err.is_null() { "unknown error".into() } else { rust_string(msg!(err, "localizedDescription"; Id)) }
}

fn responds(obj: Id, name: &'static str) -> bool {
    msg!(obj, "respondsToSelector:", sel(name) => Sel; i8) != 0
}

fn debug_enabled() -> bool {
    static D: OnceLock<bool> = OnceLock::new();
    *D.get_or_init(|| std::env::var_os("STUDIO_METAL_DEBUG").is_some())
}

fn debug(msg: impl FnOnce() -> String) {
    if debug_enabled() {
        eprintln!("[studio-nn metal] {}", msg());
    }
}

fn round_up(x: usize, to: usize) -> usize {
    x.div_ceil(to) * to
}

/// Every index the kernels compute is a 32-bit `uint`; stay well inside it.
fn u(x: usize) -> Option<u32> {
    if x < (1usize << 31) { Some(x as u32) } else { None }
}

fn bytes_of<T: Copy>(t: &T) -> &[u8] {
    // SAFETY: T is a plain `repr(C)` struct of 4-byte scalars (no padding).
    unsafe { std::slice::from_raw_parts(t as *const T as *const u8, std::mem::size_of::<T>()) }
}

// ------------------------------------------------------------------ kernels

const COMMON_SRC: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct GP {
    uint M, N, K, a_rs, a_cs, b_rs, b_cs, ldc, batch_c;
    float alpha, beta;
    uint bias_mode, use_offs, group, ocg, icg, c_in, c_out;
    uint ih, iw, oh, ow, kh, kw, sh, sw, dh, dw, ph, pw;
};

struct Bases { uint a; uint b; uint c; uint bias; };

template <bool CONV>
inline Bases bases_for(constant GP& p, device const uint2* offs, uint z) {
    Bases r;
    if (CONV) {
        uint n = z / p.group;
        uint g = z - n * p.group;
        r.a = g * p.ocg * p.K;
        r.b = (n * p.c_in + g * p.icg) * p.ih * p.iw;
        r.c = (n * p.c_out + g * p.ocg) * p.N;
        r.bias = g * p.ocg;
    } else {
        uint2 o = p.use_offs != 0 ? offs[z] : uint2(0, 0);
        r.a = o.x;
        r.b = o.y;
        r.c = z * p.batch_c;
        r.bias = 0;
    }
    return r;
}

struct ConvCol { int iy0; int ix0; };

inline ConvCol conv_col(constant GP& p, uint n) {
    ConvCol c;
    uint nn = n < p.N ? n : 0;
    uint oy = nn / p.ow;
    uint ox = nn - oy * p.ow;
    c.iy0 = int(oy * p.sh) - int(p.ph);
    c.ix0 = int(ox * p.sw) - int(p.pw);
    return c;
}

// Implicit im2col: element (k, column) of the unfolded input.
inline float conv_load(device const float* X, constant GP& p, uint base, ConvCol c, uint k) {
    uint kk = p.kh * p.kw;
    uint ic = k / kk;
    uint r = k - ic * kk;
    uint ky = r / p.kw;
    uint kx = r - ky * p.kw;
    int iy = c.iy0 + int(ky * p.dh);
    int ix = c.ix0 + int(kx * p.dw);
    if (iy < 0 || ix < 0 || iy >= int(p.ih) || ix >= int(p.iw)) return 0.0f;
    return X[base + ic * p.ih * p.iw + uint(iy) * p.iw + uint(ix)];
}

inline float epilogue(constant GP& p, device const float* bias, uint bb, float acc, uint m, uint n) {
    float v = acc * p.alpha;
    if (p.bias_mode == 1) v += p.beta * bias[0];
    else if (p.bias_mode == 2) v += p.beta * bias[n];
    else if (p.bias_mode == 3) v += p.beta * bias[bb + m];
    else if (p.bias_mode == 4) v += p.beta * bias[m * p.N + n];
    return v;
}
"#;

const BASE_SRC: &str = r#"
template <bool CONV>
inline void tiled_impl(device const float* A, device const float* B, device float* C, device const float* bias,
                       device const uint2* offs, constant GP& p, threadgroup float* As, threadgroup float* Bs,
                       uint3 tg, uint3 lid) {
    Bases bs = bases_for<CONV>(p, offs, tg.z);
    uint row = tg.y * 16 + lid.y;
    uint col = tg.x * 16 + lid.x;
    ConvCol cc = {0, 0};
    if (CONV) cc = conv_col(p, col);
    float acc = 0.0f;
    for (uint k0 = 0; k0 < p.K; k0 += 16) {
        uint ka = k0 + lid.x;
        As[lid.y * 17 + lid.x] = (row < p.M && ka < p.K) ? A[bs.a + row * p.a_rs + ka * p.a_cs] : 0.0f;
        uint kb = k0 + lid.y;
        float bv = 0.0f;
        if (kb < p.K && col < p.N) {
            bv = CONV ? conv_load(B, p, bs.b, cc, kb) : B[bs.b + kb * p.b_rs + col * p.b_cs];
        }
        Bs[lid.y * 17 + lid.x] = bv;
        threadgroup_barrier(mem_flags::mem_threadgroup);
        for (uint kk = 0; kk < 16; kk++) acc = fma(As[lid.y * 17 + kk], Bs[kk * 17 + lid.x], acc);
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }
    if (row < p.M && col < p.N) C[bs.c + row * p.ldc + col] = epilogue(p, bias, bs.bias, acc, row, col);
}

kernel void gemm_tiled(device const float* A [[buffer(0)]], device const float* B [[buffer(1)]],
                       device float* C [[buffer(2)]], device const float* bias [[buffer(3)]],
                       device const uint2* offs [[buffer(4)]], constant GP& p [[buffer(5)]],
                       uint3 tg [[threadgroup_position_in_grid]], uint3 lid [[thread_position_in_threadgroup]]) {
    threadgroup float As[16 * 17];
    threadgroup float Bs[16 * 17];
    tiled_impl<false>(A, B, C, bias, offs, p, As, Bs, tg, lid);
}

kernel void conv_tiled(device const float* A [[buffer(0)]], device const float* B [[buffer(1)]],
                       device float* C [[buffer(2)]], device const float* bias [[buffer(3)]],
                       device const uint2* offs [[buffer(4)]], constant GP& p [[buffer(5)]],
                       uint3 tg [[threadgroup_position_in_grid]], uint3 lid [[thread_position_in_threadgroup]]) {
    threadgroup float As[16 * 17];
    threadgroup float Bs[16 * 17];
    tiled_impl<true>(A, B, C, bias, offs, p, As, Bs, tg, lid);
}

struct EP { uint n, op; float a0, a1; uint adiv, amod, bdiv, bmod; };

inline float erf_f(float x) {
    float z = fabs(x);
    if (z < 0.5f) {
        float x2 = x * x;
        float term = x;
        float sum = x;
        for (int n = 1; n < 12; n++) {
            term *= -x2 / float(n);
            sum += term / float(2 * n + 1);
        }
        return sum * 1.1283791670955126f;
    }
    float t = 1.0f / (1.0f + 0.5f * z);
    float ans = t * exp(-z * z - 1.26551223f + t * (1.00002368f + t * (0.37409196f + t * (0.09678418f +
                t * (-0.18628806f + t * (0.27886807f + t * (-1.13520398f + t * (1.48851587f +
                t * (-0.82215223f + t * 0.17087277f)))))))));
    return x >= 0.0f ? 1.0f - ans : ans - 1.0f;
}

inline float sigmoid_f(float x) {
    if (x >= 0.0f) return 1.0f / (1.0f + exp(-x));
    float e = exp(x);
    return e / (1.0f + e);
}

kernel void unary_k(device const float* x [[buffer(0)]], device float* y [[buffer(1)]],
                    constant EP& p [[buffer(2)]], uint i [[thread_position_in_grid]]) {
    if (i >= p.n) return;
    float v = x[i];
    float r;
    switch (p.op) {
        case 0: r = fmax(v, 0.0f); break;
        case 1: r = sigmoid_f(v); break;
        case 2: r = precise::tanh(v); break;
        case 3: r = 0.5f * v * (1.0f + erf_f(v * 0.70710678118654752f)); break;
        case 4: r = 0.5f * v * (1.0f + precise::tanh(0.7978845608028654f * (v + 0.044715f * v * v * v))); break;
        case 5: r = erf_f(v); break;
        case 6: r = v >= 0.0f ? v : p.a0 * v; break;
        case 7: r = v < p.a0 ? p.a0 : (v > p.a1 ? p.a1 : v); break;
        default: r = v; break;
    }
    y[i] = r;
}

kernel void binary_k(device const float* a [[buffer(0)]], device const float* b [[buffer(1)]],
                     device float* y [[buffer(2)]], constant EP& p [[buffer(3)]],
                     uint i [[thread_position_in_grid]]) {
    if (i >= p.n) return;
    float x = a[(i / p.adiv) % p.amod];
    float z = b[(i / p.bdiv) % p.bmod];
    float r;
    switch (p.op) {
        case 0: r = x + z; break;
        case 1: r = x - z; break;
        case 2: r = x * z; break;
        default: r = x / z; break;
    }
    y[i] = r;
}
"#;

const SIMD_SRC: &str = r#"
#include <metal_simdgroup_matrix>
#define BM 64
#define BN 64
#define BK 16
#define AS (BK + 4)
#define BS (BN + 4)
#define SH_FLOATS (BM * AS + BK * BS + 4 * 64)

// 128 threads = 4 simdgroups in a 2×2 arrangement, each owning a 32×32 block
// of the 64×64 output tile as 4×4 simdgroup_float8x8 accumulators. Tile rows
// are padded (AS / BS) against threadgroup-memory bank conflicts, and the next
// K-slab is fetched into registers while the current one is multiplied.

template <bool CONV>
inline void load_scalar(device const float* A, device const float* B, constant GP& p, Bases bs, ConvCol cc,
                        uint m0, uint n0, uint k0, uint tid, bool a_km, bool b_km,
                        thread float* ra, thread float* rb) {
    #pragma clang loop unroll(full)
    for (uint j = 0; j < 8; j++) {
        uint i = tid + 128 * j;
        uint r = a_km ? i % BM : i / BK;
        uint c = a_km ? i / BM : i % BK;
        uint gm = m0 + r;
        uint gk = k0 + c;
        ra[j] = (gm < p.M && gk < p.K) ? A[bs.a + gm * p.a_rs + gk * p.a_cs] : 0.0f;
        uint r2 = b_km ? i % BK : i / BN;
        uint c2 = b_km ? i / BK : i % BN;
        uint gk2 = k0 + r2;
        uint gn = n0 + c2;
        float v = 0.0f;
        if (gk2 < p.K && gn < p.N) {
            v = CONV ? conv_load(B, p, bs.b, cc, gk2) : B[bs.b + gk2 * p.b_rs + gn * p.b_cs];
        }
        rb[j] = v;
    }
}

inline void store_scalar(threadgroup float* As, threadgroup float* Bs, uint tid, bool a_km, bool b_km,
                         thread float* ra, thread float* rb) {
    #pragma clang loop unroll(full)
    for (uint j = 0; j < 8; j++) {
        uint i = tid + 128 * j;
        uint r = a_km ? i % BM : i / BK;
        uint c = a_km ? i / BM : i % BK;
        As[r * AS + c] = ra[j];
        uint r2 = b_km ? i % BK : i / BN;
        uint c2 = b_km ? i / BK : i % BN;
        Bs[r2 * BS + c2] = rb[j];
    }
}

inline void load_vec(device const float* A, device const float* B, constant GP& p, Bases bs,
                     uint m0, uint n0, uint k0, uint tid, thread float4* ra, thread float4* rb) {
    #pragma clang loop unroll(full)
    for (uint j = 0; j < 2; j++) {
        uint i = tid + 128 * j;
        uint r = i / (BK / 4);
        uint c = (i % (BK / 4)) * 4;
        uint gm = m0 + r;
        uint gk = k0 + c;
        ra[j] = (gm < p.M && gk < p.K) ? *((device const float4*)(A + bs.a + gm * p.a_rs + gk)) : float4(0.0f);
        uint r2 = i / (BN / 4);
        uint c2 = (i % (BN / 4)) * 4;
        uint gk2 = k0 + r2;
        uint gn = n0 + c2;
        rb[j] = (gk2 < p.K && gn < p.N) ? *((device const float4*)(B + bs.b + gk2 * p.b_rs + gn)) : float4(0.0f);
    }
}

inline void store_vec(threadgroup float* As, threadgroup float* Bs, uint tid, thread float4* ra, thread float4* rb) {
    #pragma clang loop unroll(full)
    for (uint j = 0; j < 2; j++) {
        uint i = tid + 128 * j;
        *((threadgroup float4*)(As + (i / (BK / 4)) * AS + (i % (BK / 4)) * 4)) = ra[j];
        *((threadgroup float4*)(Bs + (i / (BN / 4)) * BS + (i % (BN / 4)) * 4)) = rb[j];
    }
}

template <bool CONV, bool VEC>
inline void simd_impl(device const float* A, device const float* B, device float* C, device const float* bias,
                      device const uint2* offs, constant GP& p, threadgroup float* sh,
                      uint3 tg, uint tid, uint sg, uint lane) {
    Bases bs = bases_for<CONV>(p, offs, tg.z);
    uint m0 = tg.y * BM;
    uint n0 = tg.x * BN;
    threadgroup float* As = sh;
    threadgroup float* Bs = sh + BM * AS;
    threadgroup float* scratch = sh + BM * AS + BK * BS + sg * 64;
    uint sr = (sg >> 1) * 32;
    uint sc = (sg & 1) * 32;
    simdgroup_float8x8 acc[4][4];
    #pragma clang loop unroll(full)
    for (uint i = 0; i < 4; i++)
        #pragma clang loop unroll(full)
        for (uint j = 0; j < 4; j++) acc[i][j] = make_filled_simdgroup_matrix<float, 8, 8>(0.0f);
    bool a_km = !VEC && p.a_cs != 1;
    bool b_km = !VEC && !CONV && p.b_cs != 1;
    // For conv the B loader keeps a fixed output column per thread (tid % BN).
    ConvCol cc = {0, 0};
    if (CONV) cc = conv_col(p, n0 + (tid % BN));
    float ra[8];
    float rb[8];
    float4 ra4[2];
    float4 rb4[2];
    if (VEC) load_vec(A, B, p, bs, m0, n0, 0, tid, ra4, rb4);
    else load_scalar<CONV>(A, B, p, bs, cc, m0, n0, 0, tid, a_km, b_km, ra, rb);
    for (uint k0 = 0; k0 < p.K; k0 += BK) {
        if (VEC) store_vec(As, Bs, tid, ra4, rb4);
        else store_scalar(As, Bs, tid, a_km, b_km, ra, rb);
        threadgroup_barrier(mem_flags::mem_threadgroup);
        uint k1 = k0 + BK;
        if (k1 < p.K) {
            if (VEC) load_vec(A, B, p, bs, m0, n0, k1, tid, ra4, rb4);
            else load_scalar<CONV>(A, B, p, bs, cc, m0, n0, k1, tid, a_km, b_km, ra, rb);
        }
        #pragma clang loop unroll(full)
        for (uint kk = 0; kk < BK; kk += 8) {
            simdgroup_float8x8 a[4];
            simdgroup_float8x8 b[4];
            #pragma clang loop unroll(full)
            for (uint i = 0; i < 4; i++) simdgroup_load(a[i], As + (sr + i * 8) * AS + kk, AS);
            #pragma clang loop unroll(full)
            for (uint j = 0; j < 4; j++) simdgroup_load(b[j], Bs + kk * BS + sc + j * 8, BS);
            #pragma clang loop unroll(full)
            for (uint i = 0; i < 4; i++)
                #pragma clang loop unroll(full)
                for (uint j = 0; j < 4; j++) simdgroup_multiply_accumulate(acc[i][j], a[i], b[j], acc[i][j]);
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }
    #pragma clang loop unroll(full)
    for (uint i = 0; i < 4; i++) {
        #pragma clang loop unroll(full)
        for (uint j = 0; j < 4; j++) {
            simdgroup_store(acc[i][j], scratch, 8);
            simdgroup_barrier(mem_flags::mem_threadgroup);
            for (uint e = lane; e < 64; e += 32) {
                uint gm = m0 + sr + i * 8 + e / 8;
                uint gn = n0 + sc + j * 8 + e % 8;
                if (gm < p.M && gn < p.N) C[bs.c + gm * p.ldc + gn] = epilogue(p, bias, bs.bias, scratch[e], gm, gn);
            }
            simdgroup_barrier(mem_flags::mem_threadgroup);
        }
    }
}

#define SIMD_KERNEL(NAME, CONV, VEC) \
kernel void NAME(device const float* A [[buffer(0)]], device const float* B [[buffer(1)]], \
                 device float* C [[buffer(2)]], device const float* bias [[buffer(3)]], \
                 device const uint2* offs [[buffer(4)]], constant GP& p [[buffer(5)]], \
                 uint3 tg [[threadgroup_position_in_grid]], uint tid [[thread_index_in_threadgroup]], \
                 uint sg [[simdgroup_index_in_threadgroup]], uint lane [[thread_index_in_simdgroup]]) { \
    threadgroup float sh[SH_FLOATS]; \
    simd_impl<CONV, VEC>(A, B, C, bias, offs, p, sh, tg, tid, sg, lane); \
}

SIMD_KERNEL(gemm_simd, false, false)
SIMD_KERNEL(gemm_simd_vec, false, true)
SIMD_KERNEL(conv_simd, true, false)
"#;

/// Mirrors `struct GP` in the MSL source.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Gp {
    m: u32,
    n: u32,
    k: u32,
    a_rs: u32,
    a_cs: u32,
    b_rs: u32,
    b_cs: u32,
    ldc: u32,
    batch_c: u32,
    alpha: f32,
    beta: f32,
    bias_mode: u32,
    use_offs: u32,
    group: u32,
    ocg: u32,
    icg: u32,
    c_in: u32,
    c_out: u32,
    ih: u32,
    iw: u32,
    oh: u32,
    ow: u32,
    kh: u32,
    kw: u32,
    sh: u32,
    sw: u32,
    dh: u32,
    dw: u32,
    ph: u32,
    pw: u32,
}

/// Mirrors `struct EP` in the MSL source.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Ep {
    n: u32,
    op: u32,
    a0: f32,
    a1: f32,
    adiv: u32,
    amod: u32,
    bdiv: u32,
    bmod: u32,
}

// ------------------------------------------------------------------ device

struct Pipe {
    name: &'static str,
    obj: Obj,
    max_threads: usize,
}

struct Pipes {
    gemm_tiled: Pipe,
    conv_tiled: Pipe,
    unary: Pipe,
    binary: Pipe,
    gemm_simd: Option<Pipe>,
    gemm_simd_vec: Option<Pipe>,
    conv_simd: Option<Pipe>,
}

/// Command queue plus a free list of scratch buffers (guarded together).
struct State {
    queue: Obj,
    free: Vec<(usize, Obj)>,
    free_bytes: usize,
}

const POOL_LIMIT: usize = 256 << 20;
const NOCOPY_MIN: usize = 1 << 20;
const WEIGHT_CACHE_LIMIT: usize = 2 << 30;

/// The process-wide Metal device, compiled pipelines and command queue.
pub struct Metal {
    device: Obj,
    name: String,
    pipes: Pipes,
    dummy: Obj,
    page: usize,
    state: Mutex<State>,
}

/// A buffer argument of a dispatch.
enum In<'a> {
    /// Nothing bound by the caller (a tiny dummy buffer is bound instead).
    None,
    /// An existing GPU buffer (e.g. a cached weight).
    Buf(Obj),
    /// Host floats plus the capacity (in elements) of the allocation behind
    /// them, which decides whether they can be wrapped without a copy.
    Data(&'a [f32], usize),
    /// Host u32 words (batch offsets).
    Words(&'a [u32]),
}

struct Launch<'a> {
    pipe: &'a Pipe,
    inputs: Vec<(usize, In<'a>)>,
    out_index: usize,
    params: &'a [u8],
    params_index: usize,
    groups: MtlSize,
    threads: MtlSize,
    out_len: usize,
}

fn new_buffer(dev: &Obj, bytes: usize) -> Option<Obj> {
    Obj::owned(msg!(dev.id(), "newBufferWithLength:options:", bytes.max(16) => usize, 0usize => usize; Id))
}

fn contents(buf: &Obj) -> *mut u8 {
    msg!(buf.id(), "contents"; *mut c_void) as *mut u8
}

fn compile(dev: &Obj, src: &str) -> std::result::Result<Obj, String> {
    let opts = Obj::owned(msg!(msg!(class(c"MTLCompileOptions"), "alloc"; Id), "init"; Id));
    if let Some(o) = &opts
        && responds(o.id(), "setFastMathEnabled:\0")
    {
        // Precise math so results track the CPU kernels.
        msg!(o.id(), "setFastMathEnabled:", 0i8 => i8; ());
    }
    let src = ns_string(src);
    if src.is_null() {
        return Err("source is not a valid C string".into());
    }
    let mut err: Id = std::ptr::null_mut();
    let lib = msg!(dev.id(), "newLibraryWithSource:options:error:",
        src => Id, opts.as_ref().map_or(std::ptr::null_mut(), |o| o.id()) => Id, &mut err as *mut Id => *mut Id; Id);
    Obj::owned(lib).ok_or_else(|| error_text(err))
}

fn pipeline(dev: &Obj, lib: &Obj, name: &'static str) -> Option<Pipe> {
    let f = Obj::owned(msg!(lib.id(), "newFunctionWithName:", ns_string(name) => Id; Id))?;
    let mut err: Id = std::ptr::null_mut();
    let p = msg!(dev.id(), "newComputePipelineStateWithFunction:error:", f.id() => Id, &mut err as *mut Id => *mut Id; Id);
    let Some(obj) = Obj::owned(p) else {
        debug(|| format!("pipeline {name}: {}", error_text(err)));
        return None;
    };
    let max_threads = msg!(obj.id(), "maxTotalThreadsPerThreadgroup"; usize);
    Some(Pipe { name, obj, max_threads })
}

impl State {
    fn take(&mut self, dev: &Obj, bytes: usize) -> Option<(usize, Obj)> {
        let bytes = bytes.max(16);
        let best = self
            .free
            .iter()
            .enumerate()
            .filter(|(_, (c, _))| *c >= bytes && *c <= bytes.saturating_mul(2).max(1 << 16))
            .min_by_key(|(_, (c, _))| *c)
            .map(|(i, _)| i);
        if let Some(i) = best {
            let e = self.free.remove(i);
            self.free_bytes -= e.0;
            return Some(e);
        }
        let cap = round_up(bytes, 1 << 12);
        Some((cap, new_buffer(dev, cap)?))
    }

    fn give(&mut self, e: (usize, Obj)) {
        self.free_bytes += e.0;
        self.free.push(e);
        while self.free_bytes > POOL_LIMIT && !self.free.is_empty() {
            let (c, _) = self.free.remove(0);
            self.free_bytes -= c;
        }
    }
}

impl Metal {
    /// The shared Metal context, or `None` when no device exists or the
    /// kernels fail to compile.
    pub fn shared() -> Option<&'static Metal> {
        static SHARED: OnceLock<Option<Metal>> = OnceLock::new();
        SHARED.get_or_init(Metal::init).as_ref()
    }

    fn init() -> Option<Metal> {
        if std::env::var_os("STUDIO_NO_METAL").is_some() {
            return None;
        }
        let _pool = ArPool::new();
        let device = Obj::owned(unsafe { MTLCreateSystemDefaultDevice() })?;
        let name = rust_string(msg!(device.id(), "name"; Id));
        let queue = Obj::owned(msg!(device.id(), "newCommandQueue"; Id))?;
        let base = match compile(&device, &format!("{COMMON_SRC}{BASE_SRC}")) {
            Ok(l) => l,
            Err(e) => {
                debug(|| format!("base library: {e}"));
                return None;
            }
        };
        let gemm_tiled = pipeline(&device, &base, "gemm_tiled")?;
        let conv_tiled = pipeline(&device, &base, "conv_tiled")?;
        let unary = pipeline(&device, &base, "unary_k")?;
        let binary = pipeline(&device, &base, "binary_k")?;
        // simdgroup matrices need an Apple7+ GPU (M1 / A14 and later).
        let simd_ok = responds(device.id(), "supportsFamily:\0")
            && msg!(device.id(), "supportsFamily:", 1007isize => isize; i8) != 0
            && std::env::var_os("STUDIO_METAL_NO_SIMDGROUP").is_none();
        let (gemm_simd, gemm_simd_vec, conv_simd) = if simd_ok {
            match compile(&device, &format!("{COMMON_SRC}{SIMD_SRC}")) {
                Ok(lib) => (pipeline(&device, &lib, "gemm_simd"), pipeline(&device, &lib, "gemm_simd_vec"), pipeline(&device, &lib, "conv_simd")),
                Err(e) => {
                    debug(|| format!("simdgroup library: {e}"));
                    (None, None, None)
                }
            }
        } else {
            (None, None, None)
        };
        let dummy = new_buffer(&device, 256)?;
        let page = match unsafe { getpagesize() } {
            p if p > 0 => p as usize,
            _ => 16384,
        };
        debug(|| format!("device '{name}', simdgroup GEMM: {}", gemm_simd.is_some()));
        Some(Metal {
            device,
            name,
            pipes: Pipes { gemm_tiled, conv_tiled, unary, binary, gemm_simd, gemm_simd_vec, conv_simd },
            dummy,
            page,
            state: Mutex::new(State { queue, free: Vec::new(), free_bytes: 0 }),
        })
    }

    /// The GPU's name (e.g. "Apple M2 Max").
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// True when the simdgroup-matrix GEMM kernels are in use.
    pub fn has_simdgroup_gemm(&self) -> bool {
        self.pipes.gemm_simd.is_some()
    }

    /// Row-major `C[m×n] = A[m×k] · B[k×n]` on the GPU (includes the
    /// host↔GPU transfers). `None` on any failure or oversized problem.
    pub fn sgemm(&self, m: usize, n: usize, k: usize, a: &[f32], b: &[f32]) -> Option<Vec<f32>> {
        if a.len() < m * k || b.len() < n * k || m == 0 || n == 0 || k == 0 {
            return None;
        }
        u(a.len())?;
        u(b.len())?;
        u(m * n)?;
        let p = Gp { m: u(m)?, n: u(n)?, k: u(k)?, a_rs: k as u32, a_cs: 1, b_rs: n as u32, b_cs: 1, ldc: n as u32, batch_c: (m * n) as u32, alpha: 1.0, ..Gp::default() };
        self.run_gemm(In::Data(a, a.len()), In::Data(b, b.len()), In::None, In::None, p, 1, false, m * n)
    }

    /// A shared-storage buffer holding a copy of `v`.
    fn upload(&self, v: &[f32]) -> Option<Obj> {
        let b = new_buffer(&self.device, v.len() * 4)?;
        let dst = contents(&b);
        if dst.is_null() {
            return None;
        }
        // SAFETY: the buffer holds at least `v.len() * 4` bytes.
        unsafe { std::ptr::copy_nonoverlapping(v.as_ptr() as *const u8, dst, v.len() * 4) };
        Some(b)
    }

    /// Wraps host memory without copying when it is page aligned and the
    /// allocation covers the page-rounded length.
    fn wrap_nocopy(&self, ptr: *const u8, len_bytes: usize, cap_bytes: usize) -> Option<Obj> {
        if len_bytes < NOCOPY_MIN || (ptr as usize) % self.page != 0 {
            return None;
        }
        let l = round_up(len_bytes, self.page);
        if l > cap_bytes {
            return None;
        }
        Obj::owned(msg!(self.device.id(), "newBufferWithBytesNoCopy:length:options:deallocator:",
            ptr as *mut c_void => *mut c_void, l => usize, 0usize => usize, std::ptr::null_mut::<c_void>() => Id; Id))
    }

    #[allow(clippy::too_many_arguments)]
    fn run_gemm(&self, a: In, b: In, bias: In, offs: In, p: Gp, z: usize, conv: bool, out_len: usize) -> Option<Vec<f32>> {
        let (m, n) = (p.m as usize, p.n as usize);
        let vec_ok = !conv
            && p.a_cs == 1
            && p.b_cs == 1
            && p.k % 4 == 0
            && p.n % 4 == 0
            && p.a_rs % 4 == 0
            && p.b_rs % 4 == 0
            && match &offs {
                In::Words(w) => w.iter().all(|o| o % 4 == 0),
                _ => true,
            };
        let simd = match (m >= 32 && n >= 32, conv) {
            (false, _) => None,
            (true, true) => self.pipes.conv_simd.as_ref(),
            (true, false) if vec_ok && self.pipes.gemm_simd_vec.is_some() => self.pipes.gemm_simd_vec.as_ref(),
            (true, false) => self.pipes.gemm_simd.as_ref(),
        };
        let (pipe, groups, threads) = match simd.filter(|sp| sp.max_threads >= 128) {
            Some(sp) => (sp, mtl_size(n.div_ceil(64), m.div_ceil(64), z), mtl_size(128, 1, 1)),
            None => {
                let tp = if conv { &self.pipes.conv_tiled } else { &self.pipes.gemm_tiled };
                if tp.max_threads < 256 {
                    return None;
                }
                (tp, mtl_size(n.div_ceil(16), m.div_ceil(16), z), mtl_size(16, 16, 1))
            }
        };
        self.launch(Launch {
            pipe,
            inputs: vec![(0, a), (1, b), (3, bias), (4, offs)],
            out_index: 2,
            params: bytes_of(&p),
            params_index: 5,
            groups,
            threads,
            out_len,
        })
    }

    fn launch(&self, l: Launch) -> Option<Vec<f32>> {
        if l.out_len == 0 || l.groups.w == 0 || l.groups.h == 0 || l.groups.d == 0 {
            return None;
        }
        let mut st = self.state.lock().ok()?;
        let pool = ArPool::new();
        let r = self.launch_locked(&mut st, l);
        drop(pool);
        r
    }

    fn launch_locked(&self, st: &mut State, l: Launch) -> Option<Vec<f32>> {
        let mut pooled: Vec<(usize, Obj)> = Vec::new();
        let mut keep: Vec<Obj> = Vec::new();
        let mut binds: Vec<(usize, Id)> = Vec::new();
        for (idx, inp) in l.inputs {
            let id = match inp {
                In::None => self.dummy.id(),
                In::Buf(o) => {
                    let id = o.id();
                    keep.push(o);
                    id
                }
                In::Data(s, cap) => {
                    if let Some(o) = self.wrap_nocopy(s.as_ptr() as *const u8, s.len() * 4, cap * 4) {
                        let id = o.id();
                        keep.push(o);
                        id
                    } else {
                        let e = st.take(&self.device, s.len() * 4)?;
                        let dst = contents(&e.1);
                        if dst.is_null() {
                            return None;
                        }
                        // SAFETY: the pooled buffer holds at least s.len()*4 bytes.
                        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr() as *const u8, dst, s.len() * 4) };
                        let id = e.1.id();
                        pooled.push(e);
                        id
                    }
                }
                In::Words(w) => {
                    let e = st.take(&self.device, w.len() * 4)?;
                    let dst = contents(&e.1);
                    if dst.is_null() {
                        return None;
                    }
                    // SAFETY: as above.
                    unsafe { std::ptr::copy_nonoverlapping(w.as_ptr() as *const u8, dst, w.len() * 4) };
                    let id = e.1.id();
                    pooled.push(e);
                    id
                }
            };
            binds.push((idx, id));
        }

        // Output: written in place when the host allocation can be wrapped.
        let out_bytes = l.out_len * 4;
        let mut out: Vec<f32> = Vec::new();
        let mut out_nocopy = None;
        if out_bytes >= NOCOPY_MIN {
            let cap = round_up(out_bytes, self.page);
            out = vec![0.0f32; cap / 4];
            out.truncate(l.out_len);
            out_nocopy = self.wrap_nocopy(out.as_ptr() as *const u8, out_bytes, cap);
        }
        let mut out_pool = None;
        let out_id = match &out_nocopy {
            Some(o) => o.id(),
            None => {
                let e = st.take(&self.device, out_bytes)?;
                let id = e.1.id();
                out_pool = Some(e);
                id
            }
        };

        let cmd = msg!(st.queue.id(), "commandBuffer"; Id);
        if cmd.is_null() {
            return None;
        }
        let enc = msg!(cmd, "computeCommandEncoder"; Id);
        if enc.is_null() {
            return None;
        }
        msg!(enc, "setComputePipelineState:", l.pipe.obj.id() => Id; ());
        for (idx, id) in binds {
            msg!(enc, "setBuffer:offset:atIndex:", id => Id, 0usize => usize, idx => usize; ());
        }
        msg!(enc, "setBuffer:offset:atIndex:", out_id => Id, 0usize => usize, l.out_index => usize; ());
        msg!(enc, "setBytes:length:atIndex:", l.params.as_ptr() as *const c_void => *const c_void, l.params.len() => usize, l.params_index => usize; ());
        msg!(enc, "dispatchThreadgroups:threadsPerThreadgroup:", l.groups => MtlSize, l.threads => MtlSize; ());
        msg!(enc, "endEncoding"; ());
        msg!(cmd, "commit"; ());
        msg!(cmd, "waitUntilCompleted"; ());
        let status = msg!(cmd, "status"; usize);
        if debug_enabled() {
            let (t0, t1) = (msg!(cmd, "GPUStartTime"; f64), msg!(cmd, "GPUEndTime"; f64));
            debug(|| format!("{} grid {}x{}x{}: GPU {:.3} ms", l.pipe.name, l.groups.w, l.groups.h, l.groups.d, (t1 - t0) * 1e3));
        }
        const COMPLETED: usize = 4;
        if status != COMPLETED {
            debug(|| format!("command buffer failed (status {status}): {}", error_text(msg!(cmd, "error"; Id))));
            return None;
        }

        if let Some(e) = out_pool {
            let src = contents(&e.1);
            if src.is_null() {
                return None;
            }
            out = vec![0.0f32; l.out_len];
            // SAFETY: the buffer holds at least out_len*4 bytes written by the GPU.
            unsafe { std::ptr::copy_nonoverlapping(src, out.as_mut_ptr() as *mut u8, out_bytes) };
            st.give(e);
        }
        for e in pooled {
            st.give(e);
        }
        drop(out_nocopy);
        drop(keep);
        Some(out)
    }
}

// ------------------------------------------------------------------ backend

struct WeightCache {
    map: HashMap<(usize, usize, u64), Obj>,
    bytes: usize,
}

/// The Metal accelerator: claims GEMM / conv / large element-wise ops.
pub struct MetalBackend {
    metal: &'static Metal,
    min_flops: f64,
    weights: Mutex<WeightCache>,
}

fn raw_bytes(d: &Data) -> &[u8] {
    let (p, n) = match d {
        Data::F32(v) => (v.as_ptr() as *const u8, v.len() * 4),
        Data::F16(v) => (v.as_ptr() as *const u8, v.len() * 2),
        Data::I64(v) => (v.as_ptr() as *const u8, v.len() * 8),
        Data::I8(v) => (v.as_ptr() as *const u8, v.len()),
        Data::U8(v) => (v.as_ptr(), v.len()),
        Data::Bool(v) => (v.as_ptr() as *const u8, v.len()),
    };
    // SAFETY: the vector owns `n` initialised bytes at `p`.
    unsafe { std::slice::from_raw_parts(p, n) }
}

/// FNV-1a over the first/last 64 bytes and 64 evenly spaced 8-byte samples.
fn sample_hash(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |s: &[u8]| {
        for &x in s {
            h ^= x as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    let n = b.len();
    eat(&b[..n.min(64)]);
    eat(&b[n.saturating_sub(64)..]);
    if n > 8 {
        for i in 0..64 {
            let o = (n - 8) * i / 63;
            eat(&b[o..o + 8]);
        }
    }
    h
}

/// f32 view of a tensor plus the capacity of its allocation (in elements).
fn f32_src(t: &Tensor) -> (Cow<'_, [f32]>, usize) {
    match &t.data {
        Data::F32(v) => (Cow::Borrowed(v.as_slice()), v.capacity()),
        _ => {
            let v = t.to_f32_vec();
            let c = v.len();
            (Cow::Owned(v), c)
        }
    }
}

/// `(div, mod)` such that operand element = `src[(i / div) % mod]` for output
/// index `i`, when the operand's non-broadcast dims form one contiguous block.
fn bcast_index(sh: &[usize], out: &[usize]) -> Option<(u32, u32)> {
    if sh.len() > out.len() {
        return None;
    }
    let pad = out.len() - sh.len();
    let dim = |i: usize| if i < pad { 1 } else { sh[i - pad] };
    // state 0: before the block, 1: inside, 2: after
    let (mut state, mut first, mut last) = (0, usize::MAX, 0);
    for i in 0..out.len() {
        if out[i] == 1 {
            continue;
        }
        let full = dim(i) == out[i];
        if !full && dim(i) != 1 {
            return None;
        }
        match (state, full) {
            (0, true) => {
                state = 1;
                first = i;
                last = i;
            }
            (1, true) => last = i,
            (1, false) => state = 2,
            (2, true) => return None,
            _ => {}
        }
    }
    if first == usize::MAX {
        return Some((1, 1));
    }
    let md: usize = out[first..=last].iter().product();
    let dv: usize = out[last + 1..].iter().product();
    Some((u(dv)?, u(md)?))
}

impl MetalBackend {
    /// `None` when no Metal device is available.
    pub fn new(min_flops: f64) -> Option<MetalBackend> {
        let metal = Metal::shared()?;
        Some(MetalBackend { metal, min_flops: min_flops.max(0.0), weights: Mutex::new(WeightCache { map: HashMap::new(), bytes: 0 }) })
    }

    pub fn device_name(&self) -> &str {
        self.metal.device_name()
    }

    /// GPU buffer for a session-constant weight, uploaded on first use.
    fn weight(&self, t: &Tensor) -> Option<Obj> {
        let raw = raw_bytes(&t.data);
        let key = (raw.as_ptr() as usize, raw.len(), sample_hash(raw));
        if let Some(o) = self.weights.lock().ok()?.map.get(&key) {
            return Some(o.clone());
        }
        let v = t.f32s();
        let buf = self.metal.upload(&v)?;
        let mut c = self.weights.lock().ok()?;
        if c.bytes + v.len() * 4 > WEIGHT_CACHE_LIMIT {
            c.map.clear();
            c.bytes = 0;
        }
        c.bytes += v.len() * 4;
        c.map.insert(key, buf.clone());
        Some(buf)
    }

    fn big_enough(&self, flops: f64) -> bool {
        flops >= self.min_flops
    }

    fn matmul(&self, inputs: &[Option<&Tensor>], ctx: &OpCtx) -> Option<Vec<Tensor>> {
        let a = inputs.first().copied().flatten()?;
        let b = inputs.get(1).copied().flatten()?;
        if !a.dtype().is_float() || !b.dtype().is_float() || a.rank() == 0 || b.rank() == 0 {
            return None;
        }
        let mut ash = a.shape.clone();
        let mut bsh = b.shape.clone();
        let (a1, b1) = (ash.len() == 1, bsh.len() == 1);
        if a1 {
            ash.insert(0, 1);
        }
        if b1 {
            bsh.push(1);
        }
        let (m, k) = (ash[ash.len() - 2], ash[ash.len() - 1]);
        let (k2, n) = (bsh[bsh.len() - 2], bsh[bsh.len() - 1]);
        if k != k2 || m == 0 || n == 0 || k == 0 {
            return None;
        }
        let abatch = &ash[..ash.len() - 2];
        let bbatch = &bsh[..bsh.len() - 2];
        let batch = broadcast_shape(abatch, bbatch).ok()?;
        let nb = numel(&batch);
        if nb == 0 || !self.big_enough(2.0 * (nb * m) as f64 * (n * k) as f64) {
            return None;
        }
        u(a.len())?;
        u(b.len())?;
        u(nb * m * n)?;
        let mut oshape = batch.clone();
        if !a1 {
            oshape.push(m);
        }
        if !b1 {
            oshape.push(n);
        }
        let words: Vec<u32>;
        let (offs, z, mm) = if numel(bbatch) == 1 && numel(abatch) == nb {
            (In::None, 1, nb * m)
        } else {
            let ast = bstrides(abatch, &batch);
            let bst = bstrides(bbatch, &batch);
            let mut w = Vec::with_capacity(nb * 2);
            for i in 0..nb {
                let (mut ia, mut ib, mut rem) = (0, 0, i);
                for d in (0..batch.len()).rev() {
                    let c = rem % batch[d];
                    rem /= batch[d];
                    ia += c * ast[d];
                    ib += c * bst[d];
                }
                w.push(u(ia * m * k)?);
                w.push(u(ib * k * n)?);
            }
            words = w;
            (In::Words(&words), nb, m)
        };
        let p = Gp {
            m: u(mm)?,
            n: n as u32,
            k: k as u32,
            a_rs: k as u32,
            a_cs: 1,
            b_rs: n as u32,
            b_cs: 1,
            ldc: n as u32,
            batch_c: (m * n) as u32,
            alpha: 1.0,
            use_offs: matches!(offs, In::Words(_)) as u32,
            ..Gp::default()
        };
        let (av, acap) = f32_src(a);
        let bv;
        let b_in = if matches!(ctx.cache, Some(Prepacked::B(_))) {
            In::Buf(self.weight(b)?)
        } else {
            bv = f32_src(b);
            In::Data(&bv.0, bv.1)
        };
        let out = self.metal.run_gemm(In::Data(&av, acap), b_in, In::None, offs, p, z, false, nb * m * n)?;
        Some(vec![Tensor::f32(&oshape, out)])
    }

    fn gemm(&self, node: &Node, inputs: &[Option<&Tensor>], ctx: &OpCtx) -> Option<Vec<Tensor>> {
        let a = inputs.first().copied().flatten()?;
        let b = inputs.get(1).copied().flatten()?;
        let c = inputs.get(2).copied().flatten();
        if a.rank() != 2 || b.rank() != 2 || !a.dtype().is_float() || !b.dtype().is_float() {
            return None;
        }
        let ta = node.attr_i("transA", 0) != 0;
        let tb = node.attr_i("transB", 0) != 0;
        let alpha = node.attr_f("alpha", 1.0);
        let beta = node.attr_f("beta", 1.0);
        let (m, k) = if ta { (a.shape[1], a.shape[0]) } else { (a.shape[0], a.shape[1]) };
        let n = if tb { b.shape[0] } else { b.shape[1] };
        let kb = if tb { b.shape[1] } else { b.shape[0] };
        if k != kb || m == 0 || n == 0 || k == 0 || !self.big_enough(2.0 * m as f64 * (n * k) as f64) {
            return None;
        }
        u(a.len())?;
        u(b.len())?;
        u(m * n)?;
        let mut bias_mode = 0;
        let cv;
        let mut bias_in = In::None;
        if let Some(c) = c
            && beta != 0.0
        {
            if !c.dtype().is_float() || c.rank() > 2 {
                return None;
            }
            let cs = &c.shape;
            bias_mode = if c.len() == 1 {
                1
            } else if cs.last() == Some(&n) && c.len() == n {
                2
            } else if c.rank() == 2 && cs[0] == m && cs[1] == 1 {
                3
            } else if c.rank() == 2 && cs[0] == m && cs[1] == n {
                4
            } else {
                return None;
            };
            cv = f32_src(c);
            bias_in = In::Data(&cv.0, cv.1);
        }
        let (a_rs, a_cs) = if ta { (1, m) } else { (k, 1) };
        let (b_rs, b_cs) = if tb { (1, k) } else { (n, 1) };
        let p = Gp {
            m: m as u32,
            n: n as u32,
            k: k as u32,
            a_rs: a_rs as u32,
            a_cs: a_cs as u32,
            b_rs: b_rs as u32,
            b_cs: b_cs as u32,
            ldc: n as u32,
            batch_c: (m * n) as u32,
            alpha,
            beta,
            bias_mode,
            ..Gp::default()
        };
        let (av, acap) = f32_src(a);
        let bv;
        let b_in = if matches!(ctx.cache, Some(Prepacked::B(_))) {
            In::Buf(self.weight(b)?)
        } else {
            bv = f32_src(b);
            In::Data(&bv.0, bv.1)
        };
        let out = self.metal.run_gemm(In::Data(&av, acap), b_in, bias_in, In::None, p, 1, false, m * n)?;
        Some(vec![Tensor::f32(&[m, n], out)])
    }

    fn conv(&self, node: &Node, inputs: &[Option<&Tensor>], ctx: &OpCtx) -> Option<Vec<Tensor>> {
        let x = inputs.first().copied().flatten()?;
        let w = inputs.get(1).copied().flatten()?;
        let b = inputs.get(2).copied().flatten();
        if !x.dtype().is_float() || !w.dtype().is_float() || !(3..=4).contains(&x.rank()) || w.rank() != x.rank() {
            return None;
        }
        let (g, oshape) = crate::ops::nn::conv_geom(node, &x.shape, &w.shape).ok()?;
        if g.k[0] != 1 || g.inp[0] != 1 || g.out[0] != 1 || g.c_out % g.group != 0 || g.c_out == 0 {
            return None;
        }
        let (icg, ocg) = (g.icg(), g.ocg());
        if icg == 1 && ocg == 1 {
            return None; // depthwise: the CPU's direct kernel is better suited
        }
        let kk = icg * g.k[1] * g.k[2];
        let pp = g.out[1] * g.out[2];
        if kk == 0 || pp == 0 || g.n == 0 || !self.big_enough(2.0 * (g.n * g.c_out * pp) as f64 * kk as f64) {
            return None;
        }
        u(x.len())?;
        u(w.len())?;
        let out_len = g.n * g.c_out * pp;
        u(out_len)?;
        let bv;
        let bias_in = match b {
            Some(b) => {
                if b.len() != g.c_out || !b.dtype().is_float() {
                    return None;
                }
                bv = f32_src(b);
                In::Data(&bv.0, bv.1)
            }
            None => In::None,
        };
        let (xv, xcap) = f32_src(x);
        let wv;
        let w_in = if matches!(ctx.cache, Some(Prepacked::Conv { .. })) {
            In::Buf(self.weight(w)?)
        } else {
            wv = f32_src(w);
            In::Data(&wv.0, wv.1)
        };
        if g.group == 1 && g.k == [1, 1, 1] && g.stride == [1, 1, 1] && g.pad == [0, 0, 0] && g.inp == g.out {
            // pointwise conv is exactly a batched GEMM: W[c_out×c_in] · X[n][c_in×HW]
            let words: Vec<u32> = (0..g.n).flat_map(|i| [0, (i * g.c_in * pp) as u32]).collect();
            let p = Gp {
                m: g.c_out as u32,
                n: pp as u32,
                k: g.c_in as u32,
                a_rs: g.c_in as u32,
                a_cs: 1,
                b_rs: pp as u32,
                b_cs: 1,
                ldc: pp as u32,
                batch_c: (g.c_out * pp) as u32,
                alpha: 1.0,
                beta: 1.0,
                bias_mode: if b.is_some() { 3 } else { 0 },
                use_offs: 1,
                ..Gp::default()
            };
            let out = self.metal.run_gemm(w_in, In::Data(&xv, xcap), bias_in, In::Words(&words), p, g.n, false, out_len)?;
            return Some(vec![Tensor::f32(&oshape, out)]);
        }
        let p = Gp {
            m: ocg as u32,
            n: pp as u32,
            k: kk as u32,
            a_rs: kk as u32,
            a_cs: 1,
            ldc: pp as u32,
            alpha: 1.0,
            beta: 1.0,
            bias_mode: if b.is_some() { 3 } else { 0 },
            group: g.group as u32,
            ocg: ocg as u32,
            icg: icg as u32,
            c_in: g.c_in as u32,
            c_out: g.c_out as u32,
            ih: g.inp[1] as u32,
            iw: g.inp[2] as u32,
            oh: g.out[1] as u32,
            ow: g.out[2] as u32,
            kh: g.k[1] as u32,
            kw: g.k[2] as u32,
            sh: g.stride[1] as u32,
            sw: g.stride[2] as u32,
            dh: g.dil[1] as u32,
            dw: g.dil[2] as u32,
            ph: g.pad[1] as u32,
            pw: g.pad[2] as u32,
            ..Gp::default()
        };
        let out = self.metal.run_gemm(w_in, In::Data(&xv, xcap), bias_in, In::None, p, g.n * g.group, true, out_len)?;
        Some(vec![Tensor::f32(&oshape, out)])
    }

    fn elementwise_ok(&self, n: usize) -> bool {
        n > 0 && u(n).is_some() && self.big_enough(n as f64)
    }

    fn launch_ew(&self, pipe: &Pipe, inputs: Vec<(usize, In)>, p: Ep, out_index: usize, params_index: usize) -> Option<Vec<f32>> {
        if pipe.max_threads < 256 {
            return None;
        }
        let n = p.n as usize;
        self.metal.launch(Launch { pipe, inputs, out_index, params: bytes_of(&p), params_index, groups: mtl_size(n.div_ceil(256), 1, 1), threads: mtl_size(256, 1, 1), out_len: n })
    }

    fn unary(&self, node: &Node, inputs: &[Option<&Tensor>]) -> Option<Vec<Tensor>> {
        let x = inputs.first().copied().flatten()?;
        let xs = x.as_f32()?;
        if !self.elementwise_ok(xs.len()) {
            return None;
        }
        let mut p = Ep { n: xs.len() as u32, ..Ep::default() };
        p.op = match node.op_type.as_str() {
            "Relu" => 0,
            "Sigmoid" => 1,
            "Tanh" => 2,
            "Gelu" if node.attr_s_or("approximate", "none") == "tanh" => 4,
            "Gelu" => 3,
            "Erf" => 5,
            "LeakyRelu" => {
                p.a0 = node.attr_f("alpha", 0.01);
                6
            }
            "Clip" => {
                let bound = |i: usize, attr: &str, d: f32| -> Option<f32> {
                    match inputs.get(i).copied().flatten() {
                        Some(t) if t.len() == 1 && t.dtype().is_float() => Some(t.f32s()[0]),
                        Some(_) => None,
                        None => Some(node.attr_f(attr, d)),
                    }
                };
                p.a0 = bound(1, "min", f32::NEG_INFINITY)?;
                p.a1 = bound(2, "max", f32::INFINITY)?;
                7
            }
            _ => return None,
        };
        let cap = match &x.data {
            Data::F32(v) => v.capacity(),
            _ => xs.len(),
        };
        let out = self.launch_ew(&self.metal.pipes.unary, vec![(0, In::Data(xs, cap))], p, 1, 2)?;
        Some(vec![Tensor::f32(&x.shape, out)])
    }

    fn binary(&self, node: &Node, inputs: &[Option<&Tensor>]) -> Option<Vec<Tensor>> {
        let a = inputs.first().copied().flatten()?;
        let b = inputs.get(1).copied().flatten()?;
        let (Data::F32(av), Data::F32(bv)) = (&a.data, &b.data) else { return None };
        let out_shape = broadcast_shape(&a.shape, &b.shape).ok()?;
        let n = numel(&out_shape);
        if !self.elementwise_ok(n) || av.is_empty() || bv.is_empty() {
            return None;
        }
        let (adiv, amod) = bcast_index(&a.shape, &out_shape)?;
        let (bdiv, bmod) = bcast_index(&b.shape, &out_shape)?;
        let op = match node.op_type.as_str() {
            "Add" => 0,
            "Sub" => 1,
            "Mul" => 2,
            "Div" => 3,
            _ => return None,
        };
        let p = Ep { n: n as u32, op, a0: 0.0, a1: 0.0, adiv, amod, bdiv, bmod };
        let ins = vec![(0, In::Data(av, av.capacity())), (1, In::Data(bv, bv.capacity()))];
        let out = self.launch_ew(&self.metal.pipes.binary, ins, p, 2, 3)?;
        Some(vec![Tensor::f32(&out_shape, out)])
    }
}

impl Backend for MetalBackend {
    fn name(&self) -> &'static str {
        "metal"
    }

    fn try_run(&self, node: &Node, inputs: &[Option<&Tensor>], ctx: &OpCtx) -> Option<Result<Vec<Tensor>>> {
        if !crate::ops::normalize_domain(&node.domain).is_empty() {
            return None;
        }
        let r = match node.op_type.as_str() {
            "MatMul" => self.matmul(inputs, ctx),
            "Gemm" => self.gemm(node, inputs, ctx),
            "Conv" => self.conv(node, inputs, ctx),
            "Add" | "Sub" | "Mul" | "Div" => self.binary(node, inputs),
            "Relu" | "Sigmoid" | "Tanh" | "Gelu" | "Erf" | "LeakyRelu" | "Clip" => self.unary(node, inputs),
            _ => None,
        };
        r.map(Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_index_modes() {
        assert_eq!(bcast_index(&[2, 3, 4], &[2, 3, 4]), Some((1, 24)));
        assert_eq!(bcast_index(&[], &[2, 3, 4]), Some((1, 1)));
        assert_eq!(bcast_index(&[4], &[2, 3, 4]), Some((1, 4)));
        assert_eq!(bcast_index(&[1, 3, 1, 1], &[2, 3, 5, 5]), Some((25, 3)));
        assert_eq!(bcast_index(&[2, 1], &[2, 3]), Some((3, 2)));
        assert_eq!(bcast_index(&[2, 1, 4], &[2, 3, 4]), None);
    }

    #[test]
    fn sampled_hash_differs() {
        let a = vec![0u8; 4096];
        let mut b = a.clone();
        b[4095] = 1;
        assert_ne!(sample_hash(&a), sample_hash(&b));
        assert_eq!(sample_hash(&[]), sample_hash(&[]));
    }
}
