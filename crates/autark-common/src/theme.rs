//! Colour tokens. Linen is the default, taken from `design/canvas/Colors.dc.html` in the Autark repo.

pub struct Palette {
    pub ground: &'static str,
    pub surface: &'static str,
    pub sidebar: &'static str,
    pub border: &'static str,
    pub text: &'static str,
    pub text2: &'static str,
    pub accent: &'static str,
    pub accent_soft: &'static str,
    pub accent_text: &'static str,
    pub wall: &'static str,
    pub bar: &'static str,
}

pub const LINEN: Palette = Palette {
    ground: "#F6F4EF",
    surface: "#FFFFFF",
    sidebar: "#EFECE5",
    border: "#E2DDD3",
    text: "#24221E",
    text2: "#68625A",
    accent: "#3F7A5F",
    accent_soft: "#E1EDE6",
    accent_text: "#336650",
    wall: "#E7E1D6",
    bar: "rgba(255, 253, 249, 0.75)",
};

impl Palette {
    /// GTK CSS `@define-color` lines, so stylesheets say `@accent` instead of a hex value.
    pub fn gtk_css(&self) -> String {
        [
            ("ground", self.ground),
            ("surface", self.surface),
            ("sidebar", self.sidebar),
            ("border", self.border),
            ("text", self.text),
            ("text2", self.text2),
            ("accent", self.accent),
            ("accent_soft", self.accent_soft),
            ("accent_text", self.accent_text),
            ("wall", self.wall),
            ("bar", self.bar),
        ]
        .iter()
        .map(|(name, value)| format!("@define-color {name} {value};\n"))
        .collect()
    }
}
