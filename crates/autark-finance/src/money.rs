//! Pesos as whole centavos. Floats drift (0.1 + 0.2), and a budget that is off by a centavo after a year of
//! sums is a budget nobody trusts, so every amount is an integer and rounds once, where it is made.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Money(i64);

/// On disk an amount is written in pesos (`1200`, `960.5`), the way a person would type it in a config file or read it in
/// a bill, not as the centavos it is held in.
impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.pesos_f64())
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Pesos;
        impl Visitor<'_> for Pesos {
            type Value = Money;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an amount in pesos")
            }

            fn visit_i64<E: de::Error>(self, pesos: i64) -> Result<Money, E> {
                pesos.checked_mul(100).map(Money).ok_or_else(|| E::custom("amount too large"))
            }

            fn visit_u64<E: de::Error>(self, pesos: u64) -> Result<Money, E> {
                i64::try_from(pesos).map_err(|_| E::custom("amount too large")).and_then(|pesos| self.visit_i64(pesos))
            }

            fn visit_f64<E: de::Error>(self, pesos: f64) -> Result<Money, E> {
                if pesos.is_finite() && pesos.abs() < 1e15 { Ok(Money::from_pesos_f64(pesos)) } else { Err(E::custom("not an amount")) }
            }
        }
        deserializer.deserialize_any(Pesos)
    }
}

impl Money {
    pub const ZERO: Money = Money(0);

    pub const fn from_centavos(centavos: i64) -> Self {
        Money(centavos)
    }

    pub const fn from_pesos(pesos: i64) -> Self {
        Money(pesos * 100)
    }

    /// Pesos as a float (for example a bill's amount times a rate), rounded to the nearest centavo.
    pub fn from_pesos_f64(pesos: f64) -> Self {
        if pesos.is_finite() { Money((pesos * 100.0).round() as i64) } else { Money::ZERO }
    }

    pub const fn centavos(self) -> i64 {
        self.0
    }

    pub fn pesos_f64(self) -> f64 {
        self.0 as f64 / 100.0
    }

    /// Shared between `parts` (days, say), rounded to the nearest centavo.
    pub fn split(self, parts: i64) -> Money {
        if parts == 0 { Money::ZERO } else { Money((self.0 as f64 / parts as f64).round() as i64) }
    }

    pub fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// Zero if below zero.
    pub fn max_zero(self) -> Money {
        Money(self.0.max(0))
    }

    pub fn abs(self) -> Money {
        Money(self.0.abs())
    }

    /// Reads what a person types: `1234`, `1,234.50`, `₱1,234.5`, `PHP 1234`, `-50`. At most two decimals.
    pub fn parse(text: &str) -> Option<Money> {
        let mut text = text.trim();
        let mut negative = false;
        if let Some(rest) = text.strip_prefix('-') {
            negative = true;
            text = rest.trim_start();
        }
        let text = text.trim_start_matches('₱');
        let text = match text.get(..3) {
            Some(prefix) if prefix.eq_ignore_ascii_case("php") => &text[3..],
            _ => text,
        };
        let text: String = text.chars().filter(|c| !c.is_whitespace() && *c != ',').collect();
        let (whole, fraction) = match text.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (text.as_str(), ""),
        };
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }
        if fraction.len() > 2 || !whole.chars().all(|c| c.is_ascii_digit()) || !fraction.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let whole: i64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
        let fraction: i64 = match fraction.len() {
            0 => 0,
            1 => fraction.parse::<i64>().ok()? * 10,
            _ => fraction.parse().ok()?,
        };
        let centavos = whole.checked_mul(100)?.checked_add(fraction)?;
        Some(Money(if negative { -centavos } else { centavos }))
    }
}

impl fmt::Display for Money {
    /// `₱1,234` when there are no centavos, `₱1,234.50` when there are, `-₱50` below zero.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.0.unsigned_abs();
        let (pesos, centavos) = (total / 100, total % 100);
        let digits = pesos.to_string();
        let mut grouped = String::new();
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        let sign = if self.0 < 0 { "-" } else { "" };
        if centavos == 0 { write!(f, "{sign}₱{grouped}") } else { write!(f, "{sign}₱{grouped}.{centavos:02}") }
    }
}

impl Add for Money {
    type Output = Money;
    fn add(self, other: Money) -> Money {
        Money(self.0 + other.0)
    }
}

impl Sub for Money {
    type Output = Money;
    fn sub(self, other: Money) -> Money {
        Money(self.0 - other.0)
    }
}

impl Neg for Money {
    type Output = Money;
    fn neg(self) -> Money {
        Money(-self.0)
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, other: Money) {
        self.0 += other.0;
    }
}

impl SubAssign for Money {
    fn sub_assign(&mut self, other: Money) {
        self.0 -= other.0;
    }
}

impl std::iter::Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Money {
        iter.fold(Money::ZERO, |a, b| a + b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_people_type() {
        assert_eq!(Money::parse("2800.5"), Some(Money::from_centavos(280_050)));
        assert_eq!(Money::parse("12,345.67"), Some(Money::from_centavos(1_234_567)));
        assert_eq!(Money::parse("₱ 1,599"), Some(Money::from_pesos(1599)));
        assert_eq!(Money::parse("php340"), Some(Money::from_pesos(340)));
        assert_eq!(Money::parse("-50"), Some(Money::from_pesos(-50)));
        assert_eq!(Money::parse(".5"), Some(Money::from_centavos(50)));
        assert_eq!(Money::parse("7."), Some(Money::from_pesos(7)));
    }

    #[test]
    fn refuses_what_is_not_an_amount() {
        for bad in ["", " ", "abc", "1.234", "1.2.3", "12e3", "₱", "-", "1,2a"] {
            assert_eq!(Money::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn shows_pesos_with_thousands_and_only_needed_centavos() {
        assert_eq!(Money::from_pesos(1200).to_string(), "₱1,200");
        assert_eq!(Money::from_centavos(280_050).to_string(), "₱2,800.50");
        assert_eq!(Money::from_centavos(5).to_string(), "₱0.05");
        assert_eq!(Money::from_pesos(-50).to_string(), "-₱50");
        assert_eq!(Money::from_pesos(1_234_567).to_string(), "₱1,234,567");
        assert_eq!(Money::ZERO.to_string(), "₱0");
    }

    #[test]
    fn sums_exactly() {
        // A paycheck, less rent, a loan payment, a savings goal and laundry.
        let paid = [Money::from_pesos(2500), Money::from_centavos(280_050), Money::from_pesos(5000), Money::from_pesos(170)];
        let total: Money = paid.into_iter().sum();
        assert_eq!(Money::from_centavos(1_200_000) - total, Money::from_centavos(152_950));
    }

    #[test]
    fn is_written_in_pesos_and_read_back_exactly() {
        let amount = Money::from_centavos(96_050);
        assert_eq!(serde_json::to_string(&amount).unwrap(), "960.5");
        assert_eq!(serde_json::from_str::<Money>("960.5").unwrap(), amount);
        assert_eq!(serde_json::from_str::<Money>("1200").unwrap(), Money::from_pesos(1200));
        for centavos in [0, 1, 7, 99, 100, 123_456_789, -5_050] {
            let money = Money::from_centavos(centavos);
            assert_eq!(serde_json::from_str::<Money>(&serde_json::to_string(&money).unwrap()).unwrap(), money);
        }
        assert!(serde_json::from_str::<Money>("\"12\"").is_err());
        assert!(serde_json::from_str::<Money>("1e30").is_err());
    }

    #[test]
    fn splits_between_days() {
        assert_eq!(Money::from_centavos(294_711).split(15), Money::from_centavos(19_647));
        assert_eq!(Money::from_pesos(100).split(3), Money::from_centavos(3_333));
        assert_eq!(Money::from_pesos(5).split(0), Money::ZERO);
        assert_eq!(Money::from_pesos(-30).split(3), Money::from_pesos(-10));
    }

    #[test]
    fn rounds_floats_once() {
        assert_eq!(Money::from_pesos_f64(1149.996), Money::from_centavos(115_000));
        assert_eq!(Money::from_pesos_f64(f64::NAN), Money::ZERO);
        assert_eq!(Money::from_pesos_f64(0.1 + 0.2), Money::from_centavos(30));
    }
}
