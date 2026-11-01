//! Tips for fitting your schedule to your budget. Each one speaks up only when a number crosses a threshold, so a
//! quiet month gets a quiet screen, and every threshold can be changed under `[electric.thresholds]` in
//! `budget.toml`. Nothing here guesses: a tip quotes the numbers that triggered it.

use chrono::Weekday;
use serde::{Deserialize, Serialize};

use crate::electric::{Basis, Plan, RateSource};
use crate::trends::Trends;

/// How much a tip matters, most urgent first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Alert,
    Warn,
    Info,
    Good,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tip {
    pub level: Level,
    pub title: String,
    pub detail: String,
}

/// The lines a tip is drawn at. Each is a fraction (0.15 is 15%) unless it says otherwise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// Projected bill at this share of the budget or more: little room left.
    pub near_budget: f64,
    /// Projected bill at this share of the budget or less: plenty of room.
    pub comfortable: f64,
    /// The latest 7 days up by this much on the 7 before.
    pub week_rise: f64,
    /// ...or down by this much.
    pub week_drop: f64,
    /// The busiest weekday uses this much more than the quietest.
    pub weekday_gap: f64,
    /// This share of the last 14 days went over the daily budget.
    pub days_over_share: f64,
    /// The latest day used this many times the days before it (a multiple, not a fraction)...
    pub spike_factor: f64,
    /// ...and at least this many kWh more.
    pub spike_min_kwh: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { near_budget: 0.90, comfortable: 0.70, week_rise: 0.15, week_drop: 0.15, weekday_gap: 0.40, days_over_share: 0.40, spike_factor: 2.0, spike_min_kwh: 1.0 }
    }
}

fn tip(level: Level, title: impl Into<String>, detail: impl Into<String>) -> Tip {
    Tip { level, title: title.into(), detail: detail.into() }
}

fn kwh(amount: f64) -> String {
    format!("{amount:.1} kWh")
}

/// What an amount of kWh a day is in hours of something plugged in: 1 kWh is an hour of a 1,000 W appliance.
fn hours(kwh_per_day: f64) -> String {
    format!("{:.1} hours less of a 1,000 W appliance a day (or {:.0} hours of a 100 W fan)", kwh_per_day, kwh_per_day * 10.0)
}

fn is_weekend(day: Weekday) -> bool {
    matches!(day, Weekday::Sat | Weekday::Sun)
}

/// The tips for now, most urgent first.
pub fn recommend(plan: &Plan, trends: &Trends, at: &Thresholds) -> Vec<Tip> {
    let mut tips = Vec::new();

    match plan.basis {
        Basis::None => return vec![tip(Level::Info, "Nothing to go on yet", "Log what the meter says today and again in a day or two. Tips start once there is a pace to read.")],
        Basis::Early | Basis::Appliances => tips.push(tip(
            Level::Info,
            "Early estimate",
            "There are fewer than 5 days of readings, so the projection could still move a lot. Tips firm up as readings come in.",
        )),
        Basis::Readings => {}
    }

    // Against the budget.
    if plan.budget.centavos() > 0 && plan.basis != Basis::None {
        let share = plan.projected_cost.pesos_f64() / plan.budget.pesos_f64();
        if share >= 1.0 {
            let cut = (plan.daily_kwh - plan.allowance_kwh_per_day).max(0.0);
            tips.push(tip(
                Level::Alert,
                format!("Heading {} over budget", plan.margin.abs()),
                format!(
                    "At {} a day you finish at {} against {}. To end on budget, average {} a day from here: {}.",
                    kwh(plan.daily_kwh),
                    plan.projected_cost,
                    plan.budget,
                    kwh(plan.allowance_kwh_per_day),
                    hours(cut)
                ),
            ));
        } else if share >= at.near_budget {
            tips.push(tip(
                Level::Warn,
                "Close to the budget",
                format!("You are on track for {}, {:.0}% of {}. {} to spare, so one heavy week would tip it over.", plan.projected_cost, share * 100.0, plan.budget, plan.margin),
            ));
        } else if share <= at.comfortable {
            tips.push(tip(Level::Good, "Plenty of room", format!("On track for {}, with {} of the {} budget to spare.", plan.projected_cost, plan.margin, plan.budget)));
        } else {
            tips.push(tip(Level::Good, "On track", format!("{} projected, {} under the {} budget.", plan.projected_cost, plan.margin, plan.budget)));
        }
    }

    // Against last week.
    if let (Some(change), Some(now), Some(before)) = (trends.week_change, trends.this_week, trends.last_week) {
        if change >= at.week_rise {
            tips.push(tip(
                Level::Warn,
                format!("Up {:.0}% on last week", change * 100.0),
                format!("{} a day, from {}. If nothing changed on purpose, check for something left running or a new appliance.", kwh(now), kwh(before)),
            ));
        } else if change <= -at.week_drop {
            tips.push(tip(Level::Good, format!("Down {:.0}% on last week", -change * 100.0), format!("{} a day, from {}. Whatever you changed is working.", kwh(now), kwh(before))));
        }
    }

    // Too many days over the daily budget.
    let shown = trends.days.len();
    if shown >= 7 && trends.days_over as f64 / shown as f64 >= at.days_over_share {
        tips.push(tip(
            Level::Warn,
            format!("{} of the last {shown} days went over", trends.days_over),
            format!("Your budget works out to {} a day. Look at the red bars in Trends for which days.", kwh(trends.budget_kwh_per_day)),
        ));
    }

    // A day that stands out.
    if let Some((latest, before)) = trends.days.split_last()
        && latest.measured
        && before.len() >= 3
    {
        let recent = &before[before.len().saturating_sub(7)..];
        let usual = recent.iter().map(|d| d.kwh).sum::<f64>() / recent.len() as f64;
        if usual > 0.0 && latest.kwh >= usual * at.spike_factor && latest.kwh - usual >= at.spike_min_kwh {
            tips.push(tip(
                Level::Warn,
                format!("{} stood out", latest.date.format("%-d %b")),
                format!("{} against a usual {}. An appliance left on, or something you do not do every day?", kwh(latest.kwh), kwh(usual)),
            ));
        }
    }

    // A weekday pattern worth shifting.
    if let (Some((high, most)), Some((low, least))) = (trends.busiest, trends.quietest)
        && least > 0.0
        && most >= least * (1.0 + at.weekday_gap)
    {
        let shift = (most - least) * 0.5;
        let which = if is_weekend(high) { "Weekends are your heavy days. " } else { "" };
        tips.push(tip(
            Level::Info,
            format!("{high} uses {:.0}% more than {low}", (most / least - 1.0) * 100.0),
            format!(
                "{which}{} on {high} against {} on {low}. Moving half the gap to a quiet day would save about {} a week, about {} a cycle.",
                kwh(most),
                kwh(least),
                kwh(shift),
                crate::money::Money::from_pesos_f64(shift * plan.rate * plan.cycle.days() as f64 / 7.0)
            ),
        ));
    }

    if plan.rate_source == RateSource::Fallback {
        tips.push(tip(Level::Info, "Add a bill to firm up the cost", format!("The cost uses a placeholder ₱{:.2}/kWh. Record your last bill and it will use your real rate.", plan.rate)));
    }

    tips.sort_by_key(|t| t.level);
    tips
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, Duration, NaiveDate};

    use super::*;
    use crate::electric::{Bill, Config, Reading, plan};
    use crate::money::Money;
    use crate::trends::trends;

    fn day(n: u32) -> NaiveDate {
        // 1 Nov 2026 is a Sunday.
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap() + Duration::days(i64::from(n) - 1)
    }

    /// Tips for a month where day `n` used `uses[n-2]`, as of the last of them.
    fn tips_for(uses: &[f64], budget: i64, with_bill: bool) -> Vec<Tip> {
        let mut meter = 15.4;
        let mut readings = vec![Reading { date: day(1), kwh: meter, note: String::new() }];
        for (index, used) in uses.iter().enumerate() {
            meter += used;
            readings.push(Reading { date: day(2 + index as u32), kwh: meter, note: String::new() });
        }
        let today = day(1 + uses.len() as u32);
        let config = Config { budget: Money::from_pesos(budget), ..Config::default() };
        let bills = if with_bill { vec![Bill { from: day(1) - Duration::days(30), to: day(1), kwh: 100.0, amount: Money::from_pesos(1200) }] } else { vec![] };
        let p = plan(&config, &readings, &bills, today);
        recommend(&p, &trends(&readings, &p), &config.thresholds)
    }

    fn titles(tips: &[Tip]) -> Vec<&str> {
        tips.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn nothing_to_go_on_says_how_to_start() {
        let tips = tips_for(&[], 1200, true);
        assert_eq!(tips.len(), 1);
        assert_eq!(tips[0].title, "Nothing to go on yet");
    }

    #[test]
    fn over_budget_is_an_alert_with_what_to_cut_in_hours() {
        // 4 kWh a day at ₱12 for 30 days is ₱1,440 against ₱1,200.
        let tips = tips_for(&[4.0; 10], 1200, true);
        assert_eq!(tips[0].level, Level::Alert);
        assert!(tips[0].title.contains("₱240 over"), "{}", tips[0].title);
        assert!(tips[0].detail.contains("hours less of a 1,000 W appliance"), "{}", tips[0].detail);
    }

    #[test]
    fn the_budget_lines_are_thresholds_you_can_see() {
        // ₱1,200 buys 100 kWh. At 3.2 kWh a day: 96 kWh, 96% of it: close.
        assert!(titles(&tips_for(&[3.2; 10], 1200, true)).contains(&"Close to the budget"));
        // 3.0 a day: 90 kWh exactly 90%: still close (at the line counts).
        assert!(titles(&tips_for(&[3.0; 10], 1200, true)).contains(&"Close to the budget"));
        // 2.5 a day: 75%: on track.
        assert!(titles(&tips_for(&[2.5; 10], 1200, true)).contains(&"On track"));
        // 2.0 a day: 60%: plenty of room.
        assert!(titles(&tips_for(&[2.0; 10], 1200, true)).contains(&"Plenty of room"));
    }

    #[test]
    fn a_lower_threshold_makes_a_tip_speak_sooner() {
        let uses = [2.5; 10];
        let mut meter = 15.4;
        let mut readings = vec![Reading { date: day(1), kwh: meter, note: String::new() }];
        for (i, used) in uses.iter().enumerate() {
            meter += used;
            readings.push(Reading { date: day(2 + i as u32), kwh: meter, note: String::new() });
        }
        let p = plan(&Config::default(), &readings, &[], day(11));
        let t = trends(&readings, &p);
        assert!(!titles(&recommend(&p, &t, &Thresholds::default())).contains(&"Close to the budget"));
        assert!(titles(&recommend(&p, &t, &Thresholds { near_budget: 0.7, ..Thresholds::default() })).contains(&"Close to the budget"));
    }

    #[test]
    fn a_rising_week_is_flagged_and_a_falling_one_is_praised() {
        let mut rising = vec![2.0; 14];
        rising.extend([3.0; 7]);
        assert!(tips_for(&rising, 5000, true).iter().any(|t| t.title.starts_with("Up 50%")));
        let mut falling = vec![3.0; 14];
        falling.extend([2.0; 7]);
        assert!(tips_for(&falling, 5000, true).iter().any(|t| t.title.starts_with("Down 33%")));
        // Under 15% either way says nothing.
        let mut steady = vec![2.0; 14];
        steady.extend([2.2; 7]);
        let tips = tips_for(&steady, 5000, true);
        assert!(!tips.iter().any(|t| t.title.starts_with("Up") || t.title.starts_with("Down")), "{:?}", titles(&tips));
    }

    #[test]
    fn a_single_heavy_day_is_called_out() {
        let mut uses = vec![2.0; 8];
        uses.push(6.0);
        let tips = tips_for(&uses, 5000, true);
        assert!(tips.iter().any(|t| t.title.ends_with("stood out") && t.detail.contains("6.0 kWh against a usual 2.0 kWh")), "{:?}", titles(&tips));
        // A small bump is not a spike: 2.0 to 3.0 is under the 2x line.
        let mut small = vec![2.0; 8];
        small.push(3.0);
        assert!(!titles(&tips_for(&small, 5000, true)).iter().any(|t| t.ends_with("stood out")));
    }

    #[test]
    fn heavy_weekends_get_a_schedule_tip() {
        let uses: Vec<f64> = (2..=29).map(|n| if matches!(day(n).weekday(), Weekday::Sat | Weekday::Sun) { 5.0 } else { 2.0 }).collect();
        let tips = tips_for(&uses, 5000, true);
        let weekday = tips.iter().find(|t| t.title.contains("more than")).expect("a weekday tip");
        assert!(weekday.detail.starts_with("Weekends are your heavy days."), "{}", weekday.detail);
        assert_eq!(weekday.level, Level::Info);
    }

    #[test]
    fn the_placeholder_rate_gets_a_nudge_and_a_bill_removes_it() {
        assert!(titles(&tips_for(&[2.0; 10], 5000, false)).contains(&"Add a bill to firm up the cost"));
        assert!(!titles(&tips_for(&[2.0; 10], 5000, true)).contains(&"Add a bill to firm up the cost"));
    }

    #[test]
    fn the_most_urgent_comes_first() {
        let tips = tips_for(&[4.0; 10], 1200, false);
        let levels: Vec<Level> = tips.iter().map(|t| t.level).collect();
        let mut sorted = levels.clone();
        sorted.sort();
        assert_eq!(levels, sorted);
        assert_eq!(levels[0], Level::Alert);
    }
}
