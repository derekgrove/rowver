//! Exact decimal numbers: the `nice` fact's rule (docs/FORMAT.md §4), and a JSON number read as
//! the value it writes, whatever its spelling.

/// The number `m × 10^p`, with no trailing zero in `m` (zero is `0 × 10^0`), so that two equal
/// numbers are equal values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Decimal {
    m: i128,
    p: i32,
}

/// How many decimal digits `m` has (1 for 0).
fn digits(m: i128) -> u32 {
    m.unsigned_abs().checked_ilog10().map_or(1, |log| log + 1)
}

impl Decimal {
    /// `m × 10^p`, its trailing zeros taken into `p`.
    fn new(mut m: i128, mut p: i32) -> Decimal {
        if m == 0 {
            return Decimal { m: 0, p: 0 };
        }
        while m % 10 == 0 {
            m /= 10;
            p += 1;
        }
        Decimal { m, p }
    }

    /// A whole number.
    pub(crate) fn whole(x: i128) -> Decimal {
        Decimal::new(x, 0)
    }

    /// A double's shortest decimal: the digits canonical JSON writes for it.
    pub(crate) fn of(x: f64) -> Decimal {
        Decimal::parse(&crate::json::float(x)).expect("canonical JSON writes a decimal number")
    }

    /// The JSON number `text` exactly, when its significant digits fit (38 of them).
    pub(crate) fn parse(text: &str) -> Option<Decimal> {
        let (negative, rest) = text.strip_prefix('-').map_or((false, text), |rest| (true, rest));
        let (number, exponent) = match rest.split_once(['e', 'E']) {
            Some((number, exponent)) => (number, exponent.parse::<i32>().ok()?),
            None => (rest, 0),
        };
        let (int, fraction) = number.split_once('.').unwrap_or((number, ""));
        if int.is_empty() || !int.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        let all = format!("{int}{fraction}");
        let significant = all.trim_start_matches('0').trim_end_matches('0');
        if significant.is_empty() {
            return Some(Decimal { m: 0, p: 0 });
        }
        if significant.len() > 38 {
            return None;
        }
        let after = all.len() - all.trim_end_matches('0').len();
        let p = exponent.checked_sub(i32::try_from(fraction.len()).ok()?)?.checked_add(after as i32)?;
        let m: i128 = significant.parse().ok()?;
        Some(Decimal { m: if negative { -m } else { m }, p })
    }

    /// The whole number it is, when it is one that fits.
    pub(crate) fn to_whole(self) -> Option<i128> {
        u32::try_from(self.p).ok().and_then(|p| 10_i128.checked_pow(p)).and_then(|scale| scale.checked_mul(self.m))
    }

    /// The double nearest to it; past the largest double, the largest.
    pub(crate) fn to_f64(self) -> f64 {
        let x: f64 = format!("{}e{}", self.m, self.p).parse().expect("digits and an exponent read as a double");
        if x.is_infinite() {
            f64::MAX.copysign(x)
        } else {
            x
        }
    }

    /// `10^(e − 1)`, `e` the power of ten of its first significant digit.
    fn step(self) -> Decimal {
        Decimal { m: 1, p: self.p + digits(self.m) as i32 - 2 }
    }

    /// The multiple of its step at or below it, or at or above it.
    fn outward(self, up: bool) -> Decimal {
        let n = digits(self.m);
        if self.m == 0 || n <= 2 {
            return self;
        }
        let cut = 10_i128.pow(n - 2);
        let away = self.m % cut != 0 && up == (self.m > 0);
        Decimal::new(self.m / cut + if away { self.m.signum() } else { 0 }, self.p + n as i32 - 2)
    }

    /// The sum of two small decimals.
    fn add(self, other: Decimal) -> Decimal {
        let p = self.p.min(other.p);
        let scaled = |d: Decimal| d.m * 10_i128.pow((d.p - p) as u32);
        Decimal::new(scaled(self) + scaled(other), p)
    }
}

/// `nice` of `lim`'s ends: each rounded outward to two significant digits, and the high end raised
/// by its step when the two are then equal (by 1 when it is 0, or the kind holds whole numbers and
/// its step is below 1).
pub(crate) fn nice(low: Decimal, high: Decimal, whole: bool) -> [Decimal; 2] {
    let (low, mut high) = (low.outward(false), high.outward(true));
    if low == high {
        let step = high.step();
        high = high.add(if high.m == 0 || (whole && step.p < 0) { Decimal::whole(1) } else { step });
    }
    [low, high]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_rounds_each_end_outward_to_two_digits_in_decimal() {
        let floats = |lo: f64, hi: f64| nice(Decimal::of(lo), Decimal::of(hi), false).map(Decimal::to_f64);
        let wholes = |lo: i128, hi: i128| nice(Decimal::whole(lo), Decimal::whole(hi), true).map(|d| d.to_whole().unwrap());
        assert_eq!(floats(0.3381, 373.67), [0.33, 380.0]);
        assert_eq!(floats(101.703125, 154.046875), [100.0, 160.0]);
        assert_eq!(floats(33.901041666666664, 120.5), [33.0, 130.0]);
        assert_eq!(floats(0.1, 0.30000000000000004), [0.1, 0.31]);
        assert_eq!(floats(f64::from(1.1_f32), f64::from(1.1_f32)), [1.1, 1.2]);
        assert_eq!(floats(-1.2, -1.2), [-1.2, -1.1]);
        assert_eq!(floats(5.0, 5.0), [5.0, 5.1]);
        assert_eq!(floats(0.0, 0.0), [0.0, 1.0]);
        assert_eq!(floats(-0.037, 41.0), [-0.037, 41.0]);
        assert_eq!(floats(150.1, 1583.2), [150.0, 1600.0]);
        assert_eq!(floats(0.0, f64::MAX), [0.0, f64::MAX]);
        assert_eq!(wholes(5, 5), [5, 6]);
        assert_eq!(wholes(1, 1), [1, 2]);
        assert_eq!(wholes(12, 12), [12, 13]);
        assert_eq!(wholes(120, 120), [120, 130]);
        assert_eq!(wholes(5, 120000001), [5, 130000000]);
        assert_eq!(wholes(-120000001, -7), [-130000000, -7]);
        assert_eq!(wholes(0, u64::MAX.into()), [0, 19_000_000_000_000_000_000]);
        assert_eq!(wholes(i64::MIN.into(), i64::MAX.into()), [-9_300_000_000_000_000_000, 9_300_000_000_000_000_000]);
    }

    #[test]
    fn a_json_number_is_the_value_it_writes() {
        let whole = |text: &str| Decimal::parse(text).and_then(Decimal::to_whole);
        assert_eq!(whole("380"), Some(380));
        assert_eq!(whole("380.0"), Some(380));
        assert_eq!(whole("3.8e2"), Some(380));
        assert_eq!(whole("38000e-2"), Some(380));
        assert_eq!(whole("-0"), Some(0));
        assert_eq!(whole("0.000e5"), Some(0));
        assert_eq!(whole("9300000000000000000"), Some(9_300_000_000_000_000_000));
        assert_eq!(whole("1.5"), None);
        assert_eq!(whole("1e400"), None);
        assert_eq!(Decimal::parse("1.100000023841858").map(Decimal::to_f64), Some(f64::from(1.1_f32)));
        assert_eq!(Decimal::parse("1e99999999999"), None);
    }
}
