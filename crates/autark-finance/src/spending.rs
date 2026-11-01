//! What you actually spent, set against what a payday leaves. The payday plan says "₱5,447 to spend over 16 days";
//! this says how much of that is gone, what is left for today, and what each day after gets if you stop now.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::payday::Period;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spend {
    pub date: NaiveDate,
    pub amount: Money,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// Where a period stands as of `today`.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// What the period leaves to spend, before any spending.
    pub to_spend: Money,
    pub spent: Money,
    /// What is still there: `to_spend` less `spent` (negative once you are over).
    pub left: Money,
    /// Days from today to the end of the period, today included.
    pub days_left: i64,
    pub spent_today: Money,
    /// Today's share if everything not yet spent were spread evenly over the days left, counting only what was
    /// spent before today.
    pub today_budget: Money,
    /// What is left of it after today's spending (negative once today is over).
    pub today_left: Money,
    /// Each day after today, if you spend nothing more today.
    pub per_day_after: Money,
    /// Spending by category, biggest first. Entries with no category are "Other".
    pub by_category: Vec<(String, Money)>,
}

pub fn summarize(period: &Period, spends: &[Spend], today: NaiveDate) -> Summary {
    let within: Vec<&Spend> = spends.iter().filter(|s| s.date >= period.start && s.date < period.end).collect();
    let spent: Money = within.iter().map(|s| s.amount).sum();
    let spent_today: Money = within.iter().filter(|s| s.date == today).map(|s| s.amount).sum();
    let before_today: Money = within.iter().filter(|s| s.date < today).map(|s| s.amount).sum();
    let days_left = if today < period.start { period.days() } else { (period.end - today).num_days().max(1) };

    let left = period.left - spent;
    let today_budget = (period.left - before_today).split(days_left);

    let mut totals: Vec<(String, Money)> = Vec::new();
    for spend in &within {
        let name = if spend.category.trim().is_empty() { "Other" } else { spend.category.trim() };
        match totals.iter_mut().find(|(known, _)| known.eq_ignore_ascii_case(name)) {
            Some((_, total)) => *total += spend.amount,
            None => totals.push((name.to_string(), spend.amount)),
        }
    }
    totals.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    Summary {
        to_spend: period.left,
        spent,
        left,
        days_left,
        spent_today,
        today_budget,
        today_left: today_budget - spent_today,
        per_day_after: if days_left > 1 { left.split(days_left - 1) } else { left },
        by_category: totals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payday::Period;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    /// 15 Nov to 29 Nov, ₱3,000 to spend over its 15 days: ₱200 a day.
    fn period() -> Period {
        Period { start: date(15), end: date(30), income: Money::from_pesos(10_000), lines: vec![], due: Money::from_pesos(7000), left: Money::from_pesos(3000), current: true }
    }

    fn spend(day: u32, pesos: i64, category: &str) -> Spend {
        Spend { date: date(day), amount: Money::from_pesos(pesos), category: category.into(), note: String::new() }
    }

    #[test]
    fn the_first_day_gets_an_even_share() {
        let summary = summarize(&period(), &[], date(15));
        assert_eq!((summary.days_left, summary.today_budget, summary.today_left), (15, Money::from_pesos(200), Money::from_pesos(200)));
        assert_eq!(summary.left, Money::from_pesos(3000));
    }

    #[test]
    fn spending_today_comes_off_today_and_what_remains_is_spread_over_the_days_after() {
        let summary = summarize(&period(), &[spend(15, 120, "Food")], date(15));
        assert_eq!(summary.spent_today, Money::from_pesos(120));
        assert_eq!(summary.today_left, Money::from_pesos(80));
        // ₱2,880 left over the 14 days after today.
        assert_eq!(summary.per_day_after, Money::from_pesos(2880).split(14));
    }

    #[test]
    fn overspending_yesterday_lowers_todays_share_and_underspending_raises_it() {
        let over = summarize(&period(), &[spend(15, 500, "Food")], date(16));
        // ₱2,500 over the 14 days from the 16th.
        assert_eq!(over.today_budget, Money::from_pesos(2500).split(14));
        assert!(over.today_budget < Money::from_pesos(200));
        let under = summarize(&period(), &[spend(15, 100, "Food")], date(16));
        assert!(under.today_budget > Money::from_pesos(200));
    }

    #[test]
    fn going_over_the_whole_period_shows_negative() {
        let summary = summarize(&period(), &[spend(15, 2000, "Food"), spend(16, 1500, "Transport")], date(16));
        assert_eq!(summary.left, Money::from_pesos(-500));
        assert!(summary.today_left.is_negative());
    }

    #[test]
    fn the_last_day_gets_everything_that_is_left() {
        let summary = summarize(&period(), &[spend(20, 1000, "Food")], date(29));
        assert_eq!(summary.days_left, 1);
        assert_eq!(summary.today_budget, Money::from_pesos(2000));
        assert_eq!(summary.per_day_after, Money::from_pesos(2000));
    }

    #[test]
    fn spending_outside_the_period_is_not_counted_and_a_future_period_is_untouched() {
        let all = [spend(14, 999, "Food"), spend(30, 999, "Food"), spend(20, 100, "Food")];
        assert_eq!(summarize(&period(), &all, date(20)).spent, Money::from_pesos(100));
        let ahead = summarize(&period(), &[], date(10));
        assert_eq!((ahead.days_left, ahead.today_budget), (15, Money::from_pesos(200)));
    }

    #[test]
    fn categories_add_up_biggest_first_and_blank_means_other() {
        let all = [spend(15, 100, "Food"), spend(16, 300, "transport"), spend(17, 50, "FOOD"), spend(18, 20, ""), spend(18, 10, "  ")];
        let summary = summarize(&period(), &all, date(18));
        assert_eq!(summary.by_category, [("transport".to_string(), Money::from_pesos(300)), ("Food".to_string(), Money::from_pesos(150)), ("Other".to_string(), Money::from_pesos(30))]);
    }
}
