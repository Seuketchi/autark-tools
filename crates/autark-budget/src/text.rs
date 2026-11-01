//! How the planner's numbers read, shared by the command line and the window so they never disagree.

use autark_finance::electric::{Basis, Plan, RateSource};
use autark_finance::afford::Verdict;
use autark_finance::calendar::Month;
use autark_finance::obligation::Confidence;
use autark_finance::payday::{Forecast, Past, Period as Payday};
use autark_finance::recommend::Level;
use autark_finance::spending::Summary;
use autark_finance::trends::{Period, Trends};
use chrono::NaiveDate;

pub fn kwh(amount: f64) -> String {
    format!("{amount:.1} kWh")
}

/// `1 Nov`.
pub fn day(date: NaiveDate) -> String {
    date.format("%-d %b").to_string()
}

/// The cycle as a range: `1 Nov – 30 Nov`.
pub fn cycle(plan: &Plan) -> String {
    format!("{} – {}", day(plan.cycle.start), day(plan.cycle.end - chrono::Duration::days(1)))
}

/// One line on how far to trust the projection.
pub fn basis(plan: &Plan) -> String {
    let days = |n: i64| if n == 1 { "1 day".to_string() } else { format!("{n} days") };
    match plan.basis {
        Basis::Readings => format!("From {} of meter readings", days(plan.span_days)),
        Basis::Early => format!("Early estimate: only {} of readings so far", days(plan.span_days)),
        Basis::Appliances if plan.span_days > 0 => {
            format!("From your appliance list: the meter has only {} so far", days(plan.span_days))
        }
        Basis::Appliances => "From your appliance list: no meter readings this cycle yet".to_string(),
        Basis::None => "Nothing to project from yet: add a meter reading".to_string(),
    }
}

/// One line on the rate.
pub fn rate(plan: &Plan) -> String {
    match plan.rate_source {
        RateSource::Bill => format!("₱{:.2}/kWh, from your last bill", plan.rate),
        RateSource::Fallback => format!("₱{:.2}/kWh, a placeholder until you add a bill", plan.rate),
    }
}

/// How the projection sits against the budget.
pub fn verdict(plan: &Plan) -> String {
    if plan.basis == Basis::None {
        return format!("Budget {}", plan.budget);
    }
    if plan.is_over() {
        format!("{} over the {} budget", plan.margin.abs(), plan.budget)
    } else {
        format!("{} under the {} budget", plan.margin, plan.budget)
    }
}

/// What is left to use each day to stay on budget.
pub fn allowance(plan: &Plan) -> String {
    let days = if plan.days_left == 1 { "1 day".to_string() } else { format!("{} days", plan.days_left) };
    if plan.allowance_kwh_per_day <= 0.0 {
        format!("The budget's kWh are used up: anything more goes over, with {days} to go")
    } else {
        format!("{} a day for the next {days} keeps you on budget", kwh(plan.allowance_kwh_per_day))
    }
}

/// The whole plan, for the terminal.
pub fn plan(plan: &Plan) -> String {
    let mut lines = vec![format!("Electricity · {} ({} days, {} left)", cycle(plan), plan.cycle.days(), plan.days_left)];
    lines.push(format!("Used so far  {}", kwh(plan.used_kwh)));
    if plan.basis == Basis::None {
        lines.push(basis(plan));
        lines.push("Add one:     autark-budget electric add <kWh>".to_string());
        lines.push(verdict(plan));
        return lines.join("\n");
    }
    lines.push(format!("Pace         {}/day", kwh(plan.daily_kwh)));
    lines.push(format!("Projected    {} = {}", kwh(plan.projected_kwh), plan.projected_cost));
    lines.push(format!("Rate         {}", rate(plan)));
    lines.push(format!("Budget       {}", verdict(plan)));
    lines.push(format!("Allowance    {}", allowance(plan)));
    lines.push(format!("Basis        {}", basis(plan)));
    lines.join("\n")
}

/// What the trends say, a line each, with what to do about it.
pub fn trend_lines(trends: &Trends) -> Vec<String> {
    let mut lines = Vec::new();
    if trends.days.is_empty() {
        return vec!["Trends start once you have two readings on different days.".to_string()];
    }
    let n = trends.days.len();
    lines.push(match (trends.this_week, trends.week_change) {
        (Some(now), Some(change)) if change.abs() < 0.03 => format!("Last 7 days: {} a day, about the same as the 7 before.", kwh(now)),
        (Some(now), Some(change)) => format!(
            "Last 7 days: {} a day, {:.0}% {} than the 7 before.",
            kwh(now),
            change.abs() * 100.0,
            if change > 0.0 { "more" } else { "less" }
        ),
        (Some(now), None) => format!("Last 7 days: {} a day. A comparison needs another week of readings.", kwh(now)),
        (None, _) => format!("Only {n} {} of readings so far; a week needs 7.", if n == 1 { "day" } else { "days" }),
    });
    lines.push(format!("Your budget works out to {} a day over the whole cycle; {} of the last {n} days went over that.", kwh(trends.budget_kwh_per_day), trends.days_over));
    lines.push(match (trends.busiest, trends.quietest) {
        (Some((high, most)), Some((low, least))) if high != low => format!("You use the most on {high} ({}) and the least on {low} ({}).", kwh(most), kwh(least)),
        _ => "Weekday habits show after about two weeks of daily readings. Days you skip a reading share their use, so they don't count.".to_string(),
    });
    lines.push(format!(
        "Every 1 kWh a day you cut saves about {} a cycle: an hour less of a 1,000 W appliance, or 10 hours less of a 100 W fan.",
        trends.saved_per_kwh_day
    ));
    lines
}

/// The last days as text bars, for the terminal.
pub fn trend_chart(trends: &Trends) -> String {
    let top = trends.days.iter().map(|d| d.kwh).fold(trends.budget_kwh_per_day, f64::max).max(0.1);
    trends
        .days
        .iter()
        .map(|d| {
            let width = ((d.kwh / top) * 24.0).round() as usize;
            let flag = if d.kwh > trends.budget_kwh_per_day { " over" } else { "" };
            format!("{:<7} {:<24} {:>4.1}{}{}", d.date.format("%a %-d"), "█".repeat(width), d.kwh, if d.measured { "" } else { " ~" }, flag)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn level_mark(level: Level) -> &'static str {
    match level {
        Level::Alert => "[!!]",
        Level::Warn => "[! ]",
        Level::Info => "[i ]",
        Level::Good => "[ok]",
    }
}

/// Weeks, newest first, a line each.
pub fn period_lines(periods: &[Period], what: &str) -> Vec<String> {
    periods
        .iter()
        .rev()
        .map(|p| {
            let partial = if p.full() { String::new() } else { format!(" ({} of {} days)", p.days, p.length) };
            format!("{what} {:<6} {:>5.1} kWh a day, {} so far{partial}", day(p.start), p.kwh_per_day, p.cost)
        })
        .collect()
}

/// Billing cycles, newest first, a line each.
pub fn cycle_lines(periods: &[Period]) -> Vec<String> {
    periods
        .iter()
        .rev()
        .map(|p| {
            let end = p.start + chrono::Duration::days(p.length as i64 - 1);
            let partial = if p.full() { String::new() } else { format!(" ({} of {} days)", p.days, p.length) };
            format!("{} – {}: {} ({} a day), {}{partial}", day(p.start), day(end), kwh(p.total_kwh), kwh(p.kwh_per_day), p.cost)
        })
        .collect()
}

/// `30 Oct – 14 Nov · 16 days`.
pub fn period_title(period: &Payday) -> String {
    format!("{} – {} · {} days", day(period.start), day(period.end - chrono::Duration::days(1)), period.days())
}

/// A mark in front of an amount that is an estimate rather than a fixed figure.
pub fn confidence_mark(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Firm => "",
        Confidence::Estimate => "≈ ",
        Confidence::Guess => "≈? ",
    }
}

/// One payday, for the terminal.
pub fn period_text(period: &Payday) -> String {
    let mut lines = vec![format!("{}{}", period_title(period), if period.current { "  (now)" } else { "" })];
    lines.push(format!("  Paycheck             {}", period.income));
    for line in &period.lines {
        let paid = if line.paid { "  paid" } else { "" };
        lines.push(format!("  - {:<22} {}{}  due {}{paid}", line.name, confidence_mark(line.confidence), line.amount, day(line.due)));
    }
    let verdict = if period.is_short() { format!("  SHORT by {}", period.left.abs()) } else { format!("  Left to spend        {}  ({} a day)", period.left, period.per_day()) };
    lines.push(verdict);
    lines.join("\n")
}

/// Where today stands against the paycheck's spending money.
pub fn spend_text(summary: &Summary) -> String {
    let mut lines = Vec::new();
    if summary.today_left.is_negative() {
        lines.push(format!("Over today's share by {}. Each day after gets {} if you spend nothing more.\n", summary.today_left.abs(), summary.per_day_after));
    }
    lines.extend([
        format!("Today        {} left of {}  (spent {})", summary.today_left, summary.today_budget, summary.spent_today),
        format!("This payday  {} left of {}, {} to go", summary.left, summary.to_spend, if summary.days_left == 1 { "last day".to_string() } else { format!("{} days", summary.days_left) }),
    ]);
    if summary.days_left > 1 {
        lines.push(format!("Tomorrow on  {} a day if you spend nothing more today", summary.per_day_after));
    }
    if !summary.by_category.is_empty() {
        lines.push(format!("By category  {}", summary.by_category.iter().map(|(name, total)| format!("{name} {total}")).collect::<Vec<_>>().join(", ")));
    }
    lines.join("\n")
}

/// A goal, debt or installment: how far along, and when it ends.
pub fn forecast_line(forecast: &Forecast) -> String {
    let ends = forecast.ends.map_or(String::new(), |date| format!(", done {}", date.format("%-d %b %Y")));
    format!("{}: {} of {} paid, {} left in {} {}{ends}", forecast.name, forecast.paid, forecast.total, forecast.remaining, forecast.payments_left, if forecast.payments_left == 1 { "payment" } else { "payments" })
}

/// The same as `forecast_line` without the name, for a card that already has it as its title.
pub fn progress_line(forecast: &Forecast) -> String {
    let ends = forecast.ends.map_or(String::new(), |date| format!(", done {}", date.format("%-d %b %Y")));
    format!("{} of {} paid · {} left in {} {}{ends}", forecast.paid, forecast.total, forecast.remaining, forecast.payments_left, if forecast.payments_left == 1 { "payment" } else { "payments" })
}

/// What a finished payday left by the plan, and what came of it: "15 Nov – 29 Nov: planned ₱3,948, spent ₱2,100, kept ₱1,848".
pub fn past_line(past: &Past) -> String {
    let span = format!("{} – {}", day(past.period.start), day(past.period.end - chrono::Duration::days(1)));
    if !past.logged {
        return format!("{span}: planned {} to spend, no spending logged", past.period.left);
    }
    let kept = past.kept();
    let result = if kept.is_negative() { format!("over by {}", kept.abs()) } else { format!("kept {kept}") };
    format!("{span}: planned {}, spent {}, {result}", past.period.left, past.spent)
}

/// The answer to "can I afford this?" in words.
pub fn afford_text(verdict: &Verdict) -> String {
    let days = if verdict.days_left == 1 { "today".to_string() } else { format!("{} days", verdict.days_left) };
    if verdict.fits {
        let mut out = format!("Yes. {} leaves {} of this payday's {}: {} a day for {days}, down from {}.", verdict.amount, verdict.left - verdict.amount, verdict.left, verdict.per_day_after, verdict.per_day_now);
        if verdict.tight {
            out.push_str(" That is tight: each day would get less than half of what it does now.");
        }
        return out;
    }
    let mut out = format!("Not this payday: {} is {} more than the {} left.", verdict.amount, verdict.short_by, verdict.left.max_zero());
    match verdict.next_fit {
        Some((start, left)) => out.push_str(&format!(" The payday of {} leaves {}, enough for it.", day(start), left)),
        None => out.push_str(" No payday in the next six months leaves enough."),
    }
    out
}

/// A month for the terminal: a line for each day that has a payday, a bill or a purchase.
pub fn month_text(month: &Month) -> String {
    let title = chrono::NaiveDate::from_ymd_opt(month.year, month.month, 1).map_or(String::new(), |d| d.format("%B %Y").to_string());
    let mut out = vec![format!("{title}  ·  {} {}  ·  due {}  ·  spent {}", month.paydays, if month.paydays == 1 { "payday" } else { "paydays" }, month.due, month.spent)];
    for day in month.days().filter(|d| d.in_month && (d.payday || !d.lines.is_empty() || d.purchases > 0)) {
        let mut parts = Vec::new();
        if day.payday {
            parts.push("PAYDAY".to_string());
        }
        for line in &day.lines {
            parts.push(format!("{}{} {}{}", if line.paid { "✓ " } else { "" }, line.name, confidence_mark(line.confidence), line.amount));
        }
        if day.purchases > 0 {
            parts.push(format!("spent {}", day.spent));
        }
        out.push(format!("{}{:<11} {}", if day.today { "> " } else { "  " }, day.date.format("%a %-d %b"), parts.join(", ")));
    }
    if out.len() == 1 {
        out.push("  Nothing planned this month.".to_string());
    }
    out.join("\n")
}
