//! `budget.toml`: the settings of every tracker, one table each. Electricity's is `[electric]`.
//! Loaded the way the other tools' config is (your file, then `/etc/autark`), and saved to your own.

use std::io;

use autark_finance::electric;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub electric: electric::Config,
    pub finance: autark_finance::plan::Config,
}

impl Settings {
    pub fn load() -> Settings {
        autark_common::config::load("budget")
    }

    /// Writes your own `budget.toml`. Comments you added by hand are not kept.
    pub fn save(&self) -> io::Result<()> {
        let path = autark_common::config::paths("budget").into_iter().next().ok_or_else(|| io::Error::other("no config folder"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("toml.new");
        std::fs::write(&temporary, text)?;
        std::fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use autark_finance::electric::Appliance;
    use autark_finance::money::Money;

    use super::*;

    #[test]
    fn settings_round_trip_through_toml() {
        let mut settings = Settings::default();
        settings.electric.budget = Money::from_centavos(150_050);
        settings.electric.billing_day = 15;
        settings.electric.appliances.push(Appliance { name: "Fridge".into(), watts: 100.0, hours_per_day: 10.0 });
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn a_file_with_only_some_keys_keeps_the_defaults_for_the_rest() {
        let settings: Settings = toml::from_str("[electric]\nbilling_day = 20\n").unwrap();
        assert_eq!(settings.electric.billing_day, 20);
        assert_eq!(settings.electric.budget, electric::Config::default().budget);
        assert_eq!(toml::from_str::<Settings>("").unwrap(), Settings::default());
    }
}
