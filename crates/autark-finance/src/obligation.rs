//! The seam between the trackers and the budget: a tracker, such as electricity, says what something will cost and
//! how sure it is, and the paycheck planner (later) takes those as line items without knowing how they were worked out.

use chrono::NaiveDate;

use crate::money::Money;
use crate::plan::{Every, Item, Kind};

/// How much to trust an amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// A fixed amount you have been told (rent, a bill that has arrived).
    Firm,
    /// Worked out from real measurements (a month's meter readings).
    Estimate,
    /// A placeholder or a few days of data; plan around it, expect it to move.
    Guess,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obligation {
    pub name: String,
    pub amount: Money,
    pub due: Option<NaiveDate>,
    pub confidence: Confidence,
}

impl Obligation {
    /// The same thing as a bill that falls due on the same day every month, for the payday planner. It starts at the
    /// coming due date, so bills already past are not charged to a payday again.
    pub fn monthly_item(&self) -> Item {
        use chrono::Datelike;
        // A bill due on a month's last day stays on the last day, whatever the month's length.
        let day = self.due.map_or(31, |due| if (due + chrono::Duration::days(1)).day() == 1 { 31 } else { due.day() });
        Item { name: self.name.clone(), kind: Kind::Bill, amount: self.amount, every: Every::Month, day: Some(day), date: self.due, ..Item::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obligation(due: Option<NaiveDate>) -> Obligation {
        Obligation { name: "Electricity".into(), amount: Money::from_pesos(1200), due, confidence: Confidence::Guess }
    }

    #[test]
    fn a_bill_due_on_a_months_last_day_stays_on_the_last_day() {
        let item = obligation(NaiveDate::from_ymd_opt(2026, 11, 30)).monthly_item();
        assert_eq!(item.day, Some(31));
        assert_eq!(item.date, NaiveDate::from_ymd_opt(2026, 11, 30));
    }

    #[test]
    fn a_bill_due_mid_month_keeps_its_day() {
        assert_eq!(obligation(NaiveDate::from_ymd_opt(2026, 11, 14)).monthly_item().day, Some(14));
    }
}
