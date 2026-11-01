//! The command line: `autark-budget electric …`. It does what the window does, for logging a reading in a few
//! keystrokes. Everything here takes the store, the settings and today's date as arguments, so it is tested without
//! touching your files.

use autark_finance::electric::{self, Appliance, Bill, Reading};
use autark_finance::payday::Payment;
use autark_finance::plan::{Every, Item, Kind};
use autark_finance::spending::{self, Spend};
use autark_finance::money::Money;
use autark_finance::store::Store;
use chrono::NaiveDate;

use crate::settings::Settings;
use crate::text;

pub const USAGE: &str = "\
autark-budget                                     open the window
autark-budget electric add <kWh> [--date D] [--note TEXT] [--force]
                                                  log what the meter says (D is YYYY-MM-DD, default today)
autark-budget electric plan                       the projected bill for this cycle
autark-budget electric list                       recent readings and bills
autark-budget electric trends                     use by day, week, billing cycle and weekday
autark-budget electric tips                       what to change, from the thresholds in budget.toml
autark-budget electric undo                       take back the reading you added last
autark-budget electric bill --kwh N --amount P --from D --to D
                                                  record a bill, so the rate becomes yours
autark-budget electric whatif <watts> <hours>     what an appliance would add to a cycle
autark-budget electric appliance add <name> <watts> <hours/day>
autark-budget electric appliance list
autark-budget electric appliance remove <name>
autark-budget electric set budget <pesos>
autark-budget electric set rate <pesos per kWh>   the rate used until a bill gives one
autark-budget electric set billing-day <1-28>

autark-budget plan [paydays]                      each payday: what is due, and what is left to spend
                                                  (the next 4, or how many you ask for)
autark-budget plan add <name> --kind bill|goal|debt|installment --amount P --every payday|month|once
                       [--day N] [--date D] [--times N] [--target P] [--saved P]
                                                  --day is the day of the month (31 is the last day);
                                                  --date is when it is due (once) or starts (the others)
autark-budget plan remove <name>
autark-budget plan set paycheck <pesos>
autark-budget plan set paydays <15,30>
autark-budget plan set categories <Food,Transport,Load>   the spending buttons (Other is always there)
autark-budget calendar [YYYY-MM|next|last]        a month: paydays, what falls due, what you spent
autark-budget afford <pesos>                      would it fit in what this payday has left?
autark-budget history [paydays]                   how the last paydays went: planned, spent and kept
autark-budget backup [folder]                     copy your settings and logs (default ~/Backups/autark-budget/<date>)
autark-budget restore <folder>                    put a backup back (what was there is kept in before-restore)
autark-budget remind [--always]                   a notification on payday, and the day before a bill is due
autark-budget paid <name> [--amount P]            tick off something due this payday (a goal: what you put in)
autark-budget unpaid <name>                       take the tick back
autark-budget spend <pesos> [category] [--note T] [--date D]
                                                  log what you spent, and see what is left today
autark-budget spend list                          this payday's spending
autark-budget spend undo                          take back the last one";

/// What a command did: what to print, and whether the settings changed and need saving.
#[derive(Debug)]
pub struct Outcome {
    pub text: String,
    pub settings_changed: bool,
}

fn said(text: impl Into<String>) -> Result<Outcome, String> {
    Ok(Outcome { text: text.into(), settings_changed: false })
}

pub fn run(args: &[String], store: &Store, settings: &mut Settings, today: NaiveDate) -> Result<Outcome, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["electric", "add", rest @ ..] => add(rest, store, settings, today),
        ["plan"] | ["plan", "show"] => plan_show(store, settings, today, 4),
        ["plan", count] if count.parse::<usize>().is_ok() => plan_show(store, settings, today, count.parse::<usize>().unwrap_or(4).clamp(1, 48)),
        ["plan", "add", rest @ ..] => plan_add(rest, settings, today),
        ["plan", "remove", name] => match settings.finance.remove(name) {
            Some(item) => Ok(Outcome { text: format!("Removed {}.", item.name), settings_changed: true }),
            None => Err(format!("No item called {name}")),
        },
        ["plan", "set", key, value] => plan_set(key, value, settings),
        ["calendar"] => calendar(None, store, settings, today),
        ["calendar", month] => calendar(Some(month), store, settings, today),
        ["afford", amount] => afford(amount, store, settings, today),
        ["history"] => history(store, settings, today, 6),
        ["history", count] => history(store, settings, today, count.parse::<usize>().map_err(|_| format!("“{count}” is not a number of paydays"))?.clamp(1, 48)),
        ["paid", rest @ ..] => paid(rest, store, settings, today),
        ["unpaid", name] => unpaid(name, store, settings, today),
        ["spend", "list"] => spend_list(store, settings, today),
        ["spend", "undo"] => spend_undo(store, settings, today),
        ["spend", rest @ ..] => spend_add(rest, store, settings, today),
        ["electric", "plan"] => said(text::plan(&plan(store, settings, today))),
        ["electric", "list"] => list(store),
        ["electric", "trends"] => trends(store, settings, today),
        ["electric", "tips"] => tips(store, settings, today),
        ["electric", "undo"] => match store.remove_last_reading().map_err(|e| e.to_string())? {
            Some(reading) => said(format!("Took back {} on {}.", text::kwh(reading.kwh), text::day(reading.date))),
            None => said("No readings to take back."),
        },
        ["electric", "bill", rest @ ..] => bill(rest, store, settings, today),
        ["electric", "whatif", watts, hours] => whatif(watts, hours, store, settings, today),
        ["electric", "appliance", rest @ ..] => appliance(rest, settings),
        ["electric", "set", key, value] => set(key, value, settings),
        _ => Err(format!("I don't know that one.\n\n{USAGE}")),
    }
}

fn plan(store: &Store, settings: &Settings, today: NaiveDate) -> electric::Plan {
    electric::plan(&settings.electric, &store.readings(), &store.bills(), today)
}

pub fn parse_date(text: &str, today: NaiveDate) -> Result<NaiveDate, String> {
    if text.eq_ignore_ascii_case("today") {
        return Ok(today);
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| format!("“{text}” is not a date: write it as YYYY-MM-DD"))
}

pub fn parse_number(text: &str, what: &str) -> Result<f64, String> {
    match text.trim().replace(',', "").parse::<f64>() {
        Ok(number) if number.is_finite() && number >= 0.0 => Ok(number),
        _ => Err(format!("“{text}” is not a number for {what}")),
    }
}

/// The value after `--name`, taken out of `args`.
fn option<'a>(args: &mut Vec<&'a str>, name: &str) -> Result<Option<&'a str>, String> {
    let Some(at) = args.iter().position(|arg| *arg == name) else { return Ok(None) };
    if at + 1 >= args.len() {
        return Err(format!("{name} needs a value"));
    }
    let value = args[at + 1];
    args.drain(at..=at + 1);
    Ok(Some(value))
}

fn flag(args: &mut Vec<&str>, name: &str) -> bool {
    let before = args.len();
    args.retain(|arg| *arg != name);
    args.len() != before
}

/// Checks a reading against today and its neighbours, the way a typo shows: a meter only counts up, so a reading
/// cannot be lower than one before it or higher than one after it, and it cannot be from a day that has not come.
pub fn check_reading_in(others: &[Reading], date: NaiveDate, kwh: f64, today: NaiveDate) -> Result<(), String> {
    if date > today {
        return Err(format!("{} has not come yet (today is {}), so the meter cannot have been read then.", text::day(date), text::day(today)));
    }
    let mut sorted: Vec<&Reading> = others.iter().collect();
    sorted.sort_by_key(|r| r.date);
    if let Some(before) = sorted.iter().rev().find(|r| r.date <= date)
        && kwh < before.kwh
    {
        return Err(format!(
            "The meter read {} on {}, so {} on {} would be going backwards.",
            text::kwh(before.kwh),
            text::day(before.date),
            text::kwh(kwh),
            text::day(date)
        ));
    }
    if let Some(after) = sorted.iter().find(|r| r.date > date)
        && kwh > after.kwh
    {
        return Err(format!("The meter read {} on {}, so {} on {} is higher than a later reading.", text::kwh(after.kwh), text::day(after.date), text::kwh(kwh), text::day(date)));
    }
    Ok(())
}

pub fn check_reading(store: &Store, date: NaiveDate, kwh: f64, today: NaiveDate) -> Result<(), String> {
    check_reading_in(&store.readings(), date, kwh, today)
}

fn add(args: &[&str], store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let mut args = args.to_vec();
    let date = option(&mut args, "--date")?.map_or(Ok(today), |d| parse_date(d, today))?;
    let note = option(&mut args, "--note")?.unwrap_or_default().to_string();
    let force = flag(&mut args, "--force");
    let [kwh] = args.as_slice() else { return Err("usage: autark-budget electric add <kWh> [--date D] [--note TEXT]".into()) };
    let kwh = parse_number(kwh, "the meter reading")?;
    // A date that has not come is a typo even when forced, since the plan only looks at readings up to today.
    if date > today {
        check_reading(store, date, kwh, today)?;
    }
    if !force {
        // Only the terminal has --force; the window asks for the button to be pressed again.
        check_reading(store, date, kwh, today).map_err(|message| if date > today { message } else { format!("{message} To keep it anyway, add it with --force.") })?;
    }
    store.add_reading(&Reading { date, kwh, note }).map_err(|e| format!("could not save: {e}"))?;
    let plan = plan(store, settings, today);
    said(format!("Logged {} on {}.\n\n{}", text::kwh(kwh), text::day(date), text::plan(&plan)))
}

fn trends(store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let readings = store.readings();
    let plan = electric::plan(&settings.electric, &readings, &store.bills(), today);
    let trends = autark_finance::trends::trends(&readings, &plan);
    let mut text = String::new();
    if !trends.days.is_empty() {
        text.push_str(&self::text::trend_chart(&trends));
        text.push_str("\n(~ is an average over days with no reading of their own)\n\n");
    }
    if !trends.weeks.is_empty() {
        text.push_str("Weeks\n");
        text.push_str(&self::text::period_lines(&trends.weeks, "Week of").join("\n"));
        text.push_str("\n\nBilling cycles\n");
        text.push_str(&self::text::cycle_lines(&trends.cycles).join("\n"));
        text.push_str("\n\n");
    }
    text.push_str(&self::text::trend_lines(&trends).join("\n"));
    said(text)
}

fn tips(store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let readings = store.readings();
    let plan = electric::plan(&settings.electric, &readings, &store.bills(), today);
    let trends = autark_finance::trends::trends(&readings, &plan);
    let tips = autark_finance::recommend::recommend(&plan, &trends, &settings.electric.thresholds);
    said(tips.iter().map(|tip| format!("{}  {}\n    {}", self::text::level_mark(tip.level), tip.title, tip.detail)).collect::<Vec<_>>().join("\n\n"))
}

fn list(store: &Store) -> Result<Outcome, String> {
    let mut readings = store.readings();
    readings.sort_by_key(|r| r.date);
    let mut lines = Vec::new();
    for (index, reading) in readings.iter().enumerate().rev().take(10) {
        let change = index.checked_sub(1).map(|before| reading.kwh - readings[before].kwh);
        let change = change.map_or(String::new(), |change| format!("  +{change:.1}"));
        let note = if reading.note.is_empty() { String::new() } else { format!("  {}", reading.note) };
        lines.push(format!("{}  {:>8.1} kWh{change}{note}", reading.date, reading.kwh));
    }
    if lines.is_empty() {
        lines.push("No readings yet. Log one: autark-budget electric add <kWh>".into());
    }
    let mut bills = store.bills();
    bills.sort_by_key(|b| b.to);
    if !bills.is_empty() {
        lines.push(String::new());
        for bill in bills.iter().rev().take(5) {
            let rate = bill.rate().map_or(String::new(), |rate| format!("  ₱{rate:.2}/kWh"));
            lines.push(format!("Bill {} – {}  {}  {}{rate}", text::day(bill.from), text::day(bill.to), text::kwh(bill.kwh), bill.amount));
        }
    }
    said(lines.join("\n"))
}

fn bill(args: &[&str], store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let mut args = args.to_vec();
    fn need<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, String> {
        value.ok_or_else(|| format!("{name} is needed: autark-budget electric bill --kwh N --amount P --from D --to D"))
    }
    let kwh = parse_number(need(option(&mut args, "--kwh")?, "--kwh")?, "kWh")?;
    let amount = need(option(&mut args, "--amount")?, "--amount")?;
    let amount = Money::parse(amount).filter(|m| !m.is_negative()).ok_or_else(|| format!("“{amount}” is not an amount"))?;
    let from = parse_date(need(option(&mut args, "--from")?, "--from")?, today)?;
    let to = parse_date(need(option(&mut args, "--to")?, "--to")?, today)?;
    if !args.is_empty() {
        return Err(format!("I don't know “{}”", args.join(" ")));
    }
    if to < from {
        return Err("The bill ends before it starts".into());
    }
    if kwh <= 0.0 {
        return Err("A bill needs the kWh it covers, to work out the rate".into());
    }
    let bill = Bill { from, to, kwh, amount };
    store.add_bill(&bill).map_err(|e| format!("could not save: {e}"))?;
    let plan = plan(store, settings, today);
    said(format!("Recorded {amount} for {}; that is {}.\n\n{}", text::kwh(kwh), text::rate(&plan), text::plan(&plan)))
}

fn whatif(watts: &str, hours: &str, store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let (watts, hours) = (parse_number(watts, "watts")?, parse_number(hours, "hours")?);
    let plan = plan(store, settings, today);
    let (kwh, cost) = electric::what_if(watts, hours, plan.rate, plan.cycle.days());
    said(format!("{watts:.0} W for {hours:.1} h a day adds {} and {cost} to a {}-day cycle, at {}.", text::kwh(kwh), plan.cycle.days(), text::rate(&plan)))
}

fn appliance(args: &[&str], settings: &mut Settings) -> Result<Outcome, String> {
    let list = &mut settings.electric.appliances;
    match args {
        ["add", name, watts, hours] => {
            let (watts, hours) = (parse_number(watts, "watts")?, parse_number(hours, "hours a day")?);
            list.retain(|a| !a.name.eq_ignore_ascii_case(name));
            list.push(Appliance { name: (*name).to_string(), watts, hours_per_day: hours });
            Ok(Outcome { text: format!("Saved {name}: {watts:.0} W, {hours:.1} h a day."), settings_changed: true })
        }
        ["remove", name] => {
            let before = list.len();
            list.retain(|a| !a.name.eq_ignore_ascii_case(name));
            if list.len() == before {
                return Err(format!("No appliance called {name}"));
            }
            Ok(Outcome { text: format!("Removed {name}."), settings_changed: true })
        }
        ["list"] => {
            if list.is_empty() {
                return said("No appliances yet: autark-budget electric appliance add <name> <watts> <hours/day>");
            }
            let total: f64 = list.iter().map(Appliance::kwh_per_day).sum();
            let mut lines: Vec<String> =
                list.iter().map(|a| format!("{:<14} {:>6.0} W  {:>4.1} h/day  {}/day", a.name, a.watts, a.hours_per_day, text::kwh(a.kwh_per_day()))).collect();
            lines.push(format!("{:<14} {}/day", "Total", text::kwh(total)));
            said(lines.join("\n"))
        }
        _ => Err("usage: autark-budget electric appliance add <name> <watts> <hours/day> | list | remove <name>".into()),
    }
}

fn set(key: &str, value: &str, settings: &mut Settings) -> Result<Outcome, String> {
    let config = &mut settings.electric;
    let text = match key {
        "budget" => {
            config.budget = Money::parse(value).filter(|m| !m.is_negative()).ok_or_else(|| format!("“{value}” is not an amount"))?;
            format!("Electricity budget is {}.", config.budget)
        }
        "rate" => {
            config.fallback_rate = parse_number(value, "the rate")?;
            format!("Placeholder rate is ₱{:.2}/kWh, used until a bill gives yours.", config.fallback_rate)
        }
        "billing-day" => {
            let day: u32 = value.parse().map_err(|_| format!("“{value}” is not a day of the month"))?;
            if !(1..=28).contains(&day) {
                return Err("The billing day is 1 to 28, so every month has it".into());
            }
            config.billing_day = day;
            format!("A cycle starts on day {day} of the month.")
        }
        _ => return Err(format!("I can set budget, rate or billing-day, not “{key}”")),
    };
    Ok(Outcome { text, settings_changed: true })
}

fn plan_show(store: &Store, settings: &Settings, today: NaiveDate, count: usize) -> Result<Outcome, String> {
    if settings.finance.paycheck.centavos() <= 0 && settings.finance.items.is_empty() {
        return said("Nothing planned yet. Start with: autark-budget plan set paycheck <pesos>");
    }
    let periods = crate::view::periods(store, settings, today, count);
    if periods.is_empty() {
        return Err("No paydays: set them with autark-budget plan set paydays 15,30".into());
    }
    let mut out = periods.iter().map(text::period_text).collect::<Vec<_>>().join("\n\n");
    let forecasts = autark_finance::payday::forecasts(&settings.finance, &store.payments(), today);
    if !forecasts.is_empty() {
        out.push_str("\n\n");
        out.push_str(&forecasts.iter().map(text::forecast_line).collect::<Vec<_>>().join("\n"));
    }
    said(out)
}

fn plan_add(args: &[&str], settings: &mut Settings, today: NaiveDate) -> Result<Outcome, String> {
    let mut args = args.to_vec();
    let kind = option(&mut args, "--kind")?.map_or(Ok(Kind::Bill), |k| Kind::parse(k).ok_or_else(|| format!("“{k}” is not a kind: bill, goal, debt or installment")))?;
    let every = option(&mut args, "--every")?.map_or(Ok(Every::Month), |e| Every::parse(e).ok_or_else(|| format!("“{e}” is not how often: payday, month or once")))?;
    let amount = option(&mut args, "--amount")?.ok_or("--amount is needed")?;
    let amount = Money::parse(amount).ok_or_else(|| format!("“{amount}” is not an amount"))?;
    let day = option(&mut args, "--day")?.map(|d| d.parse::<u32>().map_err(|_| format!("“{d}” is not a day of the month"))).transpose()?;
    let date = option(&mut args, "--date")?.map(|d| parse_date(d, today)).transpose()?;
    let times = option(&mut args, "--times")?.map(|t| t.parse::<u32>().map_err(|_| format!("“{t}” is not a number of times"))).transpose()?;
    let target = option(&mut args, "--target")?.map(|t| Money::parse(t).ok_or_else(|| format!("“{t}” is not an amount"))).transpose()?;
    let saved = option(&mut args, "--saved")?.map(|t| Money::parse(t).ok_or_else(|| format!("“{t}” is not an amount"))).transpose()?.unwrap_or_default();
    let [name] = args.as_slice() else { return Err("usage: autark-budget plan add <name> --kind K --amount P --every E [--day N] [--date D] [--times N]".into()) };
    let item = Item { name: (*name).to_string(), kind, amount, every, day, date, times, target, saved };
    let replaced = settings.finance.find(name).is_some();
    settings.finance.put(item.clone())?;
    Ok(Outcome { text: format!("{} {}: {} {}.", if replaced { "Updated" } else { "Added" }, item.name, item.amount, item.schedule()), settings_changed: true })
}

fn plan_set(key: &str, value: &str, settings: &mut Settings) -> Result<Outcome, String> {
    let config = &mut settings.finance;
    let text = match key {
        "paycheck" => {
            config.paycheck = Money::parse(value).filter(|m| !m.is_negative()).ok_or_else(|| format!("“{value}” is not an amount"))?;
            format!("Each payday brings {}.", config.paycheck)
        }
        "categories" => {
            config.set_categories(value)?;
            if config.categories.is_empty() { "No spending categories besides Other.".to_string() } else { format!("Spending buttons: {}, Other.", config.categories.join(", ")) }
        }
        "paydays" => {
            let days: Result<Vec<u32>, _> = value.split(',').map(|d| d.trim().parse::<u32>()).collect();
            let days = days.map_err(|_| format!("“{value}” is not a list of days, like 15,30"))?;
            if days.is_empty() || days.iter().any(|d| !(1..=31).contains(d)) {
                return Err("Paydays are days of the month, 1 to 31, like 15,30".into());
            }
            config.pay_days = days;
            format!("Paydays are the {} of each month.", config.pay_days.iter().map(u32::to_string).collect::<Vec<_>>().join(" and "))
        }
        _ => return Err(format!("I can set paycheck, paydays or categories, not “{key}”")),
    };
    Ok(Outcome { text, settings_changed: true })
}

/// A month of the plan: `calendar`, `calendar 2026-12`, `calendar next` or `calendar last`.
fn calendar(which: Option<&str>, store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let (year, month) = match which {
        None => (chrono::Datelike::year(&today), chrono::Datelike::month(&today)),
        Some("next") => autark_finance::calendar::shift(chrono::Datelike::year(&today), chrono::Datelike::month(&today), 1),
        Some("last") => autark_finance::calendar::shift(chrono::Datelike::year(&today), chrono::Datelike::month(&today), -1),
        Some(text) => {
            let bad = || format!("“{text}” is not a month: use 2026-12, next or last");
            let (y, m) = text.split_once('-').ok_or_else(bad)?;
            (y.parse::<i32>().map_err(|_| bad())?, m.parse::<u32>().ok().filter(|m| (1..=12).contains(m)).ok_or_else(bad)?)
        }
    };
    let month = crate::view::month(store, settings, year, month, today).ok_or("There is no such month")?;
    said(text::month_text(&month))
}

/// Whether an amount fits in this payday.
fn afford(amount: &str, store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let amount = Money::parse(amount).filter(|m| m.centavos() > 0).ok_or_else(|| format!("“{amount}” is not an amount"))?;
    let verdict = crate::view::afford(store, settings, amount, today).ok_or("No paydays are set: autark-budget plan set paydays 15,30")?;
    said(text::afford_text(&verdict))
}

/// How the last few finished paydays went.
fn history(store: &Store, settings: &Settings, today: NaiveDate, count: usize) -> Result<Outcome, String> {
    let past = autark_finance::payday::history(&settings.finance, &store.payments(), &store.spending(), today, count);
    if past.is_empty() {
        return said("No finished paydays yet.");
    }
    let mut out = past.iter().map(text::past_line).collect::<Vec<_>>().join("\n");
    out.push_str("\n\nWorked out from today's plan, so a bill you added or changed since counts in every payday.");
    said(out)
}

/// The line of this payday that `name` points to.
fn this_payday(name: &str, store: &Store, settings: &Settings, today: NaiveDate) -> Result<(autark_finance::payday::Period, autark_finance::payday::Line), String> {
    let period = crate::view::current(store, settings, today).ok_or("No paydays are set")?;
    match period.lines.iter().find(|line| line.name.eq_ignore_ascii_case(name)).cloned() {
        Some(line) => Ok((period, line)),
        None => Err(format!("Nothing called {name} is due this payday. Due now: {}", period.lines.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

fn paid(args: &[&str], store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let mut args = args.to_vec();
    let amount = option(&mut args, "--amount")?.map(|a| Money::parse(a).filter(|m| m.centavos() > 0).ok_or_else(|| format!("“{a}” is not an amount"))).transpose()?;
    let [name] = args.as_slice() else { return Err("usage: autark-budget paid <name> [--amount P]".into()) };
    let (period, line) = this_payday(name, store, settings, today)?;
    if line.paid {
        return said(format!("{} is already ticked off for this payday.", line.name));
    }
    let amount = amount.unwrap_or(line.amount);
    store.add_payment(&Payment { item: line.name.clone(), period: period.start, date: today, amount }).map_err(|e| format!("could not save: {e}"))?;
    let after = crate::view::current(store, settings, today).map_or(String::new(), |p| format!(" {} of this payday is spoken for, {} is yours.", p.due, p.left));
    said(format!("Paid {}: {amount}.{after}", line.name))
}

fn unpaid(name: &str, store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let (period, line) = this_payday(name, store, settings, today)?;
    let mut all = store.payments();
    let before = all.len();
    all.retain(|p| !(p.period == period.start && p.item.eq_ignore_ascii_case(&line.name)));
    if all.len() == before {
        return said(format!("{} was not ticked off.", line.name));
    }
    store.set_payments(&all).map_err(|e| format!("could not save: {e}"))?;
    said(format!("Took back the tick on {}.", line.name))
}

fn spend_summary(store: &Store, settings: &Settings, today: NaiveDate) -> Result<spending::Summary, String> {
    let period = crate::view::current(store, settings, today).ok_or("No paydays are set: autark-budget plan set paydays 15,30")?;
    Ok(spending::summarize(&period, &store.spending(), today))
}

fn spend_add(args: &[&str], store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let mut args = args.to_vec();
    let note = option(&mut args, "--note")?.unwrap_or_default().to_string();
    let date = option(&mut args, "--date")?.map_or(Ok(today), |d| parse_date(d, today))?;
    let (amount, category) = match args.as_slice() {
        [amount] => (*amount, ""),
        [amount, category] => (*amount, *category),
        _ => return Err("usage: autark-budget spend <pesos> [category] [--note T] [--date D]".into()),
    };
    let amount = Money::parse(amount).filter(|m| m.centavos() > 0).ok_or_else(|| format!("“{amount}” is not an amount"))?;
    if date > today {
        return Err(format!("{} has not come yet, so nothing was spent then.", text::day(date)));
    }
    store.add_spending(&Spend { date, amount, category: category.to_string(), note }).map_err(|e| format!("could not save: {e}"))?;
    said(format!("Spent {amount}{}.\n\n{}", if category.is_empty() { String::new() } else { format!(" on {category}") }, text::spend_text(&spend_summary(store, settings, today)?)))
}

fn spend_list(store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let period = crate::view::current(store, settings, today).ok_or("No paydays are set")?;
    let mut mine: Vec<Spend> = store.spending().into_iter().filter(|s| s.date >= period.start && s.date < period.end).collect();
    mine.sort_by_key(|s| s.date);
    if mine.is_empty() {
        return said("Nothing spent this payday yet. Log it: autark-budget spend <pesos> [category]");
    }
    let lines: Vec<String> = mine
        .iter()
        .rev()
        .map(|s| format!("{}  {:>10}  {}{}", text::day(s.date), s.amount.to_string(), if s.category.is_empty() { "Other" } else { &s.category }, if s.note.is_empty() { String::new() } else { format!("  {}", s.note) }))
        .collect();
    said(format!("{}\n\n{}", lines.join("\n"), text::spend_text(&spend_summary(store, settings, today)?)))
}

fn spend_undo(store: &Store, settings: &Settings, today: NaiveDate) -> Result<Outcome, String> {
    let mut all = store.spending();
    let Some(gone) = all.pop() else { return said("Nothing to take back.") };
    store.set_spending(&all).map_err(|e| format!("could not save: {e}"))?;
    said(format!("Took back {} on {}.\n\n{}", gone.amount, text::day(gone.date), text::spend_text(&spend_summary(store, settings, today)?)))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("autark-budget-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn day(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    fn go(line: &str, store: &Store, settings: &mut Settings, today: NaiveDate) -> Result<Outcome, String> {
        let args: Vec<String> = line.split_whitespace().map(String::from).collect();
        run(&args, store, settings, today)
    }

    #[test]
    fn logging_the_opening_reading_then_a_few_days() {
        let scratch = Scratch::new("log");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        let out = go("electric add 15.4 --note opening", &store, &mut settings, day(1)).unwrap();
        assert!(out.text.contains("Logged 15.4 kWh on 1 Nov"), "{}", out.text);
        assert!(out.text.contains("add a meter reading"), "one reading is not usage: {}", out.text);
        let out = go("electric add 18.4 --date 2026-11-04", &store, &mut settings, day(4)).unwrap();
        assert!(out.text.contains("Used so far  3.0 kWh"), "{}", out.text);
        assert!(out.text.contains("Early estimate"), "{}", out.text);
        assert!(out.text.contains("₱360"), "{}", out.text);
        assert!(!out.settings_changed);
    }

    #[test]
    fn a_reading_that_goes_backwards_is_refused_unless_forced() {
        let scratch = Scratch::new("backwards");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        go("electric add 100", &store, &mut settings, day(1)).unwrap();
        let error = go("electric add 10", &store, &mut settings, day(2)).err().unwrap();
        assert!(error.contains("going backwards"), "{error}");
        assert_eq!(store.readings().len(), 1);
        go("electric add 10 --force", &store, &mut settings, day(2)).unwrap();
        assert_eq!(store.readings().len(), 2);
    }

    #[test]
    fn a_reading_from_a_day_that_has_not_come_is_refused_even_when_forced() {
        let scratch = Scratch::new("future");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        let error = go("electric add 20 --date 2026-11-04", &store, &mut settings, day(1)).err().unwrap();
        assert!(error.contains("has not come yet"), "{error}");
        assert!(go("electric add 20 --date 2026-11-04 --force", &store, &mut settings, day(1)).is_err());
        assert!(store.readings().is_empty());
        go("electric add 20 --date 2026-11-04", &store, &mut settings, day(4)).unwrap();
    }

    #[test]
    fn a_reading_cannot_contradict_the_ones_around_it() {
        let scratch = Scratch::new("neighbours");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        go("electric add 10 --date 2026-11-01", &store, &mut settings, day(9)).unwrap();
        go("electric add 30 --date 2026-11-08", &store, &mut settings, day(9)).unwrap();
        // Between them is fine; above the later one, or below the earlier one, is not.
        assert!(go("electric add 20 --date 2026-11-04", &store, &mut settings, day(9)).is_ok());
        let high = go("electric add 35 --date 2026-11-05", &store, &mut settings, day(9)).err().unwrap();
        assert!(high.contains("higher than a later reading"), "{high}");
        assert!(go("electric add 5 --date 2026-11-05", &store, &mut settings, day(9)).is_err());
    }

    #[test]
    fn tips_and_trends_work_from_the_terminal() {
        let scratch = Scratch::new("tips");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        assert!(go("electric tips", &store, &mut settings, day(1)).unwrap().text.contains("Nothing to go on yet"));
        let mut meter = 10.0;
        for n in 1..=10u32 {
            meter += 4.0;
            go(&format!("electric add {meter} --date 2026-11-{n:02}"), &store, &mut settings, day(n)).unwrap();
        }
        let tips = go("electric tips", &store, &mut settings, day(10)).unwrap().text;
        assert!(tips.contains("over budget"), "{tips}");
        let trends = go("electric trends", &store, &mut settings, day(10)).unwrap().text;
        assert!(trends.contains("Weeks") && trends.contains("Billing cycles"), "{trends}");
    }

    #[test]
    fn a_bill_makes_the_rate_yours() {
        let scratch = Scratch::new("bill");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        let out = go("electric bill --kwh 80 --amount 1,040 --from 2026-10-01 --to 2026-10-31", &store, &mut settings, day(3)).unwrap();
        assert!(out.text.contains("₱13.00/kWh, from your last bill"), "{}", out.text);
        assert!(go("electric bill --kwh 80 --amount 1040 --from 2026-10-01", &store, &mut settings, day(3)).is_err());
        assert!(go("electric bill --kwh 0 --amount 5 --from 2026-10-01 --to 2026-10-31", &store, &mut settings, day(3)).is_err());
        assert!(go("electric bill --kwh 5 --amount 5 --from 2026-10-31 --to 2026-10-01", &store, &mut settings, day(3)).is_err());
    }

    #[test]
    fn undo_and_list() {
        let scratch = Scratch::new("undo");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        assert!(go("electric list", &store, &mut settings, day(1)).unwrap().text.contains("No readings yet"));
        go("electric add 15.4", &store, &mut settings, day(1)).unwrap();
        go("electric add 17.9", &store, &mut settings, day(2)).unwrap();
        let listed = go("electric list", &store, &mut settings, day(2)).unwrap().text;
        assert!(listed.contains("+2.5"), "{listed}");
        assert!(go("electric undo", &store, &mut settings, day(2)).unwrap().text.contains("17.9 kWh"));
        assert_eq!(store.readings().len(), 1);
    }

    #[test]
    fn settings_commands_change_and_check() {
        let scratch = Scratch::new("set");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        assert!(go("electric set budget 1,500", &store, &mut settings, day(1)).unwrap().settings_changed);
        assert_eq!(settings.electric.budget, Money::from_pesos(1500));
        go("electric set billing-day 15", &store, &mut settings, day(1)).unwrap();
        assert_eq!(settings.electric.billing_day, 15);
        assert!(go("electric set billing-day 31", &store, &mut settings, day(1)).is_err());
        assert!(go("electric set rate abc", &store, &mut settings, day(1)).is_err());
        assert!(go("electric set colour red", &store, &mut settings, day(1)).is_err());
    }

    #[test]
    fn appliances_are_kept_by_name_and_feed_the_early_estimate() {
        let scratch = Scratch::new("appliance");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        go("electric appliance add Fridge 100 10", &store, &mut settings, day(1)).unwrap();
        go("electric appliance add fridge 120 10", &store, &mut settings, day(1)).unwrap();
        assert_eq!(settings.electric.appliances.len(), 1, "same name replaces");
        assert!(go("electric appliance list", &store, &mut settings, day(1)).unwrap().text.contains("1.2 kWh/day"));
        go("electric add 15.4", &store, &mut settings, day(1)).unwrap();
        let out = go("electric add 15.9 --date 2026-11-03", &store, &mut settings, day(3)).unwrap();
        assert!(out.text.contains("appliance list"), "{}", out.text);
        assert!(go("electric appliance remove Kettle", &store, &mut settings, day(1)).is_err());
        go("electric appliance remove FRIDGE", &store, &mut settings, day(1)).unwrap();
        assert!(settings.electric.appliances.is_empty());
    }

    #[test]
    fn trends_show_the_days_and_say_what_a_cut_saves() {
        let scratch = Scratch::new("trends");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        assert!(go("electric trends", &store, &mut settings, day(1)).unwrap().text.contains("two readings"));
        go("electric add 10 --date 2026-11-01", &store, &mut settings, day(2)).unwrap();
        go("electric add 12 --date 2026-11-02", &store, &mut settings, day(2)).unwrap();
        go("electric add 17 --date 2026-11-03", &store, &mut settings, day(3)).unwrap();
        let out = go("electric trends", &store, &mut settings, day(3)).unwrap().text;
        assert!(out.contains("5.0 over"), "the 5 kWh day is over the 3.3 budget: {out}");
        assert!(out.contains("1 of the last 2 days went over that"), "{out}");
        assert!(out.contains("₱360"), "{out}");
    }

    #[test]
    fn whatif_prices_it_at_the_current_rate() {
        let scratch = Scratch::new("whatif");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        let out = go("electric whatif 1000 1", &store, &mut settings, day(14)).unwrap().text;
        assert!(out.contains("30.0 kWh") && out.contains("₱360"), "{out}");
    }

    #[test]
    fn mistakes_are_explained_not_swallowed() {
        let scratch = Scratch::new("errors");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        for line in ["", "electric", "electric add", "electric add abc", "electric add 5 --date tomorrow", "electric add 5 --date", "electric whatif x 1", "money"] {
            assert!(go(line, &store, &mut settings, day(1)).is_err(), "{line:?}");
        }
        assert!(go("electric add 5 --date 2026-13-01", &store, &mut settings, day(1)).err().unwrap().contains("YYYY-MM-DD"));
    }

    fn planned(store: &Store, settings: &mut Settings) {
        for line in [
            "plan set paycheck 12000",
            "plan set paydays 15,30",
            "plan add Laundry --amount 170 --every payday",
            "plan add Internet --amount 1599 --every month --day 15",
        ] {
            go(line, store, settings, day(1)).unwrap();
        }
    }

    #[test]
    fn planning_a_payday_shows_what_is_left() {
        let scratch = Scratch::new("plan");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        planned(&store, &mut settings);
        let out = go("plan", &store, &mut settings, day(1)).unwrap();
        assert!(out.text.contains("Laundry"), "{}", out.text);
        assert!(out.text.contains("Left to spend        ₱11,830"), "{}", out.text);
        assert!(go("plan add Rent --amount 4000 --every sometimes", &store, &mut settings, day(1)).is_err());
    }

    #[test]
    fn ticking_off_a_bill_is_kept_and_can_be_taken_back() {
        let scratch = Scratch::new("paid");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        planned(&store, &mut settings);
        let out = go("paid laundry --amount 180", &store, &mut settings, day(2)).unwrap();
        assert!(out.text.contains("Paid Laundry: ₱180"), "{}", out.text);
        assert!(go("paid Laundry", &store, &mut settings, day(2)).unwrap().text.contains("already"));
        assert_eq!(store.payments().len(), 1);
        go("unpaid Laundry", &store, &mut settings, day(2)).unwrap();
        assert!(store.payments().is_empty());
        assert!(go("paid Nothing", &store, &mut settings, day(2)).unwrap_err().contains("Due now"));
    }

    #[test]
    fn spending_is_logged_summed_and_taken_back() {
        let scratch = Scratch::new("spend");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        planned(&store, &mut settings);
        let out = go("spend 120 Food --note lunch", &store, &mut settings, day(1)).unwrap();
        assert!(out.text.contains("Spent ₱120 on Food"), "{}", out.text);
        assert!(go("spend list", &store, &mut settings, day(1)).unwrap().text.contains("lunch"));
        assert!(go("spend 50 --date 2026-12-25", &store, &mut settings, day(1)).is_err(), "nothing is spent in the future");
        assert!(go("spend abc", &store, &mut settings, day(1)).is_err());
        go("spend undo", &store, &mut settings, day(1)).unwrap();
        assert!(store.spending().is_empty());
    }

    #[test]
    fn spending_more_than_todays_share_warns_and_says_what_tomorrow_gets() {
        let scratch = Scratch::new("over");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        planned(&store, &mut settings);
        // ₱11,830 over 16 days is about ₱739 a day.
        let fine = go("spend 100 Food", &store, &mut settings, day(1)).unwrap();
        assert!(!fine.text.contains("Over today's share"), "{}", fine.text);
        let over = go("spend 900 Food", &store, &mut settings, day(1)).unwrap();
        assert!(over.text.contains("Over today's share by"), "{}", over.text);
        assert!(over.text.contains("Each day after gets"), "{}", over.text);
    }

    #[test]
    fn history_says_how_a_finished_payday_went() {
        let scratch = Scratch::new("history");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        planned(&store, &mut settings);
        assert_eq!(go("history", &store, &mut settings, day(1)).unwrap().text, "No finished paydays yet.");
        go("spend 1000 Food --date 2026-11-05", &store, &mut settings, day(6)).unwrap();
        let out = go("history", &store, &mut settings, day(20)).unwrap().text;
        assert!(out.starts_with("30 Oct – 14 Nov: planned ₱11,830, spent ₱1,000, kept ₱10,830"), "{out}");
        assert!(go("history abc", &store, &mut settings, day(20)).is_err());
    }

    #[test]
    fn the_spending_buttons_can_be_set_and_other_is_always_there() {
        let scratch = Scratch::new("categories");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        let out = go("plan set categories Food,Pets,other", &store, &mut settings, day(1)).unwrap();
        assert!(out.settings_changed);
        assert_eq!(out.text, "Spending buttons: Food, Pets, Other.");
        assert_eq!(settings.finance.categories, ["Food", "Pets"]);
        assert!(go("plan set categories a,b,c,d,e,f,g,h,i", &store, &mut settings, day(1)).is_err());
    }

    #[test]
    fn the_calendar_lists_the_days_that_have_something() {
        let scratch = Scratch::new("calendar");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        planned(&store, &mut settings);
        let out = go("calendar", &store, &mut settings, day(1)).unwrap().text;
        assert!(out.starts_with("November 2026  ·  2 paydays"), "{out}");
        assert!(out.contains("Sun 15 Nov") && out.contains("PAYDAY") && out.contains("Internet ₱1,599"), "{out}");
        assert!(go("calendar next", &store, &mut settings, day(1)).unwrap().text.starts_with("December 2026"));
        assert!(go("calendar 2027-02", &store, &mut settings, day(1)).unwrap().text.starts_with("February 2027"));
        assert!(go("calendar 2027-13", &store, &mut settings, day(1)).is_err());
        assert!(go("calendar soon", &store, &mut settings, day(1)).is_err());
    }

    #[test]
    fn afford_says_yes_or_no_with_the_numbers() {
        let scratch = Scratch::new("afford");
        let store = Store::open(&scratch.0);
        let mut settings = Settings::default();
        planned(&store, &mut settings);
        let yes = go("afford 500", &store, &mut settings, day(1)).unwrap().text;
        assert!(yes.starts_with("Yes. ₱500 leaves"), "{yes}");
        let no = go("afford 50000", &store, &mut settings, day(1)).unwrap().text;
        assert!(no.starts_with("Not this payday:"), "{no}");
        assert!(go("afford abc", &store, &mut settings, day(1)).is_err());
        assert!(go("afford 0", &store, &mut settings, day(1)).is_err());
    }
}
