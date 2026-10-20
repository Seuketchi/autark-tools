//! Layouts remembered per set of monitors, in `~/.config/autark/displays.toml`. Which monitors count is the
//! connected ones, less the laptop's panel while its lid is shut with another monitor plugged in. A set seen
//! for the first time gets every monitor on, each as it was last set, side by side; after that, whatever the
//! set was last changed to is what it comes back to, and is put back if the compositor strays from it
//! (Wayfire resets every output from its own config when one is plugged in).
//!
//! ```toml
//! [[layout]]
//! monitors = ["BOE NE160QDM-NY1 0x00000000", "Dell Inc. DELL U2723QE 5KC0Y83"]
//!
//! [[layout.output]]
//! id = "BOE NE160QDM-NY1 0x00000000"
//! mode = "2560x1600@165.00"
//! position = [0, 0]
//! scale = 1.5
//!
//! [[layout.output]]
//! id = "Dell Inc. DELL U2723QE 5KC0Y83"
//! enabled = false
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    /// Millihertz.
    pub refresh: i32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preferred: bool,
}

impl Mode {
    /// `2560x1600@165.00`, or `2560x1600` when the refresh doesn't matter.
    pub fn text(&self) -> String {
        format!("{}x{}@{:.2}", self.width, self.height, f64::from(self.refresh) / 1000.0)
    }

    pub fn parse(text: &str) -> Option<Mode> {
        let (size, refresh) = text.split_once('@').map_or((text, None), |(size, refresh)| (size, Some(refresh)));
        let (width, height) = size.split_once('x')?;
        let refresh = match refresh {
            Some(hertz) => (hertz.trim_end_matches("Hz").parse::<f64>().ok()? * 1000.0).round() as i32,
            None => 0,
        };
        Some(Mode { width: width.parse().ok()?, height: height.parse().ok()?, refresh, preferred: false })
    }

    /// Whether a wanted mode (refresh 0 meaning any) is this one.
    fn matches(&self, wanted: &Mode) -> bool {
        self.width == wanted.width && self.height == wanted.height && (wanted.refresh == 0 || (self.refresh - wanted.refresh).abs() <= 500)
    }
}

/// One monitor as the compositor reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    /// Millimetres.
    pub physical: (i32, i32),
    pub modes: Vec<Mode>,
    pub enabled: bool,
    pub mode: Option<Mode>,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub transform: String,
}

impl Head {
    /// Make, model and serial, which stay put when the connector changes; the connector when there's none.
    pub fn id(&self) -> String {
        let id = [self.make.as_str(), self.model.as_str(), self.serial.as_str()].iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join(" ");
        match (id.is_empty(), self.serial.is_empty()) {
            (true, _) => self.name.clone(),
            (false, true) => format!("{id} ({})", self.name),
            (false, false) => id,
        }
    }

    /// The laptop's own panel.
    pub fn internal(&self) -> bool {
        ["eDP", "LVDS", "DSI"].iter().any(|prefix| self.name.starts_with(prefix))
    }

    fn preferred(&self) -> Option<Mode> {
        self.modes.iter().find(|mode| mode.preferred).or_else(|| self.modes.first()).cloned().map(|mode| Mode { preferred: false, ..mode })
    }

    /// Width on the desktop, in logical pixels.
    fn logical_width(&self, mode: &Mode, scale: f64, transform: &str) -> i32 {
        let width = if transform.ends_with("90") || transform.ends_with("270") { mode.height } else { mode.width };
        (f64::from(width) / scale.max(0.25)).round() as i32
    }
}

/// How one monitor should be, as stored and as applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub id: String,
    #[serde(default = "yes", skip_serializing_if = "Clone::clone")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default)]
    pub position: (i32, i32),
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default = "normal")]
    pub transform: String,
}

fn yes() -> bool {
    true
}

fn one() -> f64 {
    1.0
}

fn normal() -> String {
    "normal".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub monitors: Vec<String>,
    #[serde(default, rename = "output")]
    pub outputs: Vec<Output>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Saved {
    #[serde(rename = "layout")]
    pub layouts: Vec<Layout>,
}

pub fn path() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("autark/displays.toml")
}

impl Saved {
    pub fn load() -> Self {
        autark_common::config::load("displays")
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        autark_common::files::write_atomically(&path, text.as_bytes())
    }

    pub fn find(&self, key: &[String]) -> Option<&Layout> {
        self.layouts.iter().find(|layout| sorted(&layout.monitors) == key)
    }

    /// How a monitor was last set on, in any layout, the latest first.
    fn last_known(&self, id: &str) -> Option<&Output> {
        self.layouts.iter().rev().flat_map(|layout| &layout.outputs).find(|output| output.id == id && output.enabled)
    }

    /// Stores `heads` as the layout for their set, as the latest; says whether that changed anything.
    pub fn remember(&mut self, heads: &[Head], lid_closed: bool) -> bool {
        let key = key(heads, lid_closed);
        if key.is_empty() {
            return false;
        }
        let outputs: Vec<Output> = heads.iter().filter(|head| key.contains(&head.id())).map(current).collect();
        let layout = Layout { monitors: key.clone(), outputs };
        let stored = self.layouts.iter().position(|stored| sorted(&stored.monitors) == key);
        if let Some(index) = stored
            && same(&self.layouts[index], &layout)
        {
            return false;
        }
        if let Some(index) = stored {
            self.layouts.remove(index);
        }
        self.layouts.push(layout);
        true
    }
}

fn sorted(ids: &[String]) -> Vec<String> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids
}

/// The monitors that count: all connected, less the laptop panel while the lid is shut with another plugged in.
pub fn key(heads: &[Head], lid_closed: bool) -> Vec<String> {
    let docked = heads.iter().any(|head| !head.internal());
    sorted(&heads.iter().filter(|head| !(lid_closed && docked && head.internal())).map(Head::id).collect::<Vec<_>>())
}

/// How a head is set now, as an output to store or apply.
pub fn current(head: &Head) -> Output {
    Output { id: head.id(), enabled: head.enabled, mode: head.mode.as_ref().filter(|_| head.enabled).map(Mode::text), position: (head.x, head.y), scale: head.scale, transform: head.transform.clone() }
}

fn same(a: &Layout, b: &Layout) -> bool {
    let order = |layout: &Layout| {
        let mut outputs = layout.outputs.clone();
        outputs.sort_by(|x, y| x.id.cmp(&y.id));
        outputs
    };
    let (a, b) = (order(a), order(b));
    a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|(x, y)| x.id == y.id && x.enabled == y.enabled && (!x.enabled || (x.mode == y.mode && x.position == y.position && (x.scale - y.scale).abs() < 0.001 && x.transform == y.transform)))
}

/// A head and the mode to give it, ready to apply; `None` turns it off.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub name: String,
    pub on: Option<(Mode, (i32, i32), f64, String)>,
}

/// Settings for `outputs`, matched to the heads by id or connector. Heads not mentioned stay as they are. A mode
/// the monitor lacks is refused when `strict`, or replaced by its preferred one.
pub fn settings(heads: &[Head], outputs: &[Output], strict: bool) -> Result<Vec<Setting>, String> {
    let mut settings = Vec::new();
    for head in heads {
        let Some(output) = outputs.iter().find(|output| output.id == head.id() || output.id == head.name) else {
            settings.push(Setting { name: head.name.clone(), on: current_setting(head) });
            continue;
        };
        if !output.enabled {
            settings.push(Setting { name: head.name.clone(), on: None });
            continue;
        }
        let wanted = match &output.mode {
            Some(text) => Some(Mode::parse(text).ok_or_else(|| format!("{}: {text:?} isn't a mode like 2560x1600@60.00", head.name))?),
            None => None,
        };
        let mode = match &wanted {
            Some(wanted) => {
                let found = head.modes.iter().filter(|mode| mode.matches(wanted)).max_by_key(|mode| mode.refresh).cloned().map(|mode| Mode { preferred: false, ..mode });
                if found.is_none() && strict {
                    return Err(format!("{} has no mode {}; `autark-displays list` shows its modes", head.name, output.mode.as_deref().unwrap_or_default()));
                }
                found
            }
            None => head.mode.clone().or_else(|| head.preferred()),
        }
        .or_else(|| head.preferred())
        .ok_or_else(|| format!("{} has no modes", head.name))?;
        if !(0.25..=8.0).contains(&output.scale) {
            return Err(format!("{}: scale {} is out of range", head.name, output.scale));
        }
        settings.push(Setting { name: head.name.clone(), on: Some((mode, output.position, output.scale, output.transform.clone())) });
    }
    if !settings.iter().any(|setting| setting.on.is_some()) {
        return Err("that would turn every monitor off".into());
    }
    Ok(settings)
}

fn current_setting(head: &Head) -> Option<(Mode, (i32, i32), f64, String)> {
    head.enabled.then(|| head.mode.clone()).flatten().map(|mode| (mode, (head.x, head.y), head.scale, head.transform.clone()))
}

/// What the monitors should be for the set now connected: the stored layout, or every monitor on as it was
/// last set (or as it is), side by side in their current order. Heads outside the set (a shut laptop's panel)
/// are turned off.
pub fn plan(saved: &Saved, heads: &[Head], lid_closed: bool) -> Result<Vec<Setting>, String> {
    let key = key(heads, lid_closed);
    let outside = heads.iter().filter(|head| !key.contains(&head.id())).map(|head| Output { enabled: false, ..current(head) });
    if let Some(layout) = saved.find(&key) {
        let mut outputs = layout.outputs.clone();
        outputs.extend(outside.clone());
        if let Ok(settings) = settings(heads, &outputs, false) {
            return Ok(settings);
        }
    }
    let mut inside: Vec<&Head> = heads.iter().filter(|head| key.contains(&head.id())).collect();
    inside.sort_by_key(|head| (!head.enabled, head.x, head.y));
    let mut right = 0;
    let mut outputs: Vec<Output> = outside.collect();
    for head in inside {
        let Some((output, width)) = placed(saved, head, right) else { continue };
        outputs.push(output);
        right += width;
    }
    settings(heads, &outputs, false)
}

/// A head that is to be on, as it was last set on (else as it is, else as its monitor prefers), with its left edge at `right`; and the
/// width it takes on the desktop. `None` when the monitor has no modes at all.
pub fn placed(saved: &Saved, head: &Head, right: i32) -> Option<(Output, i32)> {
    let known = saved.last_known(&head.id());
    let mode = known.and_then(|output| output.mode.as_deref()).and_then(Mode::parse).filter(|mode| head.modes.iter().any(|offered| offered.matches(mode)));
    let mode = mode.or_else(|| head.enabled.then(|| head.mode.clone()).flatten()).or_else(|| head.preferred())?;
    let scale = known.map_or(if head.scale > 0.0 { head.scale } else { 1.0 }, |output| output.scale);
    let transform = known.map_or_else(|| head.transform.clone(), |output| output.transform.clone());
    let width = head.logical_width(&mode, scale, &transform);
    Some((Output { id: head.id(), enabled: true, mode: Some(mode.text()), position: (right, 0), scale, transform }, width))
}

/// The heads as they'll be once `settings` are applied.
pub fn applied(heads: &[Head], settings: &[Setting]) -> Vec<Head> {
    let mut heads = heads.to_vec();
    for setting in settings {
        let Some(head) = heads.iter_mut().find(|head| head.name == setting.name) else { continue };
        match &setting.on {
            None => (head.enabled, head.mode) = (false, None),
            Some((mode, (x, y), scale, transform)) => {
                (head.enabled, head.mode, head.x, head.y, head.scale, head.transform) = (true, Some(mode.clone()), *x, *y, *scale, transform.clone());
            }
        }
    }
    heads
}

/// Whether applying `settings` would change anything.
pub fn differs(heads: &[Head], settings: &[Setting]) -> bool {
    settings.iter().any(|setting| {
        let Some(head) = heads.iter().find(|head| head.name == setting.name) else { return true };
        match (&setting.on, current_setting(head)) {
            (None, None) => false,
            (Some((mode, position, scale, transform)), Some((now_mode, now_position, now_scale, now_transform))) => {
                !now_mode.matches(mode) || *position != now_position || (scale - now_scale).abs() > 0.001 || *transform != now_transform
            }
            _ => true,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: i32, height: i32, hertz: i32, preferred: bool) -> Mode {
        Mode { width, height, refresh: hertz * 1000, preferred }
    }

    fn laptop() -> Head {
        Head {
            name: "eDP-1".into(),
            make: "BOE".into(),
            model: "NE160QDM-NY1".into(),
            serial: "".into(),
            modes: vec![mode(2560, 1600, 165, true), mode(2560, 1600, 60, false)],
            enabled: true,
            mode: Some(mode(2560, 1600, 165, false)),
            scale: 1.5,
            transform: "normal".into(),
            ..Head::default()
        }
    }

    fn monitor() -> Head {
        Head {
            name: "DP-2".into(),
            make: "Dell Inc.".into(),
            model: "DELL U2723QE".into(),
            serial: "5KC0Y83".into(),
            modes: vec![mode(3840, 2160, 60, true), mode(1920, 1080, 60, false)],
            enabled: false,
            mode: None,
            scale: 1.0,
            transform: "normal".into(),
            ..Head::default()
        }
    }

    #[test]
    fn ids_modes_and_the_clamshell_key() {
        assert_eq!(laptop().id(), "BOE NE160QDM-NY1 (eDP-1)");
        assert_eq!(Head { name: "HEADLESS-1".into(), ..Head::default() }.id(), "HEADLESS-1");
        assert_eq!(monitor().id(), "Dell Inc. DELL U2723QE 5KC0Y83");
        assert_eq!(Mode::parse("2560x1600@165.00"), Some(mode(2560, 1600, 165, false)));
        assert_eq!(Mode::parse("1920x1080").map(|mode| mode.refresh), Some(0));
        assert_eq!(mode(2560, 1600, 165, false).text(), "2560x1600@165.00");
        let heads = [laptop(), monitor()];
        assert_eq!(key(&heads, false).len(), 2);
        assert_eq!(key(&heads, true), vec![monitor().id()]);
        // Shut with nothing else plugged in: the panel still counts.
        assert_eq!(key(&[laptop()], true), vec![laptop().id()]);
    }

    #[test]
    fn a_new_set_turns_everything_on_with_newcomers_to_the_right() {
        let heads = [laptop(), monitor()];
        let plan = plan(&Saved::default(), &heads, false).unwrap();
        assert_eq!(plan[0].on.as_ref().map(|on| on.1), Some((0, 0)));
        // 2560 / 1.5 = 1707 logical pixels wide.
        assert_eq!(plan[1].on.as_ref().map(|on| (on.0.width, on.1)), Some((3840, (1707, 0))));
        assert!(differs(&heads, &plan));
    }

    #[test]
    fn a_new_set_keeps_each_monitor_as_it_was_last_set() {
        // The laptop alone was set to 60 Hz at 2x; the compositor reset it on plugging in a monitor.
        let mut saved = Saved::default();
        let mut alone = laptop();
        alone.mode = Some(mode(2560, 1600, 60, false));
        alone.scale = 2.0;
        saved.remember(&[alone], false);
        let mut plugged = [laptop(), monitor()];
        plugged[0].scale = 1.0;
        let plan = plan(&saved, &plugged, false).unwrap();
        assert_eq!(plan[0].on.as_ref().map(|on| (on.0.refresh, on.2)), Some((60_000, 2.0)));
        // 2560 / 2 = 1280 logical pixels, so the monitor starts there.
        assert_eq!(plan[1].on.as_ref().map(|on| on.1), Some((1280, 0)));
    }

    #[test]
    fn closing_the_lid_turns_the_panel_off_and_opening_restores_it() {
        let mut docked = [laptop(), monitor()];
        docked[1].enabled = true;
        docked[1].mode = Some(mode(3840, 2160, 60, false));
        docked[1].x = 1707;
        let mut saved = Saved::default();
        assert!(saved.remember(&docked, false));
        assert!(!saved.remember(&docked, false), "remembering the same thing again changes nothing");
        let shut = plan(&saved, &docked, true).unwrap();
        assert!(!differs(&applied(&docked, &shut), &shut));
        assert_eq!(shut[0].on, None);
        assert!(shut[1].on.is_some());
        // Lid shut: the panel is off, and the monitor alone is remembered as its own set.
        let mut clamshell = docked.clone();
        clamshell[0].enabled = false;
        clamshell[0].mode = None;
        assert!(saved.remember(&clamshell, true));
        assert_eq!(saved.layouts.len(), 2);
        // Lid open again: the two-monitor layout comes back.
        let open = plan(&saved, &clamshell, false).unwrap();
        assert_eq!(open[0].on.as_ref().map(|on| (on.0.refresh, on.2)), Some((165_000, 1.5)));
        assert!(differs(&clamshell, &open));
    }

    #[test]
    fn stored_layouts_come_back_and_bad_ones_are_refused() {
        let heads = [laptop()];
        let mut saved = Saved::default();
        let mut changed = laptop();
        changed.mode = Some(mode(2560, 1600, 60, false));
        changed.scale = 2.0;
        saved.remember(&[changed], false);
        let text = toml::to_string(&saved).unwrap();
        assert_eq!(toml::from_str::<Saved>(&text).unwrap(), saved);
        let plan = plan(&saved, &heads, false).unwrap();
        assert_eq!(plan[0].on.as_ref().map(|on| (on.0.refresh, on.2)), Some((60_000, 2.0)));
        let off = Output { id: laptop().id(), enabled: false, mode: None, position: (0, 0), scale: 1.0, transform: "normal".into() };
        assert!(settings(&heads, &[off], true).is_err());
        let bad = Output { id: "eDP-1".into(), enabled: true, mode: Some("big".into()), position: (0, 0), scale: 1.0, transform: "normal".into() };
        assert!(settings(&heads, &[bad], true).is_err());
        let missing = Output { id: "eDP-1".into(), enabled: true, mode: Some("1024x768".into()), position: (0, 0), scale: 1.0, transform: "normal".into() };
        assert!(settings(&heads, std::slice::from_ref(&missing), true).is_err());
        assert!(settings(&heads, &[missing], false).is_ok());
    }
}
