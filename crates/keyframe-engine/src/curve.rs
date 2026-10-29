// SPDX-License-Identifier: GPL-3.0-or-later
//! The curve itself.

/// How to travel from a key to the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Interp {
    /// Keep this key's value until the next key, then jump to the next value.
    Hold,
    /// A straight line to the next key.
    #[default]
    Linear,
    /// A smooth cubic curve, shaped by the tangents of this key and the next.
    Smooth,
}

impl Interp {
    pub fn as_str(self) -> &'static str {
        match self {
            Interp::Hold => "hold",
            Interp::Linear => "linear",
            Interp::Smooth => "smooth",
        }
    }

    pub fn parse(name: &str) -> Option<Interp> {
        Some(match name {
            "hold" | "step" => Interp::Hold,
            "linear" => Interp::Linear,
            "smooth" => Interp::Smooth,
            _ => return None,
        })
    }
}

/// One keyframe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// Time in ticks.
    pub t: i64,
    pub value: f64,
    /// How to travel to the next key.
    pub interp: Interp,
    /// The curve's slope at this key, in value per tick, used by smooth segments on either
    /// side. `None` is automatic.
    pub tangent: Option<f64>,
}

impl Key {
    pub fn new(t: i64, value: f64, interp: Interp) -> Key {
        Key {
            t,
            value,
            interp,
            tangent: None,
        }
    }
}

// Keys never hold NaN (curves refuse them), so equality is a true equivalence.
impl Eq for Key {}

/// Why a list of keys is not a curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveError {
    /// Keys must be in strictly increasing time order.
    NotSorted,
    /// A value or tangent is NaN or infinite.
    NotFinite,
}

impl std::fmt::Display for CurveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CurveError::NotSorted => write!(f, "keys must be in strictly increasing time order"),
            CurveError::NotFinite => write!(f, "keyframe values must be finite numbers"),
        }
    }
}

impl std::error::Error for CurveError {}

/// A list of keys, in time order, with at most one key at any time.
#[derive(Debug, Clone, Default)]
pub struct Curve {
    keys: Vec<Key>,
    /// `cum[i]` is the area under the curve from the first key to key `i`.
    cum: Vec<f64>,
}

impl PartialEq for Curve {
    fn eq(&self, other: &Curve) -> bool {
        self.keys == other.keys
    }
}

impl Eq for Curve {}

fn finite(k: &Key) -> bool {
    k.value.is_finite() && k.tangent.is_none_or(f64::is_finite)
}

impl Curve {
    /// An empty curve (no keys; callers fall back to a fixed value).
    pub fn new() -> Curve {
        Curve::default()
    }

    /// A curve from keys that are already in strictly increasing time order.
    pub fn from_keys(keys: Vec<Key>) -> Result<Curve, CurveError> {
        if keys.iter().any(|k| !finite(k)) {
            return Err(CurveError::NotFinite);
        }
        if keys.windows(2).any(|w| w[0].t >= w[1].t) {
            return Err(CurveError::NotSorted);
        }
        let mut c = Curve {
            keys,
            cum: Vec::new(),
        };
        c.refresh();
        Ok(c)
    }

    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Put a key at time `t`, replacing any key already there (an automatic tangent is restored).
    /// Returns an error if the value is not finite.
    pub fn set(&mut self, t: i64, value: f64, interp: Interp) -> Result<(), CurveError> {
        let key = Key::new(t, value, interp);
        if !finite(&key) {
            return Err(CurveError::NotFinite);
        }
        match self.keys.binary_search_by_key(&t, |k| k.t) {
            Ok(i) => self.keys[i] = key,
            Err(i) => self.keys.insert(i, key),
        }
        self.refresh();
        Ok(())
    }

    /// Remove the key at time `t`. Returns whether there was one.
    pub fn remove(&mut self, t: i64) -> bool {
        match self.keys.binary_search_by_key(&t, |k| k.t) {
            Ok(i) => {
                self.keys.remove(i);
                self.refresh();
                true
            }
            Err(_) => false,
        }
    }

    /// Move every key by `dt` ticks.
    pub fn shift(&mut self, dt: i64) {
        for k in &mut self.keys {
            k.t += dt;
        }
        self.refresh();
    }

    /// The slope of key `i` in value per tick: its own tangent, or the automatic one.
    fn tangent_at(&self, i: usize) -> f64 {
        let k = &self.keys[i];
        if let Some(t) = k.tangent {
            return t;
        }
        if i == 0 || i + 1 == self.keys.len() {
            return 0.0;
        }
        let (p, n) = (&self.keys[i - 1], &self.keys[i + 1]);
        let d0 = (k.value - p.value) / (k.t - p.t) as f64;
        let d1 = (n.value - k.value) / (n.t - k.t) as f64;
        if d0 * d1 <= 0.0 {
            0.0
        } else {
            // Harmonic mean: monotone, so a smooth curve never overshoots its keys.
            2.0 * d0 * d1 / (d0 + d1)
        }
    }

    /// Index of the segment containing `t`: the last key at or before it.
    fn segment(&self, t: i64) -> Option<usize> {
        match self.keys.binary_search_by_key(&t, |k| k.t) {
            Ok(i) => Some(i),
            Err(0) => None,
            Err(i) => Some(i - 1),
        }
    }

    /// The value at time `t`, or `None` for a curve with no keys.
    pub fn eval(&self, t: i64) -> Option<f64> {
        let first = self.keys.first()?;
        let last = self.keys.last()?;
        if t <= first.t {
            return Some(first.value);
        }
        if t >= last.t {
            return Some(last.value);
        }
        let i = self.segment(t)?;
        Some(self.segment_value(i, t))
    }

    fn segment_value(&self, i: usize, t: i64) -> f64 {
        let (a, b) = (&self.keys[i], &self.keys[i + 1]);
        if t == a.t {
            return a.value;
        }
        let span = (b.t - a.t) as f64;
        let u = (t - a.t) as f64 / span;
        match a.interp {
            Interp::Hold => a.value,
            Interp::Linear => a.value + (b.value - a.value) * u,
            Interp::Smooth => {
                let (m0, m1) = (self.tangent_at(i), self.tangent_at(i + 1));
                let (u2, u3) = (u * u, u * u * u);
                (2.0 * u3 - 3.0 * u2 + 1.0) * a.value
                    + (u3 - 2.0 * u2 + u) * span * m0
                    + (-2.0 * u3 + 3.0 * u2) * b.value
                    + (u3 - u2) * span * m1
            }
        }
    }

    /// The slope of the curve at `t` in value per tick (for smooth segments; linear segments
    /// give their constant slope; hold gives zero).
    fn segment_slope(&self, i: usize, t: i64) -> f64 {
        let (a, b) = (&self.keys[i], &self.keys[i + 1]);
        let span = (b.t - a.t) as f64;
        let u = (t - a.t) as f64 / span;
        match a.interp {
            Interp::Hold => 0.0,
            Interp::Linear => (b.value - a.value) / span,
            Interp::Smooth => {
                let (m0, m1) = (self.tangent_at(i), self.tangent_at(i + 1));
                let u2 = u * u;
                ((6.0 * u2 - 6.0 * u) * a.value
                    + (3.0 * u2 - 4.0 * u + 1.0) * span * m0
                    + (-6.0 * u2 + 6.0 * u) * b.value
                    + (3.0 * u2 - 2.0 * u) * span * m1)
                    / span
            }
        }
    }

    /// Area under the segment `i` from its start to time `t` (inside the segment).
    fn segment_area(&self, i: usize, t: i64) -> f64 {
        let (a, b) = (&self.keys[i], &self.keys[i + 1]);
        let span = (b.t - a.t) as f64;
        let ut = (t - a.t) as f64;
        let u = ut / span;
        match a.interp {
            Interp::Hold => a.value * ut,
            Interp::Linear => a.value * ut + (b.value - a.value) * ut * u / 2.0,
            Interp::Smooth => {
                let (m0, m1) = (self.tangent_at(i), self.tangent_at(i + 1));
                let (u2, u3, u4) = (u * u, u * u * u, u * u * u * u);
                span * (a.value * (u4 / 2.0 - u3 + u)
                    + span * m0 * (u4 / 4.0 - 2.0 * u3 / 3.0 + u2 / 2.0)
                    + b.value * (-u4 / 2.0 + u3)
                    + span * m1 * (u4 / 4.0 - u3 / 3.0))
            }
        }
    }

    fn refresh(&mut self) {
        self.cum.clear();
        let mut acc = 0.0;
        for i in 0..self.keys.len() {
            self.cum.push(acc);
            if i + 1 < self.keys.len() {
                acc += self.segment_area(i, self.keys[i + 1].t);
            }
        }
    }

    /// The exact area under the curve from time 0 to `t` (negative `t` gives a negative area),
    /// in value times ticks. A curve with no keys has area 0.
    pub fn integral(&self, t: i64) -> f64 {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return 0.0;
        };
        // Area from 0 up to the first key, holding the first value.
        let to_first = first.value * first.t as f64;
        if t <= first.t {
            return first.value * t as f64;
        }
        if t >= last.t {
            return to_first + self.cum[self.keys.len() - 1] + last.value * (t - last.t) as f64;
        }
        match self.segment(t) {
            Some(i) => to_first + self.cum[i] + self.segment_area(i, t),
            None => first.value * t as f64,
        }
    }

    /// The part of the curve from `a` to `b`, moved so `a` becomes time 0.
    ///
    /// The result evaluates exactly like the original did between `a` and `b`. `b` may be
    /// beyond the last key and `a` before the first: the curve then holds its end values, as
    /// the original did. The result is never empty unless the original was.
    pub fn slice(&self, a: i64, b: i64) -> Curve {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return Curve::new();
        };
        let mut out: Vec<Key> = Vec::new();
        // The first and last original keys kept lose a neighbour to the cut, so their automatic
        // tangents are frozen at what they were. Keys with both neighbours kept stay automatic.
        let kept: Vec<usize> = (0..self.keys.len())
            .filter(|&i| self.keys[i].t >= a && self.keys[i].t <= b)
            .collect();
        let frozen = |i: usize| -> Key {
            let mut k = self.keys[i];
            if kept.first() == Some(&i) || kept.last() == Some(&i) {
                k.tangent = Some(self.tangent_at(i));
            }
            k
        };
        let inside = |t: i64| t > first.t && t < last.t;
        let has_key = |t: i64| self.keys.binary_search_by_key(&t, |k| k.t).is_ok();

        if inside(a) && !has_key(a) {
            if let Some(i) = self.segment(a) {
                out.push(Key {
                    t: 0,
                    value: self.segment_value(i, a),
                    interp: self.keys[i].interp,
                    tangent: Some(self.segment_slope(i, a)),
                });
            }
        }
        for i in 0..self.keys.len() {
            let k = self.keys[i];
            if k.t >= a && k.t <= b {
                let mut f = frozen(i);
                f.t = k.t - a;
                out.push(f);
            }
        }
        if inside(b) && !has_key(b) {
            if let Some(i) = self.segment(b) {
                out.push(Key {
                    t: b - a,
                    value: self.segment_value(i, b),
                    interp: Interp::Linear,
                    tangent: Some(self.segment_slope(i, b)),
                });
            }
        }
        if out.is_empty() {
            out.push(Key::new(0, self.eval(a).unwrap_or(0.0), Interp::Hold));
        }
        let mut c = Curve {
            keys: out,
            cum: Vec::new(),
        };
        c.refresh();
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(keys: &[(i64, f64, Interp)]) -> Curve {
        Curve::from_keys(keys.iter().map(|&(t, v, i)| Key::new(t, v, i)).collect()).unwrap()
    }

    fn numeric_integral(c: &Curve, t: i64) -> f64 {
        // Simpson over many steps, from 0 to t.
        let n = 20_000;
        let h = t as f64 / f64::from(n);
        let f = |x: f64| c.eval(x.round() as i64).unwrap_or(0.0);
        let mut s = f(0.0) + f(t as f64);
        for i in 1..n {
            s += f(h * f64::from(i)) * if i % 2 == 1 { 4.0 } else { 2.0 };
        }
        s * h / 3.0
    }

    const ALL: [Interp; 3] = [Interp::Hold, Interp::Linear, Interp::Smooth];

    #[test]
    fn values_before_between_and_after() {
        let c = curve(&[
            (100, 1.0, Interp::Linear),
            (200, 3.0, Interp::Hold),
            (300, 0.0, Interp::Linear),
        ]);
        assert_eq!(
            c.eval(0),
            Some(1.0),
            "holds the first value before the first key"
        );
        assert_eq!(c.eval(100), Some(1.0));
        assert_eq!(c.eval(150), Some(2.0));
        assert_eq!(c.eval(200), Some(3.0));
        assert_eq!(
            c.eval(250),
            Some(3.0),
            "hold keeps the value until the next key"
        );
        assert_eq!(c.eval(300), Some(0.0));
        assert_eq!(c.eval(9999), Some(0.0));
        assert_eq!(Curve::new().eval(5), None);
    }

    #[test]
    fn smooth_passes_through_keys_eases_at_the_ends_and_never_overshoots() {
        let c = curve(&[
            (0, 0.0, Interp::Smooth),
            (100, 10.0, Interp::Smooth),
            (200, 11.0, Interp::Smooth),
            (300, 0.0, Interp::Smooth),
        ]);
        for k in c.keys() {
            assert!((c.eval(k.t).unwrap() - k.value).abs() < 1e-9);
        }
        // Eases in: slow at the very start.
        assert!(c.eval(5).unwrap() < 0.25);
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for t in 0..=300 {
            let v = c.eval(t).unwrap();
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(lo >= -1e-9 && hi <= 11.0 + 1e-9, "{lo} {hi}");
        // The peak at the middle key is flat (automatic tangents are zero at a turning point).
        let d = c.eval(205).unwrap() - c.eval(195).unwrap();
        assert!(d.abs() < 0.2, "{d}");
        // With only two keys it is a smoothstep.
        let c = curve(&[(0, 0.0, Interp::Smooth), (100, 1.0, Interp::Linear)]);
        assert!((c.eval(50).unwrap() - 0.5).abs() < 1e-9);
        assert!((c.eval(25).unwrap() - (3.0 * 0.0625 - 2.0 * 0.015_625)).abs() < 1e-9);
    }

    #[test]
    fn integrals_are_exact() {
        let c = curve(&[
            (1000, 1.0, Interp::Linear),
            (3000, 3.0, Interp::Smooth),
            (5000, 0.5, Interp::Hold),
            (7000, 2.0, Interp::Linear),
        ]);
        for t in [
            -500, 0, 500, 1000, 1500, 2999, 3000, 4000, 5000, 6000, 7000, 9000,
        ] {
            let exact = c.integral(t);
            let approx = if t >= 0 {
                numeric_integral(&c, t)
            } else {
                c.eval(0).unwrap() * t as f64 // before time zero the first value just carries on
            };
            assert!(
                (exact - approx).abs() < 1e-3 * (1.0 + approx.abs()),
                "t={t}: {exact} vs {approx}"
            );
        }
        // A constant speed of 2 over 1000 ticks is 2000.
        let c = curve(&[(0, 2.0, Interp::Linear)]);
        assert_eq!(c.integral(1000), 2000.0);
        assert_eq!(Curve::new().integral(1000), 0.0);
    }

    #[test]
    fn integral_is_additive_and_increasing_for_positive_values() {
        let c = curve(&[
            (0, 0.5, Interp::Smooth),
            (1000, 2.0, Interp::Smooth),
            (2000, 0.25, Interp::Linear),
        ]);
        let mut last = 0.0;
        for t in (0..=3000).step_by(50) {
            let i = c.integral(t);
            assert!(i >= last);
            last = i;
        }
        assert!(
            (c.integral(2500) - (c.integral(700) + (c.integral(2500) - c.integral(700)))).abs()
                < 1e-9
        );
    }

    #[test]
    fn slices_evaluate_exactly_like_the_original() {
        let c = curve(&[
            (100, 0.0, Interp::Smooth),
            (400, 5.0, Interp::Linear),
            (700, 2.0, Interp::Smooth),
            (1000, 8.0, Interp::Hold),
            (1300, 1.0, Interp::Smooth),
            (1600, 4.0, Interp::Linear),
        ]);
        for &(a, b) in &[
            (0, 2000),
            (250, 850),
            (400, 1000),
            (555, 556),
            (1100, 1500),
            (1700, 1800),
            (-50, 120),
            (650, 1450),
        ] {
            let s = c.slice(a, b);
            assert!(!s.is_empty());
            for t in (a..=b).step_by(7) {
                let want = c.eval(t).unwrap();
                let got = s.eval(t - a).unwrap();
                assert!(
                    (want - got).abs() < 1e-9,
                    "slice({a},{b}) at {t}: {want} vs {got}"
                );
            }
        }
    }

    #[test]
    fn slices_keep_areas_too() {
        let c = curve(&[
            (0, 1.0, Interp::Smooth),
            (500, 3.0, Interp::Smooth),
            (1000, 0.5, Interp::Linear),
        ]);
        let s = c.slice(200, 800);
        let want = c.integral(700) - c.integral(200);
        let got = s.integral(500) - s.integral(0);
        assert!((want - got).abs() < 1e-6, "{want} vs {got}");
    }

    #[test]
    fn slicing_past_the_keys_keeps_a_value() {
        let c = curve(&[(0, 1.0, Interp::Linear), (100, 5.0, Interp::Linear)]);
        let s = c.slice(500, 600);
        assert_eq!(s.keys().len(), 1);
        assert_eq!(s.eval(10), Some(5.0));
        let s = c.slice(-300, -100);
        assert_eq!(s.eval(0), Some(1.0));
        assert!(Curve::new().slice(0, 10).is_empty());
    }

    #[test]
    fn setting_replacing_removing_and_shifting() {
        let mut c = Curve::new();
        c.set(200, 2.0, Interp::Linear).unwrap();
        c.set(100, 1.0, Interp::Linear).unwrap();
        c.set(200, 4.0, Interp::Hold).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.keys()[0].t, 100);
        assert_eq!(c.keys()[1].value, 4.0);
        assert!(c.set(0, f64::NAN, Interp::Linear).is_err());
        assert!(c.remove(100));
        assert!(!c.remove(100));
        c.shift(50);
        assert_eq!(c.keys()[0].t, 250);
    }

    #[test]
    fn bad_key_lists_are_refused() {
        let k = |t, v| Key::new(t, v, Interp::Linear);
        assert_eq!(
            Curve::from_keys(vec![k(5, 0.0), k(5, 1.0)]).unwrap_err(),
            CurveError::NotSorted
        );
        assert_eq!(
            Curve::from_keys(vec![k(6, 0.0), k(5, 1.0)]).unwrap_err(),
            CurveError::NotSorted
        );
        assert_eq!(
            Curve::from_keys(vec![k(1, f64::INFINITY)]).unwrap_err(),
            CurveError::NotFinite
        );
        assert!(Curve::from_keys(vec![]).unwrap().is_empty());
    }

    #[test]
    fn interp_names_round_trip() {
        for i in ALL {
            assert_eq!(Interp::parse(i.as_str()), Some(i));
        }
        assert_eq!(Interp::parse("bouncy"), None);
    }
}
