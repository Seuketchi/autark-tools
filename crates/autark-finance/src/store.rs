//! Readings and bills on disk: one JSON object per line, appended as you go. A crash cannot damage what is already
//! there, the files read in any editor, and one bad line is skipped rather than losing the rest.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::electric::{Bill, Reading};
use crate::payday::Payment;
use crate::spending::Spend;

#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    /// `$XDG_DATA_HOME/autark-budget`, or `~/.local/share/autark-budget`.
    pub fn default_dir() -> PathBuf {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("autark-budget")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn readings_path(&self) -> PathBuf {
        self.dir.join("readings.jsonl")
    }

    fn bills_path(&self) -> PathBuf {
        self.dir.join("bills.jsonl")
    }

    fn payments_path(&self) -> PathBuf {
        self.dir.join("payments.jsonl")
    }

    fn spending_path(&self) -> PathBuf {
        self.dir.join("spending.jsonl")
    }

    pub fn payments(&self) -> Vec<Payment> {
        read_lines(&self.payments_path())
    }

    pub fn add_payment(&self, payment: &Payment) -> io::Result<()> {
        append(&self.payments_path(), payment)
    }

    /// Replaces every payment, for taking one back.
    pub fn set_payments(&self, all: &[Payment]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        rewrite(&self.payments_path(), all)
    }

    pub fn spending(&self) -> Vec<Spend> {
        read_lines(&self.spending_path())
    }

    pub fn add_spending(&self, spend: &Spend) -> io::Result<()> {
        append(&self.spending_path(), spend)
    }

    /// Replaces everything spent, for deleting one.
    pub fn set_spending(&self, all: &[Spend]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        rewrite(&self.spending_path(), all)
    }

    pub fn readings(&self) -> Vec<Reading> {
        read_lines(&self.readings_path())
    }

    pub fn bills(&self) -> Vec<Bill> {
        read_lines(&self.bills_path())
    }

    pub fn add_reading(&self, reading: &Reading) -> io::Result<()> {
        append(&self.readings_path(), reading)
    }

    pub fn add_bill(&self, bill: &Bill) -> io::Result<()> {
        append(&self.bills_path(), bill)
    }

    /// Replaces every reading, for editing or deleting one in the middle of the history.
    pub fn set_readings(&self, all: &[Reading]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        rewrite(&self.readings_path(), all)
    }

    /// Replaces every bill, for deleting one.
    pub fn set_bills(&self, all: &[Bill]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        rewrite(&self.bills_path(), all)
    }

    /// Takes the newest line back out (the one added last, whatever its date), for a reading typed wrong.
    pub fn remove_last_reading(&self) -> io::Result<Option<Reading>> {
        let mut all: Vec<Reading> = self.readings();
        let Some(last) = all.pop() else { return Ok(None) };
        rewrite(&self.readings_path(), &all)?;
        Ok(Some(last))
    }
}

fn read_lines<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    let Ok(text) = fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .filter_map(|(number, line)| match serde_json::from_str(line) {
            Ok(item) => Some(item),
            Err(error) => {
                eprintln!("{}:{}: skipped, {error}", path.display(), number + 1);
                None
            }
        })
        .collect()
}

fn append<T: Serialize>(path: &Path, item: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_string(item).map_err(io::Error::other)?;
    line.push('\n');
    OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
}

/// Replaces the file by writing a sibling and renaming it over, so it is never half written.
fn rewrite<T: Serialize>(path: &Path, items: &[T]) -> io::Result<()> {
    let mut text = String::new();
    for item in items {
        text.push_str(&serde_json::to_string(item).map_err(io::Error::other)?);
        text.push('\n');
    }
    let temporary = path.with_extension("jsonl.new");
    fs::write(&temporary, text)?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::money::Money;

    /// A folder of its own for one test, removed afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("autark-finance-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn day(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    #[test]
    fn readings_and_bills_come_back_as_they_went_in() {
        let scratch = Scratch::new("roundtrip");
        let store = Store::open(scratch.0.join("deeper"));
        assert!(store.readings().is_empty(), "nothing yet is fine, not an error");
        store.add_reading(&Reading { date: day(1), kwh: 15.4, note: "opening".into() }).unwrap();
        store.add_reading(&Reading { date: day(4), kwh: 18.4, note: String::new() }).unwrap();
        let bill = Bill { from: day(1), to: day(30), kwh: 80.0, amount: Money::from_centavos(96_050) };
        store.add_bill(&bill).unwrap();
        let readings = store.readings();
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].note, "opening");
        assert_eq!(readings[1].kwh, 18.4);
        assert_eq!(store.bills(), vec![bill]);
    }

    #[test]
    fn a_bad_line_is_skipped_and_the_rest_kept() {
        let scratch = Scratch::new("badline");
        let store = Store::open(&scratch.0);
        store.add_reading(&Reading { date: day(1), kwh: 1.0, note: String::new() }).unwrap();
        let mut file = OpenOptions::new().append(true).open(scratch.0.join("readings.jsonl")).unwrap();
        file.write_all(b"not json\n\n").unwrap();
        drop(file);
        store.add_reading(&Reading { date: day(2), kwh: 2.0, note: String::new() }).unwrap();
        assert_eq!(store.readings().len(), 2);
    }

    #[test]
    fn replacing_the_history_keeps_what_you_give_it() {
        let scratch = Scratch::new("replace");
        let store = Store::open(scratch.0.join("new-folder"));
        let all: Vec<Reading> = (1..=3).map(|d| Reading { date: day(d), kwh: f64::from(d), note: String::new() }).collect();
        store.set_readings(&all).unwrap();
        assert_eq!(store.readings(), all);
        store.set_readings(&[all[0].clone(), all[2].clone()]).unwrap();
        assert_eq!(store.readings().iter().map(|r| r.kwh).collect::<Vec<_>>(), [1.0, 3.0]);
        store.set_readings(&[]).unwrap();
        assert!(store.readings().is_empty());
        let bill = Bill { from: day(1), to: day(30), kwh: 80.0, amount: Money::from_pesos(960) };
        store.set_bills(std::slice::from_ref(&bill)).unwrap();
        assert_eq!(store.bills(), vec![bill]);
    }

    #[test]
    fn payments_and_spending_are_kept_and_replaced_like_readings() {
        let scratch = Scratch::new("finance-files");
        let store = Store::open(&scratch.0);
        assert!(store.payments().is_empty() && store.spending().is_empty());
        let payment = Payment { item: "Rent".into(), period: day(30), date: day(30), amount: Money::from_centavos(400_050) };
        store.add_payment(&payment).unwrap();
        assert_eq!(store.payments(), vec![payment]);
        store.set_payments(&[]).unwrap();
        assert!(store.payments().is_empty());
        let spend = Spend { date: day(2), amount: Money::from_pesos(85), category: "Food".into(), note: String::new() };
        store.add_spending(&spend).unwrap();
        store.add_spending(&Spend { note: "jeep".into(), category: String::new(), ..spend.clone() }).unwrap();
        assert_eq!(store.spending().len(), 2);
        assert_eq!(store.spending()[1].note, "jeep");
        store.set_spending(&store.spending()[..1]).unwrap();
        assert_eq!(store.spending(), vec![spend]);
    }

    #[test]
    fn undo_takes_back_only_the_last_one() {
        let scratch = Scratch::new("undo");
        let store = Store::open(&scratch.0);
        assert_eq!(store.remove_last_reading().unwrap(), None);
        for (date, kwh) in [(day(1), 1.0), (day(2), 2.0)] {
            store.add_reading(&Reading { date, kwh, note: String::new() }).unwrap();
        }
        let removed = store.remove_last_reading().unwrap().unwrap();
        assert_eq!(removed.kwh, 2.0);
        assert_eq!(store.readings().len(), 1);
    }
}
