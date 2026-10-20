//! The projection key (Fn+F11 on this laptop, which sends Super+P): one press goes to the next way of using the screens, round and
//! round: the laptop's own panel only, the panel and the other screens side by side, the other screens only. A screen that comes
//! back on is set as it was last set on (its mode, scale and rotation), so a 4K monitor doesn't return tiny.

use crate::layout::{self, Head, Output, Saved};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// The laptop's panel only.
    BuiltIn,
    /// The panel and the others, side by side.
    Extend,
    /// The other screens only.
    External,
}

impl Projection {
    fn after(self) -> Self {
        match self {
            Self::BuiltIn => Self::Extend,
            Self::Extend => Self::External,
            Self::External => Self::BuiltIn,
        }
    }

    /// What to tell the person, for `others` screens besides the panel.
    pub fn title(self, others: usize) -> &'static str {
        match (self, others > 1) {
            (Self::BuiltIn, _) => "Laptop screen only",
            (Self::Extend, false) => "Both screens",
            (Self::Extend, true) => "All screens",
            (Self::External, false) => "Other screen only",
            (Self::External, true) => "Other screens only",
        }
    }
}

/// How the screens are used now; `None` when none is on.
pub fn current(heads: &[Head]) -> Option<Projection> {
    let on = |internal: bool| heads.iter().any(|head| head.enabled && head.internal() == internal);
    match (on(true), on(false)) {
        (true, false) => Some(Projection::BuiltIn),
        (true, true) => Some(Projection::Extend),
        (false, true) => Some(Projection::External),
        (false, false) => None,
    }
}

/// The next way of using the screens and the outputs that make it so. The panel comes first (left), the others after it by connector,
/// so pressing again doesn't shuffle them.
pub fn next(heads: &[Head], saved: &Saved) -> Result<(Projection, Vec<Output>), String> {
    let others = heads.iter().filter(|head| !head.internal()).count();
    if others == 0 {
        return Err("no other screen is connected".into());
    }
    if !heads.iter().any(Head::internal) {
        return Err("this computer has no built-in screen to switch between".into());
    }
    let target = current(heads).map_or(Projection::Extend, Projection::after);
    let mut order: Vec<&Head> = heads.iter().collect();
    order.sort_by_key(|head| (!head.internal(), head.name.clone()));
    let mut right = 0;
    let mut outputs = Vec::new();
    for head in order {
        let wanted = match target {
            Projection::BuiltIn => head.internal(),
            Projection::Extend => true,
            Projection::External => !head.internal(),
        };
        if !wanted {
            outputs.push(Output { enabled: false, ..layout::current(head) });
            continue;
        }
        let (output, width) = layout::placed(saved, head, right).ok_or_else(|| format!("{} has no modes", head.name))?;
        outputs.push(output);
        right += width;
    }
    Ok((target, outputs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Layout, Mode};

    fn mode(width: i32, height: i32, refresh: i32) -> Mode {
        Mode { width, height, refresh, preferred: true }
    }

    fn head(name: &str, mode: Mode, on: bool, scale: f64) -> Head {
        Head {
            name: name.into(),
            make: "Acme".into(),
            model: name.into(),
            modes: vec![mode.clone()],
            enabled: on,
            mode: on.then_some(mode),
            scale,
            transform: "normal".into(),
            ..Head::default()
        }
    }

    fn panel(on: bool) -> Head {
        head("eDP-1", mode(2560, 1600, 165000), on, 1.6)
    }

    fn monitor(on: bool) -> Head {
        head("HDMI-A-1", mode(1920, 1080, 60000), on, 1.0)
    }

    fn put(outputs: &[Output], name: &str) -> Output {
        outputs.iter().find(|output| output.id.contains(name)).cloned().unwrap()
    }

    #[test]
    fn it_goes_round_the_three_ways() {
        let saved = Saved::default();
        let (way, outputs) = next(&[panel(true), monitor(false)], &saved).unwrap();
        assert_eq!(way, Projection::Extend);
        assert!(put(&outputs, "eDP").enabled && put(&outputs, "HDMI").enabled);

        let (way, outputs) = next(&[panel(true), monitor(true)], &saved).unwrap();
        assert_eq!(way, Projection::External);
        assert!(!put(&outputs, "eDP").enabled && put(&outputs, "HDMI").enabled);
        assert_eq!(put(&outputs, "HDMI").position, (0, 0), "the only screen on starts at the left edge");

        let (way, outputs) = next(&[panel(false), monitor(true)], &saved).unwrap();
        assert_eq!(way, Projection::BuiltIn);
        assert!(put(&outputs, "eDP").enabled && !put(&outputs, "HDMI").enabled);
    }

    #[test]
    fn extended_screens_sit_side_by_side_without_overlap() {
        let (_, outputs) = next(&[panel(true), monitor(false)], &Saved::default()).unwrap();
        // 2560 px at scale 1.6 is 1600 logical pixels.
        assert_eq!(put(&outputs, "eDP").position, (0, 0));
        assert_eq!(put(&outputs, "HDMI").position, (1600, 0));
    }

    #[test]
    fn a_screen_coming_back_gets_the_scale_it_last_had() {
        let mut big = head("DP-1", mode(3840, 2160, 60000), false, 1.0);
        big.modes.push(Mode { preferred: false, ..mode(1920, 1080, 60000) });
        let remembered = Output { id: big.id(), enabled: true, mode: Some("3840x2160@60.00".into()), position: (0, 0), scale: 2.0, transform: "normal".into() };
        let saved = Saved { layouts: vec![Layout { monitors: vec![big.id()], outputs: vec![remembered] }] };
        let (_, outputs) = next(&[panel(true), big], &saved).unwrap();
        assert_eq!(put(&outputs, "Acme DP-1").scale, 2.0);
        assert_eq!(put(&outputs, "Acme DP-1").mode.as_deref(), Some("3840x2160@60.00"));
        assert_eq!(put(&outputs, "Acme DP-1").position, (1600, 0));
    }

    #[test]
    fn with_nothing_on_it_starts_by_extending() {
        let (way, _) = next(&[panel(false), monitor(false)], &Saved::default()).unwrap();
        assert_eq!(way, Projection::Extend);
    }

    #[test]
    fn with_no_other_screen_it_says_so_and_changes_nothing() {
        assert_eq!(next(&[panel(true)], &Saved::default()).unwrap_err(), "no other screen is connected");
    }

    #[test]
    fn a_computer_with_no_built_in_screen_is_left_alone() {
        assert!(next(&[monitor(true), head("DP-1", mode(1920, 1080, 60000), true, 1.0)], &Saved::default()).is_err());
    }

    #[test]
    fn every_step_is_something_the_compositor_will_be_asked_for() {
        // The outputs go through the same checks as an apply: modes the monitor has, scale in range, never every screen off.
        let saved = Saved::default();
        for heads in [[panel(true), monitor(false)], [panel(true), monitor(true)], [panel(false), monitor(true)]] {
            let (_, outputs) = next(&heads, &saved).unwrap();
            let settings = layout::settings(&heads, &outputs, true).unwrap();
            assert!(settings.iter().any(|setting| setting.on.is_some()));
        }
    }

    #[test]
    fn titles_say_which_way_it_is() {
        assert_eq!(Projection::BuiltIn.title(1), "Laptop screen only");
        assert_eq!(Projection::Extend.title(1), "Both screens");
        assert_eq!(Projection::Extend.title(2), "All screens");
        assert_eq!(Projection::External.title(1), "Other screen only");
        assert_eq!(Projection::External.title(2), "Other screens only");
    }
}
