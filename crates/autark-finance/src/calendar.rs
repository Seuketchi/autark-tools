//! The plan laid out on a month's calendar: which days are paydays, what falls due on each, what you spent.

use chrono::{Datelike, Duration, NaiveDate};

use crate::money::Money;
use crate::payday::{self, Estimate, Line, Payment};
use crate::plan::Config;
use crate::spending::Spend;

#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: NaiveDate,
    /// False for the days of the neighbouring months that fill out the first and last week.
    pub in_month: bool,
    pub today: bool,
    pub payday: bool,
    /// What falls due on this day.
    pub lines: Vec<Line>,
    pub spent: Money,
    pub purchases: usize,
}

impl Day {
    pub fn due(&self) -> Money {
        self.lines.iter().map(|line| line.amount).sum()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Month {
    pub year: i32,
    pub month: u32,
    /// Weeks from Monday to Sunday, each of seven days.
    pub weeks: Vec<Vec<Day>>,
    /// Everything that falls due in the month itself, and what you spent in it.
    pub due: Money,
    pub spent: Money,
    pub paydays: usize,
}

impl Month {
    pub fn days(&self) -> impl Iterator<Item = &Day> {
        self.weeks.iter().flatten()
    }
}

fn first_of(year: i32, month: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, 1)
}

/// The month `delta` months from `year`-`month`.
pub fn shift(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + month as i32 - 1 + delta;
    (total.div_euclid(12), total.rem_euclid(12) as u32 + 1)
}

/// The calendar of one month, or `None` for a month that does not exist.
///
/// Days before you first logged a payment or a purchase show no bills, because nothing says they were paid; days from
/// today on always do.
pub fn month(config: &Config, payments: &[Payment], estimates: &[Estimate], spends: &[Spend], year: i32, month: u32, today: NaiveDate) -> Option<Month> {
    let first = first_of(year, month)?;
    let (next_year, next_month) = shift(year, month, 1);
    let last = first_of(next_year, next_month)? - Duration::days(1);
    let start = first - Duration::days(i64::from(first.weekday().num_days_from_monday()));
    let end = last + Duration::days(6 - i64::from(last.weekday().num_days_from_monday()));

    let payday_set: Vec<NaiveDate> = payday::paydays(&config.pay_days, start, end);
    let first_use = payments.iter().map(|p| p.date).chain(spends.iter().map(|s| s.date)).min();
    let known_from = first_use.map_or(today, |used| used.min(today));
    let lines: Vec<Line> = lines_between(config, payments, estimates, start, end, today).into_iter().filter(|line| line.due >= known_from).collect();

    let mut weeks = Vec::new();
    let mut date = start;
    while date <= end {
        let week = (0..7)
            .map(|offset| {
                let date = date + Duration::days(offset);
                let mine: Vec<&Spend> = spends.iter().filter(|s| s.date == date).collect();
                Day {
                    date,
                    in_month: date.month() == month && date.year() == year,
                    today: date == today,
                    payday: payday_set.contains(&date),
                    lines: lines.iter().filter(|line| line.due == date).cloned().collect(),
                    spent: mine.iter().map(|s| s.amount).sum(),
                    purchases: mine.len(),
                }
            })
            .collect();
        weeks.push(week);
        date += Duration::days(7);
    }

    let in_month = || weeks.iter().flatten().filter(|day: &&Day| day.in_month);
    Some(Month { year, month, due: in_month().map(Day::due).sum(), spent: in_month().map(|d| d.spent).sum(), paydays: in_month().filter(|d| d.payday).count(), weeks: weeks.clone() })
}

/// Every line that falls due from `start` to `end`. From today's payday on they come from the same periods Paychecks
/// shows, so a goal that is nearly full asks for the same amounts here; paydays already over are worked out one by one.
fn lines_between(config: &Config, payments: &[Payment], estimates: &[Estimate], start: NaiveDate, end: NaiveDate, today: NaiveDate) -> Vec<Line> {
    let Some(current) = payday::periods(config, payments, estimates, today, 1).into_iter().next() else { return Vec::new() };
    let mut lines = Vec::new();
    for past in payday::paydays(&config.pay_days, start - Duration::days(35), current.start - Duration::days(1)) {
        if let Some(period) = payday::periods(config, payments, estimates, past, 1).into_iter().next().filter(|period| period.end > start) {
            lines.extend(period.lines);
        }
    }
    let ahead = payday::paydays(&config.pay_days, today, end).len() + 1;
    lines.extend(payday::periods(config, payments, estimates, today, ahead.clamp(1, 60)).into_iter().flat_map(|period| period.lines));
    lines.retain(|line| line.due >= start && line.due <= end);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Every, Item, Kind};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn pesos(n: i64) -> Money {
        Money::from_pesos(n)
    }

    fn config() -> Config {
        let mut config = Config { paycheck: pesos(12_000), ..Config::default() };
        for item in [
            Item { name: "Rent".into(), amount: pesos(4000), every: Every::Month, day: Some(31), ..Item::default() },
            Item { name: "Internet".into(), amount: pesos(1600), every: Every::Month, day: Some(15), ..Item::default() },
            Item { name: "Laundry".into(), amount: pesos(170), every: Every::Payday, ..Item::default() },
            Item { name: "Fund".into(), kind: Kind::Goal, amount: pesos(500), every: Every::Payday, target: Some(pesos(1000)), ..Item::default() },
        ] {
            config.put(item).unwrap();
        }
        config
    }

    fn spend(day: NaiveDate, pesos_: i64) -> Spend {
        Spend { date: day, amount: pesos(pesos_), category: String::new(), note: String::new() }
    }

    #[test]
    fn a_month_is_whole_weeks_from_monday_to_sunday() {
        // November 2026 starts on a Sunday and ends on a Monday.
        let m = month(&config(), &[], &[], &[], 2026, 11, date(2026, 11, 10)).unwrap();
        assert!(m.weeks.iter().all(|week| week.len() == 7));
        assert_eq!(m.weeks.first().unwrap()[0].date, date(2026, 10, 26));
        assert_eq!(m.weeks.last().unwrap()[6].date, date(2026, 12, 6));
        assert_eq!(m.weeks.len(), 6);
        assert_eq!(m.days().filter(|d| d.in_month).count(), 30);
        assert!(m.days().find(|d| d.date == date(2026, 11, 10)).unwrap().today);
        assert!(!m.weeks[0][0].in_month);
        assert!(month(&config(), &[], &[], &[], 2026, 13, date(2026, 11, 10)).is_none());
    }

    #[test]
    fn paydays_and_what_falls_due_land_on_their_days() {
        let m = month(&config(), &[], &[], &[], 2026, 11, date(2026, 11, 10)).unwrap();
        let day = |d: u32| m.days().find(|day| day.date == date(2026, 11, d)).unwrap();
        assert!(day(15).payday && day(30).payday && !day(16).payday);
        assert_eq!(m.paydays, 2);
        let names = |d: u32| day(d).lines.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(15), ["Fund", "Internet", "Laundry"]);
        assert_eq!(names(30), ["Laundry", "Rent"], "the goal asks for nothing past its target");
        assert_eq!(day(15).due(), pesos(500 + 1600 + 170));
        // Everything due in November: the 15th, the 30th and the first payday in October's tail is outside.
        assert_eq!(m.due, pesos(500 + 1600 + 170 + 170 + 4000));
    }

    #[test]
    fn spending_shows_on_its_day_and_sums_for_the_month() {
        let spends = [spend(date(2026, 11, 3), 100), spend(date(2026, 11, 3), 50), spend(date(2026, 10, 30), 999), spend(date(2026, 11, 9), 70)];
        let m = month(&config(), &[], &[], &spends, 2026, 11, date(2026, 11, 10)).unwrap();
        let third = m.days().find(|d| d.date == date(2026, 11, 3)).unwrap();
        assert_eq!((third.spent, third.purchases), (pesos(150), 2));
        assert_eq!(m.spent, pesos(220), "October's purchase on the grid's first row is not November's");
    }

    #[test]
    fn days_before_you_started_logging_show_no_bills() {
        let today = date(2026, 11, 20);
        // Nothing logged ever: only days from today on show bills.
        let none = month(&config(), &[], &[], &[], 2026, 11, today).unwrap();
        assert!(none.days().filter(|d| d.date < today).all(|d| d.lines.is_empty()));
        assert!(!none.days().find(|d| d.date == date(2026, 11, 30)).unwrap().lines.is_empty());
        // Logging began on the 14th: the 15th shows its bills, the days before do not.
        let began = month(&config(), &[], &[], &[spend(date(2026, 11, 14), 10)], 2026, 11, today).unwrap();
        assert!(!began.days().find(|d| d.date == date(2026, 11, 15)).unwrap().lines.is_empty());
        assert!(began.days().filter(|d| d.date < date(2026, 11, 14)).all(|d| d.lines.is_empty()));
    }

    #[test]
    fn a_paid_bill_is_marked_paid_on_its_day() {
        let paid = [Payment { item: "Internet".into(), period: date(2026, 11, 15), date: date(2026, 11, 15), amount: pesos(1650) }];
        let m = month(&config(), &paid, &[], &[spend(date(2026, 11, 1), 1)], 2026, 11, date(2026, 11, 20)).unwrap();
        let internet = m.days().find(|d| d.date == date(2026, 11, 15)).unwrap().lines.iter().find(|l| l.name == "Internet").unwrap().clone();
        assert!(internet.paid);
        assert_eq!(internet.amount, pesos(1650));
    }

    #[test]
    fn months_shift_across_years() {
        assert_eq!(shift(2026, 12, 1), (2027, 1));
        assert_eq!(shift(2026, 1, -1), (2025, 12));
        assert_eq!(shift(2026, 6, 0), (2026, 6));
    }
}
