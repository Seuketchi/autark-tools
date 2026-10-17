//! Weather from Open-Meteo (no account), off until you set a location and switch it on, since
//! every update sends that location. Fetched when the centre opens, at most every half hour.

use gtk::prelude::*;
use serde::Deserialize;

use crate::config::Location;
use crate::shared::Shared;

#[derive(Clone)]
pub struct Weather {
    pub place: Option<String>,
    pub temperature: f64,
    pub code: u32,
    pub high: f64,
    pub low: f64,
}

#[derive(Deserialize)]
struct Answer {
    current: Current,
    daily: Daily,
}

#[derive(Deserialize)]
struct Current {
    temperature_2m: f64,
    weather_code: u32,
}

#[derive(Deserialize)]
struct Daily {
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
}

/// Blocks: run it off the main thread.
pub fn fetch(location: &Location) -> Result<Weather, String> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,weather_code&daily=temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=1",
        location.latitude, location.longitude
    );
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(15))).build().into();
    let text = agent.get(&url).call().map_err(|err| err.to_string())?.body_mut().read_to_string().map_err(|err| err.to_string())?;
    let answer: Answer = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    Ok(Weather {
        place: location.place.clone(),
        temperature: answer.current.temperature_2m,
        code: answer.current.weather_code,
        high: answer.daily.temperature_2m_max.first().copied().unwrap_or(f64::NAN),
        low: answer.daily.temperature_2m_min.first().copied().unwrap_or(f64::NAN),
    })
}

/// The WMO weather codes Open-Meteo reports, in words.
fn condition(code: u32) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51..=57 => "Drizzle",
        61..=67 => "Rain",
        71..=77 => "Snow",
        80..=82 => "Showers",
        85 | 86 => "Snow showers",
        95..=99 => "Thunderstorm",
        _ => "",
    }
}

pub fn view(weather: &Shared<Option<Weather>>) -> gtk::Box {
    let place = gtk::Label::builder().xalign(0.0).build();
    place.add_css_class("caption");
    let temperature = gtk::Label::builder().xalign(0.0).build();
    temperature.add_css_class("temperature");
    let left = gtk::Box::new(gtk::Orientation::Vertical, 2);
    left.set_hexpand(true);
    left.append(&place);
    left.append(&temperature);
    let condition_label = gtk::Label::builder().xalign(1.0).build();
    condition_label.add_css_class("name");
    let range = gtk::Label::builder().xalign(1.0).build();
    range.add_css_class("detail");
    let right = gtk::Box::new(gtk::Orientation::Vertical, 2);
    right.set_valign(gtk::Align::End);
    right.append(&condition_label);
    right.append(&range);

    let card = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    card.add_css_class("card");
    card.add_css_class("widget");
    card.append(&left);
    card.append(&right);
    weather.watch(&card, move |card, weather| {
        card.set_visible(weather.is_some());
        let Some(weather) = weather else { return };
        place.set_label(&weather.place.as_deref().unwrap_or("WEATHER").to_uppercase());
        temperature.set_label(&format!("{:.0}°", weather.temperature));
        condition_label.set_label(condition(weather.code));
        range.set_label(&format!("H {:.0}°  L {:.0}°", weather.high, weather.low));
    });
    card
}
