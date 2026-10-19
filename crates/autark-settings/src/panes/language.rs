//! Language & Region: the system language (`LANG`), from the locales the system has generated. Changing it is
//! `localectl`'s, which asks polkit; when that says no, the pane shows what it said. The keyboard layouts are on the
//! Keyboard pane.

use std::process::Command;

use gtk::prelude::*;

use crate::ui;

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Language & Region", Some("Applies to apps you open after the change, and after you log in again.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);

    let current = current_locale();
    let mut locales = installed();
    if !locales.contains(&current) && !current.is_empty() {
        locales.insert(0, current.clone());
    }
    let labels: Vec<String> = locales.iter().map(|locale| label(locale)).collect();
    let options: Vec<(&str, &str)> = locales.iter().zip(&labels).map(|(locale, label)| (locale.as_str(), label.as_str())).collect();

    let group = ui::group();
    let language = ui::choice(&options, &current, {
        let (status, current) = (status.clone(), std::cell::RefCell::new(current.clone()));
        move |locale| {
            if *current.borrow() == locale {
                return;
            }
            let output = Command::new("localectl").arg("set-locale").arg(format!("LANG={locale}")).output();
            let result = match output {
                Ok(output) if output.status.success() => Ok(()),
                Ok(output) => Err(format!("{} (changing this needs permission this session doesn't have)", String::from_utf8_lossy(&output.stderr).trim())),
                Err(error) => Err(format!("localectl: {error}")),
            };
            if result.is_ok() {
                *current.borrow_mut() = locale.to_owned();
            }
            status.set_visible(result.is_err());
            if let Err(error) = result {
                status.set_label(&error);
            }
        }
    });
    group.append(&ui::row("Language and formats", Some("Dates, numbers and messages"), &language));
    column.append(&ui::section("Language", None, &group));
    column.append(&ui::note("Only languages the system has generated are listed. Adding one means generating its locale as root."));
    column.append(&status);
    page.upcast()
}

fn current_locale() -> String {
    let output = Command::new("localectl").arg("status").output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    output.lines().find_map(|line| line.split_once("LANG=").map(|(_, locale)| locale.split_whitespace().next().unwrap_or("").to_owned())).unwrap_or_default()
}

/// The UTF-8 locales `locale -a` knows, as `LANG` writes them.
fn installed() -> Vec<String> {
    let output = Command::new("locale").arg("-a").output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    let mut found: Vec<String> = output
        .lines()
        .filter(|line| line.ends_with(".utf8") || line.ends_with(".UTF-8"))
        .map(|line| line.replace(".utf8", ".UTF-8"))
        .filter(|line| line != "C.UTF-8")
        .collect();
    found.sort();
    found.dedup();
    found
}

/// "en_US.UTF-8" as "English (United States)", for the ones people are likely to look for, else as written.
fn label(locale: &str) -> String {
    let (code, _) = locale.split_once('.').unwrap_or((locale, ""));
    let (language, region) = code.split_once('_').unwrap_or((code, ""));
    let language = match language {
        "en" => "English", "de" => "Deutsch", "fr" => "Français", "es" => "Español", "it" => "Italiano", "pt" => "Português", "nl" => "Nederlands",
        "pl" => "Polski", "cs" => "Čeština", "el" => "Ελληνικά", "ru" => "Русский", "ja" => "日本語", "zh" => "中文", "ko" => "한국어", "tl" | "fil" => "Filipino",
        other => other,
    };
    if region.is_empty() { format!("{language} · {locale}") } else { format!("{language} ({region}) · {locale}") }
}
