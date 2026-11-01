//! "Can I afford this?": an amount set against what this payday has left, and when the next payday that could take it is.

use chrono::NaiveDate;

use crate::money::Money;
use crate::payday::{self, Estimate, Payment};
use crate::plan::Config;
use crate::spending::{self, Spend};

#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub amount: Money,
    /// What this payday has left, counting what you have spent.
    pub left: Money,
    pub days_left: i64,
    pub per_day_now: Money,
    /// Each day for the rest of the payday if you buy it.
    pub per_day_after: Money,
    pub fits: bool,
    /// How much it is over, when it does not fit.
    pub short_by: Money,
    /// It fits, but takes so much that each remaining day gets less than half of what it did.
    pub tight: bool,
    /// When it does not fit: the first later payday whose plan leaves enough, with what it leaves.
    pub next_fit: Option<(NaiveDate, Money)>,
}

/// `None` when there are no paydays set, or the amount is not more than nothing.
pub fn afford(config: &Config, payments: &[Payment], estimates: &[Estimate], spends: &[Spend], amount: Money, today: NaiveDate) -> Option<Verdict> {
    if amount.centavos() <= 0 {
        return None;
    }
    let periods = payday::periods(config, payments, estimates, today, 13);
    let current = periods.first()?;
    let summary = spending::summarize(current, spends, today);
    let (left, days_left) = (summary.left, summary.days_left);
    let after = left - amount;
    let per_day_now = left.split(days_left);
    let per_day_after = after.split(days_left);
    let fits = !after.is_negative();
    let tight = fits && per_day_now.centavos() > 0 && per_day_after.centavos() * 2 < per_day_now.centavos();
    let next_fit = if fits { None } else { periods.iter().skip(1).find(|period| period.left >= amount).map(|period| (period.start, period.left)) };
    Some(Verdict { amount, left, days_left, per_day_now, per_day_after, fits, short_by: if fits { Money::ZERO } else { -after }, tight, next_fit })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Every, Item};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn pesos(n: i64) -> Money {
        Money::from_pesos(n)
    }

    /// ₱10,000 a payday on the 15th and 30th, with ₱2,000 of laundry and rent every payday, so ₱8,000 is left.
    fn config() -> Config {
        let mut config = Config { paycheck: pesos(10_000), ..Config::default() };
        config.put(Item { name: "Rent".into(), amount: pesos(2000), every: Every::Payday, ..Item::default() }).unwrap();
        config
    }

    #[test]
    fn something_small_fits_and_says_what_each_day_gets() {
        // 15 Nov, the first day of a 15-day payday: ₱8,000 is ₱533.33 a day.
        let v = afford(&config(), &[], &[], &[], pesos(1500), date(2026, 11, 15)).unwrap();
        assert!(v.fits && !v.tight);
        assert_eq!((v.left, v.days_left), (pesos(8000), 15));
        assert_eq!(v.per_day_now, pesos(8000).split(15));
        assert_eq!(v.per_day_after, pesos(6500).split(15));
        assert_eq!((v.short_by, v.next_fit), (Money::ZERO, None));
    }

    #[test]
    fn what_you_already_spent_counts() {
        let spent = [Spend { date: date(2026, 11, 16), amount: pesos(7000), category: String::new(), note: String::new() }];
        let v = afford(&config(), &[], &[], &spent, pesos(1500), date(2026, 11, 17)).unwrap();
        assert_eq!(v.left, pesos(1000));
        assert!(!v.fits);
        assert_eq!(v.short_by, pesos(500));
    }

    #[test]
    fn something_too_big_names_the_payday_that_could_take_it() {
        let v = afford(&config(), &[], &[], &[], pesos(8500), date(2026, 11, 15)).unwrap();
        assert!(!v.fits);
        assert_eq!(v.short_by, pesos(500));
        // The next payday also leaves ₱8,000, so nothing later can take it either.
        assert_eq!(v.next_fit, None);

        // A payday that leaves more can: a bill of ₱3,000 only this payday leaves the next ₱10,000 to spend.
        let mut config = config();
        config.put(Item { name: "Tuition".into(), amount: pesos(3000), every: Every::Once, date: Some(date(2026, 11, 20)), ..Item::default() }).unwrap();
        let v = afford(&config, &[], &[], &[], pesos(6000), date(2026, 11, 15)).unwrap();
        assert!(!v.fits, "₱5,000 is left after the tuition");
        assert_eq!(v.next_fit, Some((date(2026, 11, 30), pesos(8000))));
    }

    #[test]
    fn a_purchase_that_eats_most_of_the_rest_is_tight() {
        let v = afford(&config(), &[], &[], &[], pesos(7000), date(2026, 11, 15)).unwrap();
        assert!(v.fits && v.tight, "₱1,000 over 15 days is well under half of ₱8,000 over 15");
        assert!(!afford(&config(), &[], &[], &[], pesos(3000), date(2026, 11, 15)).unwrap().tight);
    }

    #[test]
    fn nothing_without_paydays_or_an_amount() {
        assert_eq!(afford(&Config { pay_days: vec![], ..config() }, &[], &[], &[], pesos(10), date(2026, 11, 15)), None);
        assert_eq!(afford(&config(), &[], &[], &[], Money::ZERO, date(2026, 11, 15)), None);
        assert_eq!(afford(&config(), &[], &[], &[], pesos(-5), date(2026, 11, 15)), None);
    }
}
