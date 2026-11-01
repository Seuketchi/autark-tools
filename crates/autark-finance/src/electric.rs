//! Electricity: meter readings and bills in, a projected bill out.
//!
//! The meter's reading is the only thing it trusts. A cycle's usage is the latest reading minus the meter at the
//! cycle's start, so a month with no readings says so instead of guessing from last month. Until a few days of
//! readings exist the projection is marked early, and falls back to the appliances you listed when there are any.

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::obligation::{Confidence, Obligation};
use crate::recommend::Thresholds;

/// Fewer days of readings than this and the pace is not yet worth projecting a month from.
pub const EARLY_DAYS: i64 = 5;
/// With this many days of readings the last week's pace counts as much as the whole cycle's.
const BLEND_DAYS: i64 = 10;
/// The latest `RECENT_DAYS` of readings are the "last week".
const RECENT_DAYS: i64 = 7;

/// One look at the meter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub date: NaiveDate,
    pub kwh: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// What the landlord or the power company charged for a period.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bill {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub kwh: f64,
    pub amount: Money,
}

impl Bill {
    /// Pesos per kWh with every fee included, so it is what the next kWh really costs on average.
    pub fn rate(&self) -> Option<f64> {
        (self.kwh > 0.0).then(|| self.amount.pesos_f64() / self.kwh)
    }
}

/// Something that draws power, for the early estimate and for "what if I add this".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Appliance {
    pub name: String,
    pub watts: f64,
    pub hours_per_day: f64,
}

impl Appliance {
    pub fn kwh_per_day(&self) -> f64 {
        kwh_per_day(self.watts, self.hours_per_day)
    }
}

pub fn kwh_per_day(watts: f64, hours_per_day: f64) -> f64 {
    (watts.max(0.0) * hours_per_day.clamp(0.0, 24.0)) / 1000.0
}

/// What a new appliance adds to a cycle of `days`: its kWh, and its cost at `rate` pesos per kWh.
pub fn what_if(watts: f64, hours_per_day: f64, rate: f64, days: i64) -> (f64, Money) {
    let kwh = kwh_per_day(watts, hours_per_day) * days as f64;
    (kwh, Money::from_pesos_f64(kwh * rate))
}

/// The `[electric]` table of `budget.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// What you mean to spend on electricity in a cycle.
    pub budget: Money,
    /// The day of the month a cycle starts (1 to 28, so every month has it).
    pub billing_day: u32,
    /// Pesos per kWh until a bill gives the real one.
    pub fallback_rate: f64,
    #[serde(rename = "appliance")]
    pub appliances: Vec<Appliance>,
    /// When a tip speaks up; see `recommend`.
    pub thresholds: Thresholds,
}

impl Default for Config {
    fn default() -> Self {
        Config { budget: Money::from_pesos(1200), billing_day: 1, fallback_rate: 12.0, appliances: Vec::new(), thresholds: Thresholds::default() }
    }
}

impl Config {
    pub fn billing_day(&self) -> u32 {
        self.billing_day.clamp(1, 28)
    }
}

/// A billing cycle: `start` through the day before `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cycle {
    pub start: NaiveDate,
    /// The first day of the next cycle.
    pub end: NaiveDate,
}

impl Cycle {
    /// The cycle that `day` falls in, for a cycle that starts on `billing_day` (1 to 28) each month.
    pub fn containing(day: NaiveDate, billing_day: u32) -> Cycle {
        let billing_day = billing_day.clamp(1, 28);
        let (mut year, mut month) = (day.year(), day.month());
        if day.day() < billing_day {
            (year, month) = shift(year, month, -1);
        }
        let start = NaiveDate::from_ymd_opt(year, month, billing_day).unwrap_or(day);
        let (next_year, next_month) = shift(year, month, 1);
        let end = NaiveDate::from_ymd_opt(next_year, next_month, billing_day).unwrap_or(day);
        Cycle { start, end }
    }

    pub fn days(&self) -> i64 {
        (self.end - self.start).num_days()
    }
}

fn shift(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + month as i32 - 1 + delta;
    (total.div_euclid(12), total.rem_euclid(12) as u32 + 1)
}

/// Where the projection's kWh come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// The meter, with enough days to trust the pace.
    Readings,
    /// The meter, but only a few days of it.
    Early,
    /// The appliances you listed, because the meter has too little to go on yet.
    Appliances,
    /// Nothing to project from: no readings this cycle and no appliances.
    None,
}

/// Where the rate comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateSource {
    /// The newest bill's pesos per kWh.
    Bill,
    /// The `fallback_rate` setting, until a bill is added.
    Fallback,
}

/// The cycle so far and the month ahead.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub cycle: Cycle,
    pub today: NaiveDate,
    pub basis: Basis,
    pub rate: f64,
    pub rate_source: RateSource,
    /// kWh used in the cycle so far, by the meter.
    pub used_kwh: f64,
    /// Days between the first and the latest reading counted.
    pub span_days: i64,
    pub daily_kwh: f64,
    pub projected_kwh: f64,
    pub projected_cost: Money,
    pub budget: Money,
    /// Budget less the projected cost; negative is over.
    pub margin: Money,
    /// kWh a day you can still use and end the cycle on budget (0 when you are already past it).
    pub allowance_kwh_per_day: f64,
    pub days_left: i64,
}

impl Plan {
    pub fn is_over(&self) -> bool {
        self.margin.is_negative()
    }

    /// What the budget planner sees: a line item with how sure it is.
    pub fn obligation(&self) -> Obligation {
        let (amount, confidence) = match self.basis {
            Basis::Readings => (self.projected_cost, Confidence::Estimate),
            Basis::Early | Basis::Appliances => (self.projected_cost, Confidence::Guess),
            Basis::None => (self.budget, Confidence::Guess),
        };
        Obligation { name: "Electricity".into(), amount, due: Some(self.cycle.end - Duration::days(1)), confidence }
    }
}

/// The meter at the start of the cycle and the newest reading since, as (date, kWh) points.
///
/// If a reading falls on the cycle's first day that is the start. If readings surround it, the start is worked out
/// in between, so a reading taken before the cycle began does not put last month's use into this one. If there is
/// none before it, the start is the first reading in the cycle.
fn span(readings: &[Reading], cycle: Cycle, today: NaiveDate) -> Option<((NaiveDate, f64), (NaiveDate, f64))> {
    let usable: Vec<&Reading> = readings.iter().filter(|r| r.date <= today && r.kwh.is_finite()).collect();
    let before = usable.iter().rposition(|r| r.date <= cycle.start);
    let start = match before {
        Some(index) => {
            let prev = usable[index];
            if prev.date == cycle.start {
                (prev.date, prev.kwh)
            } else {
                let next = usable[index + 1..].iter().find(|r| r.date > prev.date)?;
                let share = (cycle.start - prev.date).num_days() as f64 / (next.date - prev.date).num_days() as f64;
                (cycle.start, prev.kwh + (next.kwh - prev.kwh) * share)
            }
        }
        None => {
            let first = usable.first()?;
            (first.date, first.kwh)
        }
    };
    let latest = usable.iter().rev().find(|r| r.date > start.0)?;
    Some((start, (latest.date, latest.kwh)))
}

/// kWh a day over the latest week of readings, when there are enough to say.
fn recent_pace(readings: &[Reading], latest: NaiveDate, today: NaiveDate) -> Option<f64> {
    let week: Vec<&Reading> = readings.iter().filter(|r| r.date <= today && r.date >= latest - Duration::days(RECENT_DAYS)).collect();
    let (first, last) = (week.first()?, week.last()?);
    let days = (last.date - first.date).num_days();
    (week.len() >= 2 && days >= 3 && last.kwh >= first.kwh).then(|| (last.kwh - first.kwh) / days as f64)
}

/// Projects the cycle `today` falls in. `readings` and `bills` may be in any order.
pub fn plan(config: &Config, readings: &[Reading], bills: &[Bill], today: NaiveDate) -> Plan {
    let cycle = Cycle::containing(today, config.billing_day());

    let mut readings = readings.to_vec();
    readings.sort_by_key(|r| r.date);

    let newest_bill = bills.iter().filter(|b| b.rate().is_some()).max_by_key(|b| b.to);
    let (rate, rate_source) = match newest_bill.and_then(Bill::rate) {
        Some(rate) => (rate, RateSource::Bill),
        None => (config.fallback_rate, RateSource::Fallback),
    };

    // The meter's pace, when it has moved forward over at least a day.
    let mut used_kwh = 0.0;
    let mut span_days = 0;
    let mut pace = None;
    if let Some(((start_date, start_kwh), (latest_date, latest_kwh))) = span(&readings, cycle, today) {
        let days = (latest_date - start_date).num_days();
        let used = latest_kwh - start_kwh;
        // A meter that went backwards was replaced or misread; there is nothing to learn from this span.
        if days > 0 && used >= 0.0 {
            used_kwh = used;
            span_days = days;
            let overall = used / days as f64;
            pace = Some(match recent_pace(&readings, latest_date, today) {
                Some(recent) if days >= BLEND_DAYS => (overall + recent) / 2.0,
                _ => overall,
            });
        }
    }

    let appliances: f64 = config.appliances.iter().map(Appliance::kwh_per_day).sum();
    let early = span_days < EARLY_DAYS;
    let (basis, daily_kwh) = match pace {
        Some(daily) if !early => (Basis::Readings, daily),
        Some(_) if appliances > 0.0 => (Basis::Appliances, appliances),
        Some(daily) => (Basis::Early, daily),
        None if appliances > 0.0 => (Basis::Appliances, appliances),
        None => (Basis::None, 0.0),
    };

    let projected_kwh = daily_kwh * cycle.days() as f64;
    let projected_cost = Money::from_pesos_f64(projected_kwh * rate);
    let budget = config.budget;
    let days_left = (cycle.end - today).num_days().max(1);
    let budget_kwh = if rate > 0.0 { budget.pesos_f64() / rate } else { 0.0 };
    let allowance_kwh_per_day = ((budget_kwh - used_kwh) / days_left as f64).max(0.0);

    Plan {
        cycle,
        today,
        basis,
        rate,
        rate_source,
        used_kwh,
        span_days,
        daily_kwh,
        projected_kwh,
        projected_cost,
        budget,
        margin: budget - projected_cost,
        allowance_kwh_per_day,
        days_left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn reading(date: NaiveDate, kwh: f64) -> Reading {
        Reading { date, kwh, note: String::new() }
    }

    #[test]
    fn a_cycle_starts_on_the_billing_day() {
        let cycle = Cycle::containing(day(2026, 11, 14), 1);
        assert_eq!((cycle.start, cycle.end), (day(2026, 11, 1), day(2026, 12, 1)));
        assert_eq!(cycle.days(), 30);
        // Before the billing day the cycle began last month, across a year end too.
        let cycle = Cycle::containing(day(2027, 1, 9), 15);
        assert_eq!((cycle.start, cycle.end), (day(2026, 12, 15), day(2027, 1, 15)));
        // The day itself starts the new one.
        assert_eq!(Cycle::containing(day(2026, 11, 15), 15).start, day(2026, 11, 15));
        // A billing day past 28 is held to 28 so February has it.
        assert_eq!(Cycle::containing(day(2027, 2, 28), 31).start, day(2027, 2, 28));
    }

    #[test]
    fn a_bills_rate_includes_every_fee() {
        let bill = Bill { from: day(2026, 10, 1), to: day(2026, 11, 1), kwh: 80.0, amount: Money::from_pesos(960) };
        assert_eq!(bill.rate(), Some(12.0));
        assert_eq!(Bill { kwh: 0.0, ..bill }.rate(), None);
    }

    #[test]
    fn what_if_prices_an_appliance() {
        // A 1,000 W rice cooker for an hour a day over a 30-day cycle: 30 kWh.
        let (kwh, cost) = what_if(1000.0, 1.0, 12.0, 30);
        assert_eq!(kwh, 30.0);
        assert_eq!(cost, Money::from_pesos(360));
        assert_eq!(kwh_per_day(50.0, 99.0), 1.2, "hours are held to 24");
    }

    #[test]
    fn no_readings_means_no_projection_not_a_guess() {
        let plan = plan(&Config::default(), &[], &[], day(2026, 11, 14));
        assert_eq!(plan.basis, Basis::None);
        assert_eq!(plan.projected_cost, Money::ZERO);
        assert_eq!(plan.obligation().amount, Money::from_pesos(1200), "the budget stands in");
        assert_eq!(plan.obligation().confidence, Confidence::Guess);
    }

    #[test]
    fn the_opening_reading_alone_is_not_usage() {
        let plan = plan(&Config::default(), &[reading(day(2026, 11, 1), 15.4)], &[], day(2026, 11, 1));
        assert_eq!(plan.basis, Basis::None);
        assert_eq!(plan.used_kwh, 0.0);
    }

    #[test]
    fn a_few_days_is_an_early_estimate() {
        // 15.4 on the 1st, 18.4 on the 4th: 1 kWh a day, 30 a month, but only three days to say so.
        let readings = [reading(day(2026, 11, 1), 15.4), reading(day(2026, 11, 4), 18.4)];
        let plan = plan(&Config::default(), &readings, &[], day(2026, 11, 4));
        assert_eq!(plan.basis, Basis::Early);
        assert_eq!(plan.span_days, 3);
        assert!((plan.used_kwh - 3.0).abs() < 1e-9);
        assert!((plan.projected_kwh - 30.0).abs() < 1e-9);
        assert_eq!(plan.rate_source, RateSource::Fallback);
        assert_eq!(plan.projected_cost, Money::from_pesos(360));
        assert_eq!(plan.obligation().confidence, Confidence::Guess);
    }

    #[test]
    fn early_falls_back_to_appliances_when_listed() {
        let config = Config { appliances: vec![Appliance { name: "Fridge".into(), watts: 100.0, hours_per_day: 10.0 }], ..Config::default() };
        let readings = [reading(day(2026, 11, 1), 15.4), reading(day(2026, 11, 3), 15.9)];
        let plan = plan(&config, &readings, &[], day(2026, 11, 3));
        assert_eq!(plan.basis, Basis::Appliances);
        assert!((plan.daily_kwh - 1.0).abs() < 1e-9);
        assert!((plan.used_kwh - 0.5).abs() < 1e-9, "usage still comes from the meter");
    }

    #[test]
    fn a_week_of_readings_is_trusted() {
        // 2 kWh a day for 8 days.
        let readings: Vec<_> = (0..=8).map(|n| reading(day(2026, 11, 1 + n), 15.4 + 2.0 * f64::from(n))).collect();
        let plan = plan(&Config::default(), &readings, &[], day(2026, 11, 9));
        assert_eq!(plan.basis, Basis::Readings);
        assert!((plan.daily_kwh - 2.0).abs() < 1e-9);
        assert!((plan.projected_kwh - 60.0).abs() < 1e-9);
        assert_eq!(plan.projected_cost, Money::from_pesos(720));
        assert_eq!(plan.margin, Money::from_pesos(480));
        assert!(!plan.is_over());
        assert_eq!(plan.obligation().confidence, Confidence::Estimate);
        assert_eq!(plan.obligation().amount, Money::from_pesos(720));
        assert_eq!(plan.obligation().due, Some(day(2026, 11, 30)), "due on the cycle's last day");
    }

    #[test]
    fn a_bills_rate_replaces_the_fallback_and_the_newest_wins() {
        let bills = [
            Bill { from: day(2026, 8, 1), to: day(2026, 9, 1), kwh: 100.0, amount: Money::from_pesos(1000) },
            Bill { from: day(2026, 9, 1), to: day(2026, 10, 1), kwh: 100.0, amount: Money::from_pesos(1100) },
        ];
        let plan = plan(&Config::default(), &[], &bills, day(2026, 11, 3));
        assert_eq!(plan.rate_source, RateSource::Bill);
        assert!((plan.rate - 11.0).abs() < 1e-9);
    }

    #[test]
    fn going_over_budget_shows_in_the_margin_and_the_allowance_is_zero() {
        // 4 kWh a day is 120 kWh, ₱1,440 at ₱12: ₱240 over.
        let readings: Vec<_> = (0..=10).map(|n| reading(day(2026, 11, 1 + n), 15.4 + 4.0 * f64::from(n))).collect();
        let plan = plan(&Config::default(), &readings, &[], day(2026, 11, 11));
        assert_eq!(plan.projected_cost, Money::from_pesos(1440));
        assert_eq!(plan.margin, Money::from_pesos(-240));
        assert!(plan.is_over());
        // ₱1,200 buys 100 kWh and 40 are used: 60 left over the 20 days from the 11th to the end of the month.
        assert_eq!(plan.days_left, 20);
        assert!((plan.allowance_kwh_per_day - 3.0).abs() < 1e-9);
        let spent = [reading(day(2026, 11, 1), 0.0), reading(day(2026, 11, 8), 140.0)];
        assert_eq!(super::plan(&Config::default(), &spent, &[], day(2026, 11, 8)).allowance_kwh_per_day, 0.0);
    }

    #[test]
    fn a_reading_from_last_month_does_not_put_last_months_use_in_this_one() {
        // 10 kWh on 21 Oct, 40 on 10 Nov: 1.5 a day, so the meter read 26.5 on 1 Nov, and this cycle has used 13.5.
        let readings = [reading(day(2026, 10, 21), 10.0), reading(day(2026, 11, 10), 40.0)];
        let plan = plan(&Config::default(), &readings, &[], day(2026, 11, 10));
        assert!((plan.used_kwh - 13.5).abs() < 1e-9);
        assert_eq!(plan.span_days, 9);
    }

    #[test]
    fn a_meter_that_goes_backwards_is_ignored() {
        let readings = [reading(day(2026, 11, 1), 500.0), reading(day(2026, 11, 8), 12.0)];
        let plan = plan(&Config::default(), &readings, &[], day(2026, 11, 8));
        assert_eq!(plan.basis, Basis::None);
    }

    #[test]
    fn readings_in_any_order_and_from_the_future_are_handled() {
        let readings = [reading(day(2026, 11, 9), 33.4), reading(day(2026, 11, 1), 15.4), reading(day(2026, 11, 20), 999.0)];
        let plan = plan(&Config::default(), &readings, &[], day(2026, 11, 9));
        assert!((plan.used_kwh - 18.0).abs() < 1e-9);
        assert_eq!(plan.span_days, 8);
    }

    #[test]
    fn the_last_week_counts_once_there_is_enough_history() {
        // 1 kWh a day for ten days, then 3 a day for the next five: the pace blends the two.
        let mut readings = vec![reading(day(2026, 11, 1), 0.0)];
        let mut meter = 0.0;
        for n in 1..=15u32 {
            meter += if n <= 10 { 1.0 } else { 3.0 };
            readings.push(reading(day(2026, 11, 1 + n), meter));
        }
        let plan = plan(&Config::default(), &readings, &[], day(2026, 11, 16));
        let overall = 25.0 / 15.0;
        // The week back from the 16th starts at the 9th, when the meter read 8.
        let recent = (25.0 - 8.0) / 7.0;
        assert!((plan.daily_kwh - (overall + recent) / 2.0).abs() < 1e-9);
    }
}
