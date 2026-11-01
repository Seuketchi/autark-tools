//! Trends: how your use moves day to day and week to week, so you can see which part of your schedule to change.
//!
//! A meter gives totals, so a day's use is the difference between the readings around it. Days between two readings
//! that are more than a day apart share that use equally and are marked as spread, since they hide the real shape of
//! the days. The weekday pattern uses only days measured as single days.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::electric::{Cycle, Plan, Reading};
use crate::money::Money;

/// Days shown in the daily chart.
pub const RECENT_DAYS: usize = 14;
/// The weekday pattern looks back this many days.
const PATTERN_DAYS: i64 = 56;
/// A weekday needs this many measured days before its average means anything.
const PATTERN_SAMPLES: usize = 2;
/// And this many weekdays need an average before busiest and quietest are named.
const PATTERN_WEEKDAYS: usize = 5;

/// One day's use: what the meter moved in the day up to its reading.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: NaiveDate,
    pub kwh: f64,
    /// False when the day shares a gap between readings with others, so its figure is an average.
    pub measured: bool,
}

/// A week or a billing cycle: the days of it the readings cover, and what they used.
#[derive(Clone, Debug, PartialEq)]
pub struct Period {
    pub start: NaiveDate,
    /// Days of the period the readings cover.
    pub days: usize,
    /// Days the period has in all (7 for a week).
    pub length: usize,
    pub total_kwh: f64,
    pub kwh_per_day: f64,
    /// What those kWh cost at the current rate.
    pub cost: Money,
}

impl Period {
    /// Covers every day of it.
    pub fn full(&self) -> bool {
        self.days >= self.length
    }
}

/// Weeks shown, and billing cycles shown.
pub const WEEKS: usize = 8;
pub const CYCLES: usize = 6;

#[derive(Clone, Debug, PartialEq)]
pub struct Trends {
    /// The last `WEEKS` weeks (Monday to Sunday), oldest first. The newest may be a week in progress.
    pub weeks: Vec<Period>,
    /// The last `CYCLES` billing cycles, oldest first, by the cycle length of the plan.
    pub cycles: Vec<Period>,
    /// The last `RECENT_DAYS` days, oldest first.
    pub days: Vec<Day>,
    /// Average use by weekday, Monday first, where there are enough measured days.
    pub weekday: [Option<f64>; 7],
    /// kWh a day over the latest 7 days, and the 7 before them.
    pub this_week: Option<f64>,
    pub last_week: Option<f64>,
    /// How much more (positive) or less the latest 7 days used than the 7 before, as a fraction.
    pub week_change: Option<f64>,
    pub busiest: Option<(Weekday, f64)>,
    pub quietest: Option<(Weekday, f64)>,
    /// kWh a day the budget allows over a whole cycle.
    pub budget_kwh_per_day: f64,
    /// How many of the days shown went over that.
    pub days_over: usize,
    /// What cutting 1 kWh a day saves over a cycle.
    pub saved_per_kwh_day: Money,
}

/// Every day the readings cover, oldest first. Readings from after `today`, and a meter that went backwards, are left out.
fn daily(readings: &[Reading], today: NaiveDate) -> Vec<Day> {
    let mut sorted: Vec<&Reading> = readings.iter().filter(|r| r.date <= today && r.kwh.is_finite()).collect();
    sorted.sort_by_key(|r| r.date);
    // Two readings on one day: the later one entered stands.
    let mut points: Vec<(NaiveDate, f64)> = Vec::new();
    for reading in sorted {
        match points.last_mut() {
            Some(last) if last.0 == reading.date => last.1 = reading.kwh,
            _ => points.push((reading.date, reading.kwh)),
        }
    }
    let mut days = Vec::new();
    for pair in points.windows(2) {
        let ((from, before), (to, after)) = (pair[0], pair[1]);
        let gap = (to - from).num_days();
        if after < before {
            continue;
        }
        let each = (after - before) / gap as f64;
        for step in 1..=gap {
            days.push(Day { date: from + Duration::days(step), kwh: each, measured: gap == 1 });
        }
    }
    days
}

/// Groups days by whatever `key` says starts their period, oldest first.
fn periods(all: &[Day], rate: f64, key: impl Fn(NaiveDate) -> (NaiveDate, usize)) -> Vec<Period> {
    let mut found: Vec<Period> = Vec::new();
    for day in all {
        let (start, length) = key(day.date);
        match found.last_mut() {
            Some(last) if last.start == start => {
                last.days += 1;
                last.total_kwh += day.kwh;
            }
            _ => found.push(Period { start, days: 1, length, total_kwh: day.kwh, kwh_per_day: 0.0, cost: Money::ZERO }),
        }
    }
    for period in &mut found {
        period.kwh_per_day = period.total_kwh / period.days as f64;
        period.cost = Money::from_pesos_f64(period.total_kwh * rate);
    }
    found
}

fn mean(days: &[Day]) -> f64 {
    days.iter().map(|d| d.kwh).sum::<f64>() / days.len() as f64
}

pub fn trends(readings: &[Reading], plan: &Plan) -> Trends {
    let all = daily(readings, plan.today);

    let days: Vec<Day> = all[all.len().saturating_sub(RECENT_DAYS)..].to_vec();

    let billing_day = plan.cycle.start.day();
    let mut weeks = periods(&all, plan.rate, |date| (date - Duration::days(i64::from(date.weekday().num_days_from_monday())), 7));
    weeks.drain(..weeks.len().saturating_sub(WEEKS));
    let mut cycles = periods(&all, plan.rate, |date| {
        let cycle = Cycle::containing(date, billing_day);
        (cycle.start, cycle.days() as usize)
    });
    cycles.drain(..cycles.len().saturating_sub(CYCLES));

    let this_week = (all.len() >= 7).then(|| mean(&all[all.len() - 7..]));
    let last_week = (all.len() >= 14).then(|| mean(&all[all.len() - 14..all.len() - 7]));
    let week_change = match (this_week, last_week) {
        (Some(now), Some(before)) if before > 0.0 => Some((now - before) / before),
        _ => None,
    };

    let mut weekday = [None; 7];
    if let Some(latest) = all.last() {
        let since = latest.date - Duration::days(PATTERN_DAYS);
        for (index, slot) in weekday.iter_mut().enumerate() {
            let samples: Vec<f64> = all.iter().filter(|d| d.measured && d.date > since && d.date.weekday().num_days_from_monday() as usize == index).map(|d| d.kwh).collect();
            if samples.len() >= PATTERN_SAMPLES {
                *slot = Some(samples.iter().sum::<f64>() / samples.len() as f64);
            }
        }
    }
    let known: Vec<(Weekday, f64)> = weekday.iter().enumerate().filter_map(|(index, avg)| avg.map(|avg| (weekday_of(index), avg))).collect();
    let (busiest, quietest) = if known.len() >= PATTERN_WEEKDAYS {
        let by = |a: &&(Weekday, f64), b: &&(Weekday, f64)| a.1.total_cmp(&b.1);
        (known.iter().max_by(by).copied(), known.iter().min_by(by).copied())
    } else {
        (None, None)
    };

    let budget_kwh_per_day = if plan.rate > 0.0 && plan.cycle.days() > 0 { plan.budget.pesos_f64() / plan.rate / plan.cycle.days() as f64 } else { 0.0 };
    let days_over = days.iter().filter(|d| d.kwh > budget_kwh_per_day).count();

    Trends {
        weeks,
        cycles,
        days,
        weekday,
        this_week,
        last_week,
        week_change,
        busiest,
        quietest,
        budget_kwh_per_day,
        days_over,
        saved_per_kwh_day: Money::from_pesos_f64(plan.rate * plan.cycle.days() as f64),
    }
}

fn weekday_of(index: usize) -> Weekday {
    [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun][index % 7]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::electric::{Config, plan};

    fn day(n: u32) -> NaiveDate {
        // 1 Nov 2026 is a Sunday.
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap() + Duration::days(i64::from(n) - 1)
    }

    fn readings(uses: &[f64]) -> Vec<Reading> {
        let mut meter = 15.4;
        let mut all = vec![Reading { date: day(1), kwh: meter, note: String::new() }];
        for (index, used) in uses.iter().enumerate() {
            meter += used;
            all.push(Reading { date: day(2 + index as u32), kwh: meter, note: String::new() });
        }
        all
    }

    fn trends_for(readings: &[Reading], today: u32) -> Trends {
        super::trends(readings, &plan(&Config::default(), readings, &[], day(today)))
    }

    #[test]
    fn each_day_is_the_difference_between_its_readings() {
        let t = trends_for(&readings(&[2.0, 3.0, 1.0]), 4);
        let used: Vec<f64> = t.days.iter().map(|d| d.kwh).collect();
        for (got, want) in used.iter().zip([2.0, 3.0, 1.0]) {
            assert!((got - want).abs() < 1e-9);
        }
        assert!(t.days.iter().all(|d| d.measured));
        assert_eq!(t.days[0].date, day(2));
    }

    #[test]
    fn a_gap_between_readings_is_shared_and_marked_spread() {
        let all = [Reading { date: day(1), kwh: 10.0, note: String::new() }, Reading { date: day(4), kwh: 16.0, note: String::new() }];
        let t = trends_for(&all, 4);
        assert_eq!(t.days.len(), 3);
        assert!(t.days.iter().all(|d| !d.measured && (d.kwh - 2.0).abs() < 1e-9));
        assert_eq!(t.weekday, [None; 7], "spread days say nothing about weekdays");
    }

    #[test]
    fn a_meter_that_went_backwards_and_future_readings_are_left_out() {
        let all = [
            Reading { date: day(1), kwh: 500.0, note: String::new() },
            Reading { date: day(2), kwh: 10.0, note: String::new() },
            Reading { date: day(3), kwh: 12.0, note: String::new() },
            Reading { date: day(9), kwh: 99.0, note: String::new() },
        ];
        let t = trends_for(&all, 3);
        assert_eq!(t.days.len(), 1);
        assert!((t.days[0].kwh - 2.0).abs() < 1e-9);
    }

    #[test]
    fn this_week_against_the_week_before() {
        // Two weeks at 2 kWh a day, then a week at 3: 50% more.
        let mut uses = vec![2.0; 14];
        uses.extend([3.0; 7]);
        let t = trends_for(&readings(&uses), 22);
        assert!((t.this_week.unwrap() - 3.0).abs() < 1e-9);
        assert!((t.last_week.unwrap() - 2.0).abs() < 1e-9);
        assert!((t.week_change.unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(t.days.len(), RECENT_DAYS);
    }

    #[test]
    fn a_week_of_readings_has_no_comparison_yet() {
        let t = trends_for(&readings(&[2.0; 8]), 9);
        assert!(t.this_week.is_some());
        assert_eq!(t.last_week, None);
        assert_eq!(t.week_change, None);
    }

    #[test]
    fn weekends_show_up_as_the_busiest_days() {
        // 2 kWh on weekdays, 5 on Saturdays and Sundays, for four weeks. 2 Nov is the first Monday.
        let uses: Vec<f64> = (2..=29).map(|n| if matches!(day(n).weekday(), Weekday::Sat | Weekday::Sun) { 5.0 } else { 2.0 }).collect();
        let t = trends_for(&readings(&uses), 29);
        assert!(matches!(t.busiest, Some((Weekday::Sat | Weekday::Sun, kwh)) if (kwh - 5.0).abs() < 1e-9), "{:?}", t.busiest);
        assert!(matches!(t.quietest, Some((_, kwh)) if (kwh - 2.0).abs() < 1e-9), "{:?}", t.quietest);
        assert!((t.weekday[0].unwrap() - 2.0).abs() < 1e-9, "Monday");
    }

    #[test]
    fn the_budget_and_what_a_cut_saves() {
        // ₱1,200 at ₱12 is 100 kWh over 30 days: 3.33 a day. Cutting 1 kWh a day saves 30 kWh, ₱360.
        let t = trends_for(&readings(&[2.0, 4.0, 3.0, 5.0]), 6);
        assert!((t.budget_kwh_per_day - 100.0 / 30.0).abs() < 1e-9);
        assert_eq!(t.days_over, 2, "the 4 and the 5");
        assert_eq!(t.saved_per_kwh_day, Money::from_pesos(360));
    }

    #[test]
    fn weeks_run_monday_to_sunday_and_say_when_they_are_partial() {
        // 1 Nov is a Sunday, so the readings start with a lone Sunday-ending week (the 2nd belongs to the next Monday).
        let t = trends_for(&readings(&[2.0; 10]), 11);
        // Days 2 to 11: Mon 2 Nov to Wed 11 Nov. A full week (2 to 8) and three days of the next.
        assert_eq!(t.weeks.len(), 2);
        assert_eq!(t.weeks[0].start, day(2));
        assert!(t.weeks[0].full() && (t.weeks[0].kwh_per_day - 2.0).abs() < 1e-9);
        assert!(!t.weeks[1].full());
        assert_eq!(t.weeks[1].days, 3);
        assert_eq!(t.weeks[0].cost, Money::from_pesos(168), "14 kWh at ₱12");
    }

    #[test]
    fn only_the_latest_weeks_are_kept() {
        let t = trends_for(&readings(&[2.0; 90]), 91);
        assert_eq!(t.weeks.len(), WEEKS);
        assert!(t.weeks.last().unwrap().start > t.weeks[0].start);
    }

    #[test]
    fn billing_cycles_group_by_the_plans_cycle_start() {
        // Readings from 1 Nov for 45 days. The first reading only starts the count, so November's use is known for the
        // 29 days from the 2nd, and the cycle is not a full one.
        let t = trends_for(&readings(&[2.0; 45]), 46);
        assert_eq!(t.cycles.len(), 2);
        assert_eq!(t.cycles[0].start, day(1));
        assert_eq!(t.cycles[0].length, 30);
        assert_eq!(t.cycles[0].days, 29);
        assert!(!t.cycles[0].full());
        assert!((t.cycles[0].total_kwh - 58.0).abs() < 1e-9);
        assert!((t.cycles[0].kwh_per_day - 2.0).abs() < 1e-9);
        assert_eq!(t.cycles[1].start, NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
        assert!(!t.cycles[1].full());
    }

    #[test]
    fn no_readings_is_an_empty_trend_not_a_crash() {
        let t = trends_for(&[], 5);
        assert!(t.days.is_empty() && t.this_week.is_none() && t.busiest.is_none());
        assert!(t.weeks.is_empty() && t.cycles.is_empty());
        assert_eq!(t.days_over, 0);
    }
}
