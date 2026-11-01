//! The plan, payday by payday. Each payday starts a period that runs to the next one, and everything that falls due
//! in it comes out of that paycheck: what is left is what you can spend, and divided by the days it has to last, what
//! you can spend each day.
//!
//! An item belongs to the period its due date falls in, which is the payday just before it. So with paydays on the
//! 15th and the last day, rent due on the last day comes out of the paycheck of that day, and internet due on the
//! 15th out of the paycheck of that day.

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::obligation::Confidence;
use crate::plan::{Config, Every, Item, Kind};
use crate::spending::Spend;

/// You marked something paid: how much, when, and which payday's paycheck it came out of.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Payment {
    pub item: String,
    /// The payday that starts the period it was paid in.
    pub period: NaiveDate,
    pub date: NaiveDate,
    pub amount: Money,
}

/// An amount worked out elsewhere, such as the electricity bill the meter projects, that joins the plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    pub item: Item,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub name: String,
    pub kind: Kind,
    pub amount: Money,
    pub due: NaiveDate,
    pub confidence: Confidence,
    pub paid: bool,
}

/// From one payday up to (not including) the next.
#[derive(Clone, Debug, PartialEq)]
pub struct Period {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub income: Money,
    pub lines: Vec<Line>,
    /// Everything that falls due in it.
    pub due: Money,
    /// Income less everything due: what is yours to spend.
    pub left: Money,
    /// The period `today` is in.
    pub current: bool,
}

impl Period {
    pub fn days(&self) -> i64 {
        (self.end - self.start).num_days()
    }

    pub fn per_day(&self) -> Money {
        self.left.split(self.days())
    }

    pub fn is_short(&self) -> bool {
        self.left.is_negative()
    }
}

fn month_start(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default()
}

fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + month as i32 - 1 + delta;
    (total.div_euclid(12), total.rem_euclid(12) as u32 + 1)
}

/// `day` of the month, or the month's last day if it has fewer.
fn clamp_day(year: i32, month: u32, day: u32) -> NaiveDate {
    let (next_year, next_month) = shift_month(year, month, 1);
    let last = (month_start(next_year, next_month) - Duration::days(1)).day();
    NaiveDate::from_ymd_opt(year, month, day.clamp(1, last)).unwrap_or_default()
}

/// Every payday from `from` through `to`, oldest first.
pub fn paydays(pay_days: &[u32], from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut found = Vec::new();
    let (mut year, mut month) = (from.year(), from.month());
    while month_start(year, month) <= to {
        found.extend(pay_days.iter().map(|day| clamp_day(year, month, *day)).filter(|date| *date >= from && *date <= to));
        (year, month) = shift_month(year, month, 1);
    }
    found.sort();
    found.dedup();
    found
}

/// When an item falls due, from `first` (the earliest payday looked at) up to `last`.
fn dues(item: &Item, all_paydays: &[NaiveDate], last: NaiveDate) -> Vec<NaiveDate> {
    let Some(first) = all_paydays.first().copied() else { return Vec::new() };
    let mut dues: Vec<NaiveDate> = match item.every {
        Every::Payday => all_paydays.to_vec(),
        Every::Month => {
            let day = item.day.unwrap_or(31);
            let (mut year, mut month) = (first.year(), first.month());
            let mut found = Vec::new();
            while month_start(year, month) <= last {
                found.push(clamp_day(year, month, day));
                (year, month) = shift_month(year, month, 1);
            }
            found
        }
        Every::Once => item.date.into_iter().collect(),
    };
    if item.every != Every::Once
        && let Some(start) = item.date
    {
        dues.retain(|due| *due >= start);
        if let Some(times) = item.times {
            dues.truncate(times as usize);
        }
    }
    dues
}

fn paid_in(payments: &[Payment], name: &str, period: NaiveDate) -> Option<Money> {
    let matching: Vec<&Payment> = payments.iter().filter(|p| p.period == period && p.item.eq_ignore_ascii_case(name)).collect();
    (!matching.is_empty()).then(|| matching.iter().map(|p| p.amount).sum())
}

/// `count` periods starting with the one `today` is in.
pub fn periods(config: &Config, payments: &[Payment], estimates: &[Estimate], today: NaiveDate, count: usize) -> Vec<Period> {
    if config.pay_days.is_empty() || count == 0 {
        return Vec::new();
    }
    let (back_year, back_month) = shift_month(today.year(), today.month(), -14);
    let (ahead_year, ahead_month) = shift_month(today.year(), today.month(), (count / config.pay_days.len().max(1)) as i32 + 3);
    let scan_from = month_start(back_year, back_month);
    let scan_to = clamp_day(ahead_year, ahead_month, 31);
    let all = paydays(&config.pay_days, scan_from, scan_to);
    let Some(current) = all.iter().rposition(|payday| *payday <= today) else { return Vec::new() };
    let bounds = &all[current..];

    let mut periods: Vec<Period> = bounds
        .windows(2)
        .take(count)
        .map(|pair| Period { start: pair[0], end: pair[1], income: config.paycheck, lines: Vec::new(), due: Money::ZERO, left: Money::ZERO, current: pair[0] <= today && today < pair[1] })
        .collect();
    let Some(horizon_end) = periods.last().map(|period| period.end) else { return Vec::new() };

    let items = config.items.iter().map(|item| (item, Confidence::Firm)).chain(estimates.iter().map(|estimate| (&estimate.item, estimate.confidence)));
    for (item, confidence) in items {
        // A goal asks for no more than it still needs.
        let mut remaining = match (item.kind, item.target) {
            (Kind::Goal, Some(target)) => {
                let put_in: Money = payments.iter().filter(|p| p.item.eq_ignore_ascii_case(&item.name)).map(|p| p.amount).sum();
                Some((target - item.saved - put_in).max_zero())
            }
            _ => None,
        };
        for due in dues(item, &all, horizon_end) {
            let Some(period) = periods.iter_mut().find(|period| period.start <= due && due < period.end) else { continue };
            let (amount, paid) = match paid_in(payments, &item.name, period.start) {
                Some(done) => (done, true),
                None => match remaining.as_mut() {
                    Some(left) => {
                        let ask = item.amount.min(*left);
                        *left -= ask;
                        (ask, false)
                    }
                    None => (item.amount, false),
                },
            };
            if amount.centavos() <= 0 && !paid {
                continue;
            }
            period.lines.push(Line { name: item.name.clone(), kind: item.kind, amount, due, confidence, paid });
        }
    }

    for period in &mut periods {
        period.lines.sort_by(|a, b| a.due.cmp(&b.due).then_with(|| a.name.cmp(&b.name)));
        period.due = period.lines.iter().map(|line| line.amount).sum();
        period.left = period.income - period.due;
    }
    periods
}

/// A payday that is over: what it left to spend by the plan, and what you did spend.
#[derive(Clone, Debug, PartialEq)]
pub struct Past {
    pub period: Period,
    pub spent: Money,
    /// Whether any spending was logged in it. If not, `spent` says nothing.
    pub logged: bool,
}

impl Past {
    /// What was left of the paycheck's spending money at the end (negative if you spent more than it left).
    pub fn kept(&self) -> Money {
        self.period.left - self.spent
    }
}

/// The last `count` paydays that are over, newest first, counting only those since you first logged a payment or a
/// purchase (before that, nothing was kept, so there is nothing to compare). They are worked out from today's plan,
/// so an item you added or changed since shows in them as it is now.
pub fn history(config: &Config, payments: &[Payment], spends: &[Spend], today: NaiveDate, count: usize) -> Vec<Past> {
    let Some(now) = periods(config, payments, &[], today, 1).into_iter().next() else { return Vec::new() };
    let (year, month) = shift_month(today.year(), today.month(), -((count / config.pay_days.len().max(1)) as i32) - 2);
    let Some(first_use) = payments.iter().map(|p| p.date).chain(spends.iter().map(|s| s.date)).min() else { return Vec::new() };
    let mut starts = paydays(&config.pay_days, month_start(year, month), now.start - Duration::days(1));
    starts.reverse();
    starts
        .into_iter()
        .take(count)
        .filter_map(|start| {
            let mut period = periods(config, payments, &[], start, 1).into_iter().next()?;
            if period.end <= first_use {
                return None;
            }
            period.current = false;
            let within: Vec<&Spend> = spends.iter().filter(|s| s.date >= period.start && s.date < period.end).collect();
            Some(Past { spent: within.iter().map(|s| s.amount).sum(), logged: !within.is_empty(), period })
        })
        .collect()
}

/// How far along a goal, debt or installment is, and when it ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    pub name: String,
    pub kind: Kind,
    /// The whole of it: a goal's target, or what a debt or installment comes to.
    pub total: Money,
    pub paid: Money,
    pub remaining: Money,
    pub payments_left: usize,
    /// The last payment, when it ends.
    pub ends: Option<NaiveDate>,
}

/// Goals, debts and installments, and the other items that end by themselves (a one-time bill or a fixed term).
pub fn forecasts(config: &Config, payments: &[Payment], today: NaiveDate) -> Vec<Forecast> {
    let horizon = periods(config, payments, &[], today, 48);
    config
        .items
        .iter()
        .filter(|item| item.kind == Kind::Goal || item.every == Every::Once || item.times.is_some())
        .map(|item| {
            let ahead: Vec<&Line> = horizon.iter().flat_map(|period| period.lines.iter()).filter(|line| line.name.eq_ignore_ascii_case(&item.name) && !line.paid).collect();
            let put_in: Money = payments.iter().filter(|p| p.item.eq_ignore_ascii_case(&item.name)).map(|p| p.amount).sum();
            let coming: Money = ahead.iter().map(|line| line.amount).sum();
            let (total, paid) = match item.kind {
                Kind::Goal => (item.target.unwrap_or_default(), item.saved + put_in),
                _ => (put_in + coming, put_in),
            };
            Forecast { name: item.name.clone(), kind: item.kind, total, paid, remaining: (total - paid).max_zero(), payments_left: ahead.len(), ends: ahead.iter().map(|line| line.due).max() }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn pesos(n: i64) -> Money {
        Money::from_pesos(n)
    }

    fn item(name: &str, amount: i64) -> Item {
        Item { name: name.into(), amount: pesos(amount), ..Item::default() }
    }

    /// The plan this was built for, as of 1 Nov 2026: paid on the 15th and 30th, ₱12,000 each time.
    fn plan() -> Config {
        let mut config = Config { paycheck: Money::from_centavos(1_200_000), ..Config::default() };
        let items = [
            // The ₱4,000 rent starts at the end of November; this paycheck's rent was a different amount, paid already.
            Item { day: Some(31), date: Some(date(2026, 11, 30)), ..item("Rent", 4000) },
            Item { day: Some(15), ..item("Internet", 1599) },
            Item { day: Some(15), ..item("Parents", 2500) },
            Item { every: Every::Payday, ..item("Laundry", 170) },
            Item { kind: Kind::Goal, every: Every::Payday, target: Some(pesos(25_000)), saved: pesos(12_000), ..item("Fridge", 5000) },
            Item { kind: Kind::Debt, every: Every::Once, date: Some(date(2026, 11, 1)), ..item("Family loan", 2800) },
            Item { kind: Kind::Installment, day: Some(15), date: Some(date(2026, 11, 15)), times: Some(2), ..item("Shop plan old", 215) },
            Item { kind: Kind::Installment, day: Some(15), date: Some(date(2026, 12, 15)), times: Some(3), ..item("Shop plan new", 459) },
        ];
        for item in items {
            config.put(item).unwrap();
        }
        config
    }

    fn names(period: &Period) -> Vec<&str> {
        period.lines.iter().map(|line| line.name.as_str()).collect()
    }

    #[test]
    fn paydays_fall_on_the_set_days_and_short_months_use_their_last() {
        let found = paydays(&[15, 30], date(2027, 1, 1), date(2027, 3, 31));
        assert_eq!(found, [date(2027, 1, 15), date(2027, 1, 30), date(2027, 2, 15), date(2027, 2, 28), date(2027, 3, 15), date(2027, 3, 30)]);
        // Two days that are both past the end of February are one payday.
        assert_eq!(paydays(&[28, 31], date(2027, 2, 1), date(2027, 2, 28)), [date(2027, 2, 28)]);
        assert_eq!(paydays(&[15, 30], date(2027, 1, 16), date(2027, 1, 29)), Vec::<NaiveDate>::new());
    }

    #[test]
    fn periods_run_payday_to_payday_starting_with_the_one_today_is_in() {
        let all = periods(&plan(), &[], &[], date(2026, 11, 1), 4);
        let starts: Vec<NaiveDate> = all.iter().map(|p| p.start).collect();
        assert_eq!(starts, [date(2026, 10, 30), date(2026, 11, 15), date(2026, 11, 30), date(2026, 12, 15)]);
        assert!(all[0].current && !all[1].current);
        assert_eq!(all[0].days(), 16);
        assert_eq!(all[1].days(), 15);
        // On a payday itself, that payday's period is the current one.
        assert_eq!(periods(&plan(), &[], &[], date(2026, 11, 15), 1)[0].start, date(2026, 11, 15));
        assert!(periods(&Config { pay_days: vec![], ..plan() }, &[], &[], date(2026, 11, 1), 3).is_empty());
    }

    #[test]
    fn each_item_comes_out_of_the_payday_just_before_it_is_due() {
        let all = periods(&plan(), &[], &[], date(2026, 11, 1), 4);
        // 30 Oct to 14 Nov: nothing monthly falls due; the payday items and the debt due on the 1st do.
        assert_eq!(names(&all[0]), ["Fridge", "Laundry", "Family loan"]);
        // 15 to 29 Nov: internet, parents and the old shop plan, all due on the 15th.
        assert!(["Internet", "Parents", "Shop plan old"].iter().all(|n| names(&all[1]).contains(n)), "{:?}", names(&all[1]));
        // 30 Nov to 14 Dec: rent, due on the last day of November.
        assert!(names(&all[2]).contains(&"Rent"));
        assert!(!names(&all[2]).contains(&"Internet"));
    }

    #[test]
    fn an_item_with_a_start_date_does_not_reach_back_before_it() {
        // Without the start date, rent due on 31 Oct would come out of the 30 Oct paycheck.
        let mut config = plan();
        config.put(Item { date: None, ..config.items[config.find("Rent").unwrap()].clone() }).unwrap();
        assert!(names(&periods(&config, &[], &[], date(2026, 11, 1), 1)[0]).contains(&"Rent"));
        assert!(!names(&periods(&plan(), &[], &[], date(2026, 11, 1), 1)[0]).contains(&"Rent"));
    }

    #[test]
    fn what_is_left_is_income_less_everything_due() {
        let all = periods(&plan(), &[], &[], date(2026, 11, 1), 3);
        // 30 Oct: ₱12,000 less the fridge ₱5,000, laundry ₱170 and the family loan ₱2,800.
        assert_eq!(all[0].due, Money::from_centavos(797_000));
        assert_eq!(all[0].left, Money::from_centavos(403_000));
        // 15 Nov: less internet, parents, old shop plan, fridge and laundry.
        assert_eq!(all[1].due, pesos(1599 + 2500 + 215 + 5000 + 170));
        assert_eq!(all[1].left, Money::from_centavos(1_200_000 - 948_400));
        assert_eq!(all[1].per_day(), all[1].left.split(15));
        assert!(!all[1].is_short());
    }

    #[test]
    fn a_goal_stops_asking_once_it_is_full() {
        let all = periods(&plan(), &[], &[], date(2026, 11, 1), 6);
        let fridge: Vec<(NaiveDate, Money)> = all.iter().flat_map(|p| p.lines.iter()).filter(|l| l.name == "Fridge").map(|l| (l.due, l.amount)).collect();
        // ₱12,000 saved of ₱25,000: ₱5,000, ₱5,000, then only the ₱3,000 it still needs, then nothing.
        assert_eq!(fridge, [(date(2026, 10, 30), pesos(5000)), (date(2026, 11, 15), pesos(5000)), (date(2026, 11, 30), pesos(3000))]);
    }

    #[test]
    fn marking_a_goal_paid_counts_toward_it_and_the_rest_adjusts() {
        // Put in only ₱4,000 this payday.
        let paid = [Payment { item: "Fridge".into(), period: date(2026, 10, 30), date: date(2026, 11, 1), amount: pesos(4000) }];
        let all = periods(&plan(), &paid, &[], date(2026, 11, 1), 6);
        let fridge: Vec<(Money, bool)> = all.iter().flat_map(|p| p.lines.iter()).filter(|l| l.name == "Fridge").map(|l| (l.amount, l.paid)).collect();
        // 12,000 + 4,000 = 16,000; 9,000 to go: 5,000 then 4,000.
        assert_eq!(fridge, [(pesos(4000), true), (pesos(5000), false), (pesos(4000), false)]);
        // The paid line still counts against the paycheck.
        assert_eq!(all[0].due, pesos(4000 + 170 + 2800));
    }

    #[test]
    fn an_installment_ends_after_its_term() {
        let all = periods(&plan(), &[], &[], date(2026, 11, 1), 12);
        let old: Vec<NaiveDate> = all.iter().flat_map(|p| p.lines.iter()).filter(|l| l.name == "Shop plan old").map(|l| l.due).collect();
        assert_eq!(old, [date(2026, 11, 15), date(2026, 12, 15)]);
        let new: Vec<NaiveDate> = all.iter().flat_map(|p| p.lines.iter()).filter(|l| l.name == "Shop plan new").map(|l| l.due).collect();
        assert_eq!(new, [date(2026, 12, 15), date(2027, 1, 15), date(2027, 2, 15)]);
    }

    #[test]
    fn a_one_time_item_happens_once_and_a_past_one_does_not_come_back() {
        let all = periods(&plan(), &[], &[], date(2026, 11, 20), 6);
        assert!(!all.iter().flat_map(|p| p.lines.iter()).any(|l| l.name == "Family loan"), "paid on 1 Nov, before this period");
        let all = periods(&plan(), &[], &[], date(2026, 11, 1), 6);
        assert_eq!(all.iter().flat_map(|p| p.lines.iter()).filter(|l| l.name == "Family loan").count(), 1);
    }

    #[test]
    fn an_estimate_from_a_tracker_joins_the_plan_with_its_confidence() {
        let electricity = Estimate { item: Item { day: Some(30), ..item("Electricity", 1350) }, confidence: Confidence::Estimate };
        let all = periods(&plan(), &[], &[electricity], date(2026, 11, 1), 4);
        let line = all[2].lines.iter().find(|l| l.name == "Electricity").expect("electricity on 30 Nov");
        assert_eq!((line.amount, line.confidence, line.due), (pesos(1350), Confidence::Estimate, date(2026, 11, 30)));
        assert_eq!(all[2].lines.iter().find(|l| l.name == "Rent").unwrap().confidence, Confidence::Firm);
    }

    #[test]
    fn a_paycheck_that_does_not_cover_what_is_due_shows_short() {
        let mut config = plan();
        config.paycheck = pesos(5000);
        let all = periods(&config, &[], &[], date(2026, 11, 1), 2);
        assert!(all[0].is_short() && all[0].left == pesos(5000) - all[0].due);
    }

    #[test]
    fn marking_a_bill_paid_keeps_the_amount_actually_paid() {
        let paid = [Payment { item: "rent".into(), period: date(2026, 11, 30), date: date(2026, 11, 30), amount: pesos(4200) }];
        let all = periods(&plan(), &paid, &[], date(2026, 11, 1), 3);
        let rent = all[2].lines.iter().find(|l| l.name == "Rent").unwrap();
        assert!(rent.paid && rent.amount == pesos(4200));
    }

    #[test]
    fn forecasts_say_how_far_along_and_when_it_ends() {
        let all = forecasts(&plan(), &[], date(2026, 11, 1));
        let get = |name: &str| all.iter().find(|f| f.name == name).unwrap();
        let fridge = get("Fridge");
        assert_eq!((fridge.total, fridge.paid, fridge.remaining, fridge.payments_left), (pesos(25_000), pesos(12_000), pesos(13_000), 3));
        assert_eq!(fridge.ends, Some(date(2026, 11, 30)));
        let new = get("Shop plan new");
        assert_eq!((new.total, new.paid, new.payments_left, new.ends), (pesos(459 * 3), Money::ZERO, 3, Some(date(2027, 2, 15))));
        assert_eq!(get("Family loan").remaining, pesos(2800));
        assert!(!all.iter().any(|f| f.name == "Rent"), "a bill with no end is not a forecast");
    }

    #[test]
    fn what_has_been_paid_comes_off_what_remains() {
        let paid = [Payment { item: "Shop plan old".into(), period: date(2026, 11, 15), date: date(2026, 11, 15), amount: pesos(215) }];
        let all = forecasts(&plan(), &paid, date(2026, 11, 16));
        let old = all.iter().find(|f| f.name == "Shop plan old").unwrap();
        assert_eq!((old.paid, old.remaining, old.payments_left), (pesos(215), pesos(215), 1));
    }

    #[test]
    fn history_lists_finished_paydays_newest_first_with_what_was_kept() {
        let spends = [
            Spend { date: date(2026, 11, 2), amount: pesos(100), category: String::new(), note: String::new() },
            Spend { date: date(2026, 11, 20), amount: pesos(1500), category: String::new(), note: String::new() },
            Spend { date: date(2026, 11, 29), amount: pesos(500), category: String::new(), note: String::new() },
        ];
        let past = history(&plan(), &[], &spends, date(2026, 12, 20), 3);
        // Today is in the period of 15 Dec; the three before it are 30 Nov, 15 Nov and 30 Oct.
        assert_eq!(past.iter().map(|p| p.period.start).collect::<Vec<_>>(), [date(2026, 11, 30), date(2026, 11, 15), date(2026, 10, 30)]);
        assert!(past.iter().all(|p| !p.period.current));
        let nov15 = &past[1];
        assert_eq!(nov15.spent, pesos(2000));
        assert!(nov15.logged);
        assert_eq!(nov15.kept(), nov15.period.left - pesos(2000));
        assert_eq!(past[2].spent, pesos(100));
        assert!(!past[0].logged, "nothing was logged from 30 Nov");
        assert_eq!(past[0].spent, Money::ZERO);
    }

    #[test]
    fn history_is_empty_without_paydays_and_never_includes_the_current_one() {
        assert!(history(&Config { pay_days: vec![], ..plan() }, &[], &[], date(2026, 11, 1), 3).is_empty());
        let spends = [Spend { date: date(2026, 11, 2), amount: pesos(100), category: String::new(), note: String::new() }];
        let past = history(&plan(), &[], &spends, date(2026, 11, 15), 4);
        assert!(past.iter().all(|p| p.period.start < date(2026, 11, 15)), "{past:?}");
        assert_eq!(past.len(), 1, "paydays that ended before the first purchase are not counted");
        assert_eq!(past[0].period.start, date(2026, 10, 30));
        assert!(history(&plan(), &[], &[], date(2026, 11, 15), 4).is_empty(), "nothing logged, nothing to compare");
    }
}
