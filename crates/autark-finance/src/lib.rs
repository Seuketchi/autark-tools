//! The logic behind Autark's budget tools, with no window and no network, so all of it is tested on its own.
//!
//! - [`money`]: amounts of pesos, kept as whole centavos so sums never drift.
//! - [`electric`]: meter readings, bills, the billing cycle and the planner that projects the month's bill.
//! - [`obligation`]: the seam between trackers and the budget: a tracker says "this will cost about so much".
//! - [`recommend`]: tips for your schedule, each one triggered by a threshold you can change.
//! - [`trends`]: use day by day and week by week, and by weekday, for fitting your schedule to your budget.
//! - [`plan`]: what you earn and what you owe: bills, goals, debts and installments, and when each falls due.
//! - [`payday`]: the plan laid out payday by payday, with what is left to spend after everything due.
//! - [`spending`]: what you actually spent, set against what a payday leaves.
//! - [`calendar`]: the plan on a month's calendar, with paydays, what falls due and what you spent.
//! - [`afford`]: "can I afford this?", an amount set against what the payday has left.
//! - [`store`]: readings and bills as append-only JSON lines, one folder per user.

pub mod afford;
pub mod calendar;
pub mod electric;
pub mod money;
pub mod obligation;
pub mod payday;
pub mod plan;
pub mod recommend;
pub mod spending;
pub mod store;
pub mod trends;
