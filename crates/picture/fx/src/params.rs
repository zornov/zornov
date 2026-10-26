// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Declarative parameter lists. Every effect, transition and motion preset
//! publishes a `&'static [ParamDef]`; the UI builds its controls from it and
//! the implementation reads values back from a [`ParamSet`].
//!
//! Sizes are given in **percent of the frame height** wherever a length is
//! needed, so a look chosen on a small preview matches the 4K export.

use std::fmt;

/// The kind of control a parameter needs, with its legal range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// A slider. `min..=max` is the legal range.
    Float { min: f32, max: f32 },
    /// An integer slider or stepper.
    Int { min: i32, max: i32 },
    /// A checkbox.
    Bool,
    /// A drop-down; the value is an index into the list of labels.
    Choice(&'static [&'static str]),
    /// A colour picker. Linear RGB plus straight alpha.
    Colour,
    /// A position on the frame, normalised so (0, 0) is the top-left
    /// corner and (1, 1) the bottom-right one. Values outside are allowed
    /// within -1..2 so things can sit off-screen.
    Point,
}

/// A parameter value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamValue {
    Float(f32),
    Int(i32),
    Bool(bool),
    Choice(u32),
    Colour([f32; 4]),
    Point([f32; 2]),
}

/// Unit shown next to a value; purely informative for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    None,
    /// Percent of the frame height.
    PercentHeight,
    /// A 0..1 amount shown as 0..100 %.
    Fraction,
    Degrees,
    Seconds,
    Hertz,
    /// Plain multiplier such as a zoom factor.
    Times,
}

/// One entry of a parameter list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamDef {
    /// Stable identifier, used in project files. Never rename one.
    pub id: &'static str,
    /// Label shown in the inspector.
    pub label: &'static str,
    pub kind: ParamKind,
    pub default: ParamValue,
    /// Whether the value may change over time with keyframes.
    pub animatable: bool,
    pub unit: Unit,
}

impl ParamDef {
    pub const fn float(id: &'static str, label: &'static str, min: f32, max: f32, default: f32) -> Self {
        Self {
            id,
            label,
            kind: ParamKind::Float { min, max },
            default: ParamValue::Float(default),
            animatable: true,
            unit: Unit::None,
        }
    }

    pub const fn int(id: &'static str, label: &'static str, min: i32, max: i32, default: i32) -> Self {
        Self {
            id,
            label,
            kind: ParamKind::Int { min, max },
            default: ParamValue::Int(default),
            animatable: false,
            unit: Unit::None,
        }
    }

    pub const fn bool(id: &'static str, label: &'static str, default: bool) -> Self {
        Self { id, label, kind: ParamKind::Bool, default: ParamValue::Bool(default), animatable: false, unit: Unit::None }
    }

    pub const fn choice(id: &'static str, label: &'static str, options: &'static [&'static str], default: u32) -> Self {
        Self {
            id,
            label,
            kind: ParamKind::Choice(options),
            default: ParamValue::Choice(default),
            animatable: false,
            unit: Unit::None,
        }
    }

    pub const fn colour(id: &'static str, label: &'static str, default: [f32; 4]) -> Self {
        Self { id, label, kind: ParamKind::Colour, default: ParamValue::Colour(default), animatable: true, unit: Unit::None }
    }

    pub const fn point(id: &'static str, label: &'static str, default: [f32; 2]) -> Self {
        Self { id, label, kind: ParamKind::Point, default: ParamValue::Point(default), animatable: true, unit: Unit::None }
    }

    /// Marks the parameter as not animatable.
    pub const fn fixed(mut self) -> Self {
        self.animatable = false;
        self
    }

    /// Marks the parameter as animatable (integers default to fixed).
    pub const fn animated(mut self) -> Self {
        self.animatable = true;
        self
    }

    pub const fn unit(mut self, unit: Unit) -> Self {
        self.unit = unit;
        self
    }

    /// Numeric range for sliders, if the parameter has one.
    pub fn range(&self) -> Option<(f32, f32)> {
        match self.kind {
            ParamKind::Float { min, max } => Some((min, max)),
            ParamKind::Int { min, max } => Some((min as f32, max as f32)),
            ParamKind::Point => Some((-1.0, 2.0)),
            ParamKind::Colour => Some((0.0, 1.0)),
            _ => None,
        }
    }

    /// Brings a value into this parameter's kind and range. Numbers are
    /// converted between float and int, out-of-range values are clamped,
    /// and anything that does not fit at all gives an error.
    pub fn coerce(&self, v: ParamValue) -> Result<ParamValue, ParamError> {
        let bad = || ParamError::WrongKind { id: self.id.to_string() };
        let finite = |f: f32| if f.is_finite() { Ok(f) } else { Err(ParamError::NotFinite { id: self.id.to_string() }) };
        Ok(match (self.kind, v) {
            (ParamKind::Float { min, max }, ParamValue::Float(f)) => ParamValue::Float(finite(f)?.clamp(min, max)),
            (ParamKind::Float { min, max }, ParamValue::Int(i)) => ParamValue::Float((i as f32).clamp(min, max)),
            (ParamKind::Int { min, max }, ParamValue::Int(i)) => ParamValue::Int(i.clamp(min, max)),
            (ParamKind::Int { min, max }, ParamValue::Float(f)) => {
                ParamValue::Int((finite(f)?.round() as i32).clamp(min, max))
            }
            (ParamKind::Bool, ParamValue::Bool(b)) => ParamValue::Bool(b),
            (ParamKind::Choice(opts), ParamValue::Choice(c)) => {
                if (c as usize) < opts.len() {
                    ParamValue::Choice(c)
                } else {
                    return Err(ParamError::NoSuchOption { id: self.id.to_string(), index: c });
                }
            }
            (ParamKind::Colour, ParamValue::Colour(c)) => {
                for &x in &c {
                    finite(x)?;
                }
                // Colours may be brighter than 1 (emissive) but not negative;
                // alpha is a coverage and stays in 0..1.
                ParamValue::Colour([c[0].clamp(0.0, 64.0), c[1].clamp(0.0, 64.0), c[2].clamp(0.0, 64.0), c[3].clamp(0.0, 1.0)])
            }
            (ParamKind::Point, ParamValue::Point(p)) => {
                ParamValue::Point([finite(p[0])?.clamp(-1.0, 2.0), finite(p[1])?.clamp(-1.0, 2.0)])
            }
            _ => return Err(bad()),
        })
    }
}

/// Why a value was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamError {
    UnknownId(String),
    WrongKind { id: String },
    NotFinite { id: String },
    NoSuchOption { id: String, index: u32 },
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamError::UnknownId(id) => write!(f, "no parameter called `{id}`"),
            ParamError::WrongKind { id } => write!(f, "wrong kind of value for `{id}`"),
            ParamError::NotFinite { id } => write!(f, "value for `{id}` is not a finite number"),
            ParamError::NoSuchOption { id, index } => write!(f, "`{id}` has no option {index}"),
        }
    }
}

impl std::error::Error for ParamError {}

/// A complete set of values for one parameter list. It always holds a
/// valid value for every parameter, so implementations never deal with
/// missing or out-of-range input.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamSet {
    defs: &'static [ParamDef],
    values: Vec<ParamValue>,
}

impl ParamSet {
    /// All parameters at their defaults.
    pub fn defaults(defs: &'static [ParamDef]) -> Self {
        Self { defs, values: defs.iter().map(|d| d.default).collect() }
    }

    pub fn defs(&self) -> &'static [ParamDef] {
        self.defs
    }

    /// Values in declaration order.
    pub fn values(&self) -> &[ParamValue] {
        &self.values
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.defs.iter().position(|d| d.id == id)
    }

    /// Sets a value, clamping it into range. See [`ParamDef::coerce`].
    pub fn set(&mut self, id: &str, v: ParamValue) -> Result<(), ParamError> {
        let i = self.index(id).ok_or_else(|| ParamError::UnknownId(id.to_string()))?;
        self.values[i] = self.defs[i].coerce(v)?;
        Ok(())
    }

    /// Builder form of [`ParamSet::set`] for code and tests; panics on a
    /// bad id or kind because that is a programming error there.
    pub fn with(mut self, id: &str, v: ParamValue) -> Self {
        if let Err(e) = self.set(id, v) {
            panic!("{e}");
        }
        self
    }

    pub fn get(&self, id: &str) -> Option<ParamValue> {
        self.index(id).map(|i| self.values[i])
    }

    fn expect(&self, id: &str) -> ParamValue {
        match self.get(id) {
            Some(v) => v,
            None => panic!("parameter `{id}` is not declared"),
        }
    }

    /// Float value (ints are converted).
    pub fn f(&self, id: &str) -> f32 {
        match self.expect(id) {
            ParamValue::Float(f) => f,
            ParamValue::Int(i) => i as f32,
            ParamValue::Bool(b) => b as u8 as f32,
            ParamValue::Choice(c) => c as f32,
            _ => panic!("parameter `{id}` is not a number"),
        }
    }

    pub fn i(&self, id: &str) -> i32 {
        match self.expect(id) {
            ParamValue::Int(i) => i,
            ParamValue::Float(f) => f.round() as i32,
            _ => panic!("parameter `{id}` is not an integer"),
        }
    }

    pub fn b(&self, id: &str) -> bool {
        match self.expect(id) {
            ParamValue::Bool(b) => b,
            _ => panic!("parameter `{id}` is not a switch"),
        }
    }

    /// Index of the selected option.
    pub fn choice(&self, id: &str) -> usize {
        match self.expect(id) {
            ParamValue::Choice(c) => c as usize,
            _ => panic!("parameter `{id}` is not a choice"),
        }
    }

    /// Straight-alpha linear colour.
    pub fn colour(&self, id: &str) -> [f32; 4] {
        match self.expect(id) {
            ParamValue::Colour(c) => c,
            _ => panic!("parameter `{id}` is not a colour"),
        }
    }

    pub fn point(&self, id: &str) -> [f32; 2] {
        match self.expect(id) {
            ParamValue::Point(p) => p,
            _ => panic!("parameter `{id}` is not a point"),
        }
    }

    /// Packs the values for a GPU uniform buffer: one `vec4<f32>` per
    /// parameter in declaration order. Numbers, switches and choice indices
    /// go in `.x`; points in `.xy`; colours fill all four lanes with the
    /// colour premultiplied by its alpha.
    pub fn pack_vec4(&self) -> Vec<[f32; 4]> {
        self.values
            .iter()
            .map(|v| match *v {
                ParamValue::Float(f) => [f, 0.0, 0.0, 0.0],
                ParamValue::Int(i) => [i as f32, 0.0, 0.0, 0.0],
                ParamValue::Bool(b) => [b as u8 as f32, 0.0, 0.0, 0.0],
                ParamValue::Choice(c) => [c as f32, 0.0, 0.0, 0.0],
                ParamValue::Point(p) => [p[0], p[1], 0.0, 0.0],
                ParamValue::Colour(c) => [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]],
            })
            .collect()
    }
}

/// Checks a parameter list for mistakes: duplicate ids, defaults outside
/// their range or of the wrong kind, empty choices. Used by the tests over
/// every catalogue entry.
pub fn validate_defs(defs: &[ParamDef]) -> Result<(), String> {
    for (i, d) in defs.iter().enumerate() {
        if defs[..i].iter().any(|o| o.id == d.id) {
            return Err(format!("duplicate id `{}`", d.id));
        }
        if d.id.is_empty() || !d.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(format!("id `{}` must be lower_snake_case", d.id));
        }
        if let ParamKind::Choice(opts) = d.kind {
            if opts.is_empty() {
                return Err(format!("`{}` has no options", d.id));
            }
        }
        if let Some((lo, hi)) = d.range() {
            if lo > hi {
                return Err(format!("`{}` has an empty range", d.id));
            }
        }
        match d.coerce(d.default) {
            Ok(v) if v == d.default => {}
            Ok(_) => return Err(format!("default of `{}` is out of range", d.id)),
            Err(e) => return Err(format!("default of `{}`: {e}", d.id)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static DEFS: &[ParamDef] = &[
        ParamDef::float("radius", "Radius", 0.0, 10.0, 2.0),
        ParamDef::int("count", "Count", 1, 8, 3),
        ParamDef::choice("mode", "Mode", &["a", "b"], 1),
        ParamDef::colour("tint", "Tint", [1.0, 0.5, 0.0, 0.5]),
    ];

    #[test]
    fn defaults_and_clamping() {
        validate_defs(DEFS).unwrap();
        let mut p = ParamSet::defaults(DEFS);
        assert_eq!(p.f("radius"), 2.0);
        p.set("radius", ParamValue::Float(99.0)).unwrap();
        assert_eq!(p.f("radius"), 10.0);
        p.set("count", ParamValue::Float(4.6)).unwrap();
        assert_eq!(p.i("count"), 5);
        assert!(p.set("mode", ParamValue::Choice(2)).is_err());
        assert!(p.set("radius", ParamValue::Float(f32::NAN)).is_err());
        assert!(p.set("nope", ParamValue::Bool(true)).is_err());
        assert_eq!(p.pack_vec4()[3], [0.5, 0.25, 0.0, 0.5]);
    }
}
