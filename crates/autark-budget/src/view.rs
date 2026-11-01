//! The plan as the window and the terminal both see it: your items and paycheck, plus the electricity bill the meter
//! projects, laid out payday by payday.

use autark_finance::electric;
use autark_finance::afford::{self, Verdict};
use autark_finance::calendar::{self, Month};
use autark_finance::money::Money;
use autark_finance::payday::{self, Estimate, Period};
use autark_finance::store::Store;
use chrono::NaiveDate;

use crate::settings::Settings;

/// The electricity bill as a line in the plan, as sure as the meter lets it be.
pub fn electricity(store: &Store, settings: &Settings, today: NaiveDate) -> Estimate {
    let plan = electric::plan(&settings.electric, &store.readings(), &store.bills(), today);
    let obligation = plan.obligation();
    Estimate { item: obligation.monthly_item(), confidence: obligation.confidence }
}

/// `count` paydays' periods from the one `today` is in.
pub fn periods(store: &Store, settings: &Settings, today: NaiveDate, count: usize) -> Vec<Period> {
    payday::periods(&settings.finance, &store.payments(), &[electricity(store, settings, today)], today, count)
}

pub fn current(store: &Store, settings: &Settings, today: NaiveDate) -> Option<Period> {
    periods(store, settings, today, 1).into_iter().next()
}

/// The calendar of a month: paydays, what falls due, what you spent.
pub fn month(store: &Store, settings: &Settings, year: i32, month: u32, today: NaiveDate) -> Option<Month> {
    calendar::month(&settings.finance, &store.payments(), &[electricity(store, settings, today)], &store.spending(), year, month, today)
}

/// Whether an amount fits in what this payday has left.
pub fn afford(store: &Store, settings: &Settings, amount: Money, today: NaiveDate) -> Option<Verdict> {
    afford::afford(&settings.finance, &store.payments(), &[electricity(store, settings, today)], &store.spending(), amount, today)
}

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}
