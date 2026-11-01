//! Budget: electricity first, with the rest of your money to follow. With no arguments it opens the window;
//! `autark-budget electric …` does the same from the terminal (see `cli`).

mod backup;
mod bars;
mod calendar_pane;
mod cli;
mod electric_pane;
mod mascot;
mod settings;
mod text;
mod paychecks_pane;
mod plan_pane;
mod remind;
mod spending_pane;
mod ui;
mod view;
mod window;

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return open_window();
    }
    if matches!(args[0].as_str(), "-h" | "--help" | "help") {
        println!("{}", cli::USAGE);
        return glib::ExitCode::SUCCESS;
    }
    let store = autark_finance::store::Store::open(autark_finance::store::Store::default_dir());
    let mut settings = settings::Settings::load();
    let today = chrono::Local::now().date_naive();
    match args[0].as_str() {
        "backup" | "restore" => return finish(backup_or_restore(&args, &store, today)),
        "remind" => return finish(remind(&args[1..], &store, &settings, today)),
        _ => {}
    }
    match cli::run(&args, &store, &mut settings, today) {
        Ok(outcome) => {
            println!("{}", outcome.text);
            if outcome.settings_changed
                && let Err(error) = settings.save()
            {
                eprintln!("could not save your settings: {error}");
                return glib::ExitCode::FAILURE;
            }
            glib::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            glib::ExitCode::from(2)
        }
    }
}

fn finish(result: Result<String, String>) -> glib::ExitCode {
    match result {
        Ok(text) => {
            println!("{text}");
            glib::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            glib::ExitCode::from(2)
        }
    }
}

/// The settings file these commands copy: your own `budget.toml`.
fn settings_path() -> Result<std::path::PathBuf, String> {
    autark_common::config::paths("budget").into_iter().next().ok_or_else(|| "no config folder".to_string())
}

fn backup_or_restore(args: &[String], store: &autark_finance::store::Store, today: chrono::NaiveDate) -> Result<String, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    backup::command(&args, store.dir(), &settings_path()?, today)
}

/// `remind`: say the payday and bills-due reminder, if there is one. `--always` says the payday summary on any day.
fn remind(args: &[String], store: &autark_finance::store::Store, settings: &settings::Settings, today: chrono::NaiveDate) -> Result<String, String> {
    let always = args.iter().any(|a| a == "--always");
    if let Some(unknown) = args.iter().find(|a| *a != "--always") {
        return Err(format!("remind takes only --always, not “{unknown}”"));
    }
    let Some(period) = view::current(store, settings, today) else { return Ok("No paydays are set, so nothing to remind.".into()) };
    match remind::reminder(&period, today, always) {
        Some(reminder) => {
            remind::send(&reminder)?;
            Ok(format!("{}\n{}", reminder.title, reminder.body))
        }
        None => Ok("Nothing to remind today.".into()),
    }
}

fn open_window() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Budget").build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("budget.css")));
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let store = autark_finance::store::Store::open(autark_finance::store::Store::default_dir());
        window::Budget::new(app, store, settings::Settings::load()).window.present();
    });
    // The arguments are ours, not GTK's.
    app.run_with_args::<&str>(&[])
}
