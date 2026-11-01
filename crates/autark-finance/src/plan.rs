//! The plan: what you earn each payday and what falls due, as a list of items. An item is a bill (rent), a goal
//! (the fridge fund), a debt (a loan from family) or an installment (a pay-later purchase); each says how much, and when it comes up.
//! This is the `[finance]` table of `budget.toml`, written to be read and edited by hand.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::money::Money;

/// What an item is, for grouping and for how it behaves: a goal stops once it is full.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Bill,
    Goal,
    Debt,
    Installment,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Bill, Kind::Goal, Kind::Debt, Kind::Installment];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Bill => "bill",
            Kind::Goal => "goal",
            Kind::Debt => "debt",
            Kind::Installment => "installment",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name().eq_ignore_ascii_case(text.trim()))
    }
}

/// How often an item comes up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Every {
    /// Out of every paycheck.
    Payday,
    /// On `day` of every month (31 is the last day, whatever the month).
    #[default]
    Month,
    /// Once, on `date`.
    Once,
}

impl Every {
    pub fn parse(text: &str) -> Option<Every> {
        match text.trim().to_ascii_lowercase().as_str() {
            "payday" => Some(Every::Payday),
            "month" | "monthly" => Some(Every::Month),
            "once" => Some(Every::Once),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    pub name: String,
    pub kind: Kind,
    /// Each time it comes up. For a goal, what you put in each payday.
    pub amount: Money,
    pub every: Every,
    /// The day of the month, for `every = "month"`. 31 means the last day.
    pub day: Option<u32>,
    /// When it happens (`once`), or when it starts (the others: nothing before this date counts, and `times` counts from it).
    pub date: Option<NaiveDate>,
    /// Stops after this many times, counting from `date`: an installment's term.
    pub times: Option<u32>,
    /// A goal's full amount; it stops asking once it is reached.
    pub target: Option<Money>,
    /// A goal's savings before the plan began.
    pub saved: Money,
}

impl Default for Item {
    fn default() -> Self {
        Item { name: String::new(), kind: Kind::Bill, amount: Money::ZERO, every: Every::Month, day: None, date: None, times: None, target: None, saved: Money::ZERO }
    }
}

impl Item {
    /// Says what is wrong with an item, in words for the person who typed it.
    pub fn check(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Give it a name".into());
        }
        if self.amount.centavos() <= 0 {
            return Err("The amount has to be more than ₱0".into());
        }
        match self.every {
            Every::Month if !self.day.is_some_and(|day| (1..=31).contains(&day)) => return Err("A monthly item needs a day of the month, 1 to 31 (31 is the last day)".into()),
            Every::Once if self.date.is_none() => return Err("A one-time item needs the date it is due".into()),
            _ => {}
        }
        if self.kind == Kind::Goal {
            match self.target {
                Some(target) if target > self.saved => {}
                Some(_) => return Err("The goal's target has to be more than what is already saved".into()),
                None => return Err("A goal needs a target amount".into()),
            }
        }
        if self.times == Some(0) {
            return Err("It has to happen at least once".into());
        }
        if self.times.is_some() && self.date.is_none() && self.every != Every::Once {
            return Err("An item that stops after some times needs a start date, to count from".into());
        }
        Ok(())
    }

    /// In a few words, when it comes up.
    pub fn schedule(&self) -> String {
        match self.every {
            Every::Payday => "every payday".to_string(),
            Every::Month => match self.day {
                Some(31) | None => "last day of the month".to_string(),
                Some(day) => format!("on the {day}{} of the month", ordinal(day)),
            },
            Every::Once => self.date.map_or("once".to_string(), |date| format!("once, {}", date.format("%-d %b %Y"))),
        }
    }
}

fn ordinal(day: u32) -> &'static str {
    match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

/// The `[finance]` table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The days of the month you are paid. A day past the end of a short month is that month's last day.
    pub pay_days: Vec<u32>,
    /// What each payday brings.
    pub paycheck: Money,
    /// The buttons of the spending log, besides the Other that is always there.
    pub categories: Vec<String>,
    #[serde(rename = "item")]
    pub items: Vec<Item>,
}

impl Default for Config {
    fn default() -> Self {
        Config { pay_days: vec![15, 30], paycheck: Money::ZERO, categories: vec!["Food".into(), "Transport".into(), "Load".into()], items: Vec::new() }
    }
}

impl Config {
    pub fn find(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|item| item.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Adds an item, replacing one of the same name.
    pub fn put(&mut self, item: Item) -> Result<(), String> {
        item.check()?;
        match self.find(&item.name) {
            Some(at) => self.items[at] = item,
            None => self.items.push(item),
        }
        Ok(())
    }

    /// Sets the spending categories from a list typed with commas. "Other" is always there, so it is left out.
    pub fn set_categories(&mut self, text: &str) -> Result<(), String> {
        let mut names: Vec<String> = Vec::new();
        for name in text.split(',').map(str::trim).filter(|name| !name.is_empty()) {
            if name.chars().count() > 20 {
                return Err(format!("“{name}” is too long for a button (20 letters at most)"));
            }
            if !name.eq_ignore_ascii_case("other") && !names.iter().any(|known| known.eq_ignore_ascii_case(name)) {
                names.push(name.to_string());
            }
        }
        if names.len() > 8 {
            return Err("Eight categories at most, so the buttons fit".into());
        }
        self.categories = names;
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Option<Item> {
        self.find(name).map(|at| self.items.remove(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rent() -> Item {
        Item { name: "Rent".into(), amount: Money::from_pesos(4000), every: Every::Month, day: Some(31), ..Item::default() }
    }

    #[test]
    fn a_good_item_passes_and_a_bad_one_says_why() {
        assert!(rent().check().is_ok());
        for (item, why) in [
            (Item { name: " ".into(), ..rent() }, "name"),
            (Item { amount: Money::ZERO, ..rent() }, "more than"),
            (Item { day: None, ..rent() }, "day of the month"),
            (Item { day: Some(32), ..rent() }, "day of the month"),
            (Item { every: Every::Once, ..rent() }, "date"),
            (Item { kind: Kind::Goal, ..rent() }, "target"),
            (Item { kind: Kind::Goal, target: Some(Money::from_pesos(100)), saved: Money::from_pesos(100), ..rent() }, "more than what is already saved"),
            (Item { times: Some(0), ..rent() }, "at least once"),
            (Item { times: Some(3), ..rent() }, "start date"),
        ] {
            let error = item.check().unwrap_err();
            assert!(error.contains(why), "{item:?}: {error}");
        }
    }

    #[test]
    fn an_item_says_when_it_comes_up() {
        assert_eq!(rent().schedule(), "last day of the month");
        assert_eq!(Item { day: Some(15), ..rent() }.schedule(), "on the 15th of the month");
        assert_eq!(Item { day: Some(1), ..rent() }.schedule(), "on the 1st of the month");
        assert_eq!(Item { day: Some(22), ..rent() }.schedule(), "on the 22nd of the month");
        assert_eq!(Item { day: Some(13), ..rent() }.schedule(), "on the 13th of the month");
        assert_eq!(Item { every: Every::Payday, ..rent() }.schedule(), "every payday");
    }

    #[test]
    fn putting_an_item_with_the_same_name_replaces_it() {
        let mut config = Config::default();
        config.put(rent()).unwrap();
        config.put(Item { name: "rent".into(), amount: Money::from_pesos(4500), ..rent() }).unwrap();
        assert_eq!(config.items.len(), 1);
        assert_eq!(config.items[0].amount, Money::from_pesos(4500));
        assert!(config.put(Item { amount: Money::ZERO, ..rent() }).is_err());
        assert_eq!(config.remove("RENT").unwrap().amount, Money::from_pesos(4500));
        assert!(config.remove("rent").is_none());
    }

    #[test]
    fn the_plan_reads_and_writes_as_toml_a_person_can_edit() {
        let mut config = Config { paycheck: Money::from_centavos(1_200_000), ..Config::default() };
        config.put(rent()).unwrap();
        config
            .put(Item { name: "Fridge".into(), kind: Kind::Goal, amount: Money::from_pesos(5000), every: Every::Payday, target: Some(Money::from_pesos(25_000)), saved: Money::from_pesos(12_000), ..Item::default() })
            .unwrap();
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("paycheck = 12000.0") && text.contains("kind = \"goal\"") && text.contains("every = \"payday\""), "{text}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
        // Only what matters has to be written.
        let by_hand: Config = toml::from_str("paycheck = 100\n[[item]]\nname = \"Internet\"\namount = 1599\nday = 15\n").unwrap();
        assert_eq!(by_hand.items[0].every, Every::Month);
        assert_eq!(by_hand.pay_days, vec![15, 30]);
    }

    #[test]
    fn categories_are_trimmed_deduplicated_and_never_include_other() {
        let mut config = Config::default();
        config.set_categories(" Food, food ,Pets, other,, Load ").unwrap();
        assert_eq!(config.categories, ["Food", "Pets", "Load"]);
        assert!(config.set_categories("a,b,c,d,e,f,g,h,i").is_err());
        assert!(config.set_categories("an extremely long category name").is_err());
        assert_eq!(config.categories, ["Food", "Pets", "Load"], "a refused list changes nothing");
        config.set_categories("").unwrap();
        assert!(config.categories.is_empty());
    }

    #[test]
    fn a_settings_file_without_categories_gets_the_usual_ones() {
        let config: Config = toml::from_str("paycheck = 100.0\n").unwrap();
        assert_eq!(config.categories, ["Food", "Transport", "Load"]);
    }
}
