// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dense tensors: a shape plus typed storage.
//!
//! Storage types: f32, f16 (stored as raw `u16` bits, computed in f32), i64
//! (every wider/narrower integer type except 8-bit lands here: int16/32,
//! uint16/32/64), i8, u8 and bool. f64 and bf16 are converted to f32 on load.

use crate::error::Result;
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DType {
    F32,
    F16,
    I64,
    I8,
    U8,
    Bool,
}

impl DType {
    /// Maps an ONNX `TensorProto.DataType` to the storage type used here.
    pub fn from_onnx(t: i32) -> Option<DType> {
        Some(match t {
            1 | 11 | 16 => DType::F32, // float, double, bfloat16
            10 => DType::F16,
            2 => DType::U8,
            3 => DType::I8,
            4 | 5 | 6 | 7 | 12 | 13 => DType::I64,
            9 => DType::Bool,
            _ => return None,
        })
    }

    pub fn to_onnx(self) -> i32 {
        match self {
            DType::F32 => 1,
            DType::F16 => 10,
            DType::I64 => 7,
            DType::I8 => 3,
            DType::U8 => 2,
            DType::Bool => 9,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, DType::F32 | DType::F16)
    }

    pub fn size_bytes(self) -> usize {
        match self {
            DType::F32 => 4,
            DType::F16 => 2,
            DType::I64 => 8,
            DType::I8 | DType::U8 | DType::Bool => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    F32(Vec<f32>),
    /// IEEE half floats as raw bits.
    F16(Vec<u16>),
    I64(Vec<i64>),
    I8(Vec<i8>),
    U8(Vec<u8>),
    Bool(Vec<bool>),
}

impl Data {
    pub fn len(&self) -> usize {
        match self {
            Data::F32(v) => v.len(),
            Data::F16(v) => v.len(),
            Data::I64(v) => v.len(),
            Data::I8(v) => v.len(),
            Data::U8(v) => v.len(),
            Data::Bool(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn dtype(&self) -> DType {
        match self {
            Data::F32(_) => DType::F32,
            Data::F16(_) => DType::F16,
            Data::I64(_) => DType::I64,
            Data::I8(_) => DType::I8,
            Data::U8(_) => DType::U8,
            Data::Bool(_) => DType::Bool,
        }
    }
}

/// Element types usable in dtype-generic data-movement code.
pub trait Elem: Copy + Default + PartialEq + Send + Sync + 'static {
    const DTYPE: DType;
    fn slice(d: &Data) -> Option<&[Self]>;
    fn wrap(v: Vec<Self>) -> Data;
}

macro_rules! elem {
    ($t:ty, $var:ident) => {
        impl Elem for $t {
            const DTYPE: DType = DType::$var;
            fn slice(d: &Data) -> Option<&[Self]> {
                match d {
                    Data::$var(v) => Some(v),
                    _ => None,
                }
            }
            fn wrap(v: Vec<Self>) -> Data {
                Data::$var(v)
            }
        }
    };
}
elem!(f32, F32);
elem!(u16, F16);
elem!(i64, I64);
elem!(i8, I8);
elem!(u8, U8);
elem!(bool, Bool);

/// Calls a generic function `f::<T>(args..)` with `T` picked from a [`DType`].
#[macro_export]
macro_rules! dispatch_dtype {
    ($dt:expr, $f:ident ( $($a:expr),* )) => {
        match $dt {
            $crate::tensor::DType::F32 => $f::<f32>($($a),*),
            $crate::tensor::DType::F16 => $f::<u16>($($a),*),
            $crate::tensor::DType::I64 => $f::<i64>($($a),*),
            $crate::tensor::DType::I8 => $f::<i8>($($a),*),
            $crate::tensor::DType::U8 => $f::<u8>($($a),*),
            $crate::tensor::DType::Bool => $f::<bool>($($a),*),
        }
    };
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Data,
}

pub fn numel(shape: &[usize]) -> usize {
    shape.iter().product()
}

/// Row-major strides.
pub fn strides(shape: &[usize]) -> Vec<usize> {
    let mut s = vec![1; shape.len()];
    for i in (0..shape.len().saturating_sub(1)).rev() {
        s[i] = s[i + 1] * shape[i + 1];
    }
    s
}

impl Tensor {
    pub fn new(shape: Vec<usize>, data: Data) -> Result<Tensor> {
        crate::ensure!(numel(&shape) == data.len(), "tensor shape {:?} needs {} elements, got {}", shape, numel(&shape), data.len());
        Ok(Tensor { shape, data })
    }
    fn mk(shape: &[usize], data: Data) -> Tensor {
        assert_eq!(numel(shape), data.len(), "tensor shape {shape:?} does not match data length {}", data.len());
        Tensor { shape: shape.to_vec(), data }
    }
    pub fn f32(shape: &[usize], v: Vec<f32>) -> Tensor {
        Self::mk(shape, Data::F32(v))
    }
    pub fn f16_from_f32(shape: &[usize], v: &[f32]) -> Tensor {
        Self::mk(shape, Data::F16(v.iter().map(|&x| f32_to_f16(x)).collect()))
    }
    pub fn i64(shape: &[usize], v: Vec<i64>) -> Tensor {
        Self::mk(shape, Data::I64(v))
    }
    pub fn u8(shape: &[usize], v: Vec<u8>) -> Tensor {
        Self::mk(shape, Data::U8(v))
    }
    pub fn i8(shape: &[usize], v: Vec<i8>) -> Tensor {
        Self::mk(shape, Data::I8(v))
    }
    pub fn bool(shape: &[usize], v: Vec<bool>) -> Tensor {
        Self::mk(shape, Data::Bool(v))
    }
    pub fn scalar_f32(x: f32) -> Tensor {
        Self::mk(&[], Data::F32(vec![x]))
    }
    pub fn scalar_i64(x: i64) -> Tensor {
        Self::mk(&[], Data::I64(vec![x]))
    }
    pub fn zeros(dtype: DType, shape: &[usize]) -> Tensor {
        let n = numel(shape);
        let data = match dtype {
            DType::F32 => Data::F32(vec![0.0; n]),
            DType::F16 => Data::F16(vec![0; n]),
            DType::I64 => Data::I64(vec![0; n]),
            DType::I8 => Data::I8(vec![0; n]),
            DType::U8 => Data::U8(vec![0; n]),
            DType::Bool => Data::Bool(vec![false; n]),
        };
        Tensor { shape: shape.to_vec(), data }
    }
    pub fn from_elems<T: Elem>(shape: &[usize], v: Vec<T>) -> Tensor {
        Self::mk(shape, T::wrap(v))
    }

    pub fn dtype(&self) -> DType {
        self.data.dtype()
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn rank(&self) -> usize {
        self.shape.len()
    }
    pub fn elems<T: Elem>(&self) -> Option<&[T]> {
        T::slice(&self.data)
    }

    pub fn as_f32(&self) -> Option<&[f32]> {
        match &self.data {
            Data::F32(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_f32_mut(&mut self) -> Option<&mut [f32]> {
        match &mut self.data {
            Data::F32(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<&[i64]> {
        match &self.data {
            Data::I64(v) => Some(v),
            _ => None,
        }
    }

    /// The values as f32 (borrowed when already f32).
    pub fn f32s(&self) -> Cow<'_, [f32]> {
        match &self.data {
            Data::F32(v) => Cow::Borrowed(v),
            Data::F16(v) => Cow::Owned(v.iter().map(|&h| f16_to_f32(h)).collect()),
            Data::I64(v) => Cow::Owned(v.iter().map(|&x| x as f32).collect()),
            Data::I8(v) => Cow::Owned(v.iter().map(|&x| x as f32).collect()),
            Data::U8(v) => Cow::Owned(v.iter().map(|&x| x as f32).collect()),
            Data::Bool(v) => Cow::Owned(v.iter().map(|&x| x as u8 as f32).collect()),
        }
    }
    pub fn to_f32_vec(&self) -> Vec<f32> {
        self.f32s().into_owned()
    }
    pub fn into_f32_vec(self) -> Vec<f32> {
        match self.data {
            Data::F32(v) => v,
            _ => self.to_f32_vec(),
        }
    }
    /// The values as i64 (floats truncate toward zero).
    pub fn i64s(&self) -> Cow<'_, [i64]> {
        match &self.data {
            Data::I64(v) => Cow::Borrowed(v),
            Data::F32(v) => Cow::Owned(v.iter().map(|&x| x as i64).collect()),
            Data::F16(v) => Cow::Owned(v.iter().map(|&h| f16_to_f32(h) as i64).collect()),
            Data::I8(v) => Cow::Owned(v.iter().map(|&x| x as i64).collect()),
            Data::U8(v) => Cow::Owned(v.iter().map(|&x| x as i64).collect()),
            Data::Bool(v) => Cow::Owned(v.iter().map(|&x| x as i64).collect()),
        }
    }
    pub fn to_i64_vec(&self) -> Vec<i64> {
        self.i64s().into_owned()
    }
    pub fn to_bool_vec(&self) -> Vec<bool> {
        match &self.data {
            Data::Bool(v) => v.clone(),
            _ => self.f32s().iter().map(|&x| x != 0.0).collect(),
        }
    }

    /// Element-wise conversion (ONNX `Cast` semantics: float→int truncates,
    /// float→u8/i8 saturates, anything→bool is `!= 0`).
    pub fn cast(&self, to: DType) -> Tensor {
        if self.dtype() == to {
            return self.clone();
        }
        let data = match to {
            DType::F32 => Data::F32(self.to_f32_vec()),
            DType::F16 => Data::F16(self.f32s().iter().map(|&x| f32_to_f16(x)).collect()),
            DType::I64 => Data::I64(self.to_i64_vec()),
            DType::I8 => Data::I8(match &self.data {
                Data::F32(_) | Data::F16(_) => self.f32s().iter().map(|&x| x.clamp(-128.0, 127.0) as i8).collect(),
                _ => self.i64s().iter().map(|&x| x as i8).collect(),
            }),
            DType::U8 => Data::U8(match &self.data {
                Data::F32(_) | Data::F16(_) => self.f32s().iter().map(|&x| x.clamp(0.0, 255.0) as u8).collect(),
                _ => self.i64s().iter().map(|&x| x as u8).collect(),
            }),
            DType::Bool => Data::Bool(self.to_bool_vec()),
        };
        Tensor { shape: self.shape.clone(), data }
    }

    pub fn reshape(mut self, shape: &[usize]) -> Result<Tensor> {
        crate::ensure!(numel(shape) == self.len(), "cannot reshape {:?} to {:?}", self.shape, shape);
        self.shape = shape.to_vec();
        Ok(self)
    }

    /// Max absolute difference to another tensor (as f32), for tests.
    pub fn max_abs_diff(&self, other: &Tensor) -> f32 {
        let a = self.f32s();
        let b = other.f32s();
        if a.len() != b.len() {
            return f32::INFINITY;
        }
        a.iter().zip(b.iter()).map(|(x, y)| if x.is_nan() && y.is_nan() { 0.0 } else { (x - y).abs() }).fold(0.0, f32::max)
    }
}

// ---------------------------------------------------------------- half floats

/// IEEE 754 binary16 → binary32.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if man == 0 {
            sign
        } else {
            // subnormal: normalise
            let mut e = 127 - 15 + 1;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 31 {
        sign | 0x7f80_0000 | (man << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

/// IEEE 754 binary32 → binary16, round to nearest even.
pub fn f32_to_f16(f: f32) -> u16 {
    let x = f.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let man = x & 0x7f_ffff;
    if exp == 255 {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rem = m & ((1 << shift) - 1);
        let mut r = m >> shift;
        if rem > half || (rem == half && (r & 1) == 1) {
            r += 1;
        }
        return sign | r as u16;
    }
    let mut r = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && (r & 1) == 1) {
        r += 1; // may carry into the exponent, which is the correct result
    }
    sign | r as u16
}

/// bfloat16 bits → f32.
pub fn bf16_to_f32(h: u16) -> f32 {
    f32::from_bits((h as u32) << 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn half_roundtrip() {
        for &v in &[0.0f32, 1.0, -2.5, 65504.0, 6.1035156e-5, 5.9604645e-8, 0.33325195] {
            assert_eq!(f16_to_f32(f32_to_f16(v)), v, "{v}");
        }
        assert_eq!(f32_to_f16(1e6), 0x7c00);
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
        // every half value round trips
        for h in 0..=0xffffu16 {
            let f = f16_to_f32(h);
            if !f.is_nan() {
                assert_eq!(f32_to_f16(f), h, "{h:#x}");
            }
        }
    }
    #[test]
    fn strides_row_major() {
        assert_eq!(strides(&[2, 3, 4]), vec![12, 4, 1]);
        assert_eq!(strides(&[]), Vec::<usize>::new());
    }
}
