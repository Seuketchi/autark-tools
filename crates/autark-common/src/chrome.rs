//! The window buttons every Autark app shares: minimise, maximise and close at the right end of the top bar, as quiet
//! rounded buttons with their symbols always showing, in Linen's greys, with close turning red only under the pointer.
//! [`controls`] builds them; the stylesheet comes with every app's theme (see [`CSS`]).

use gtk::prelude::*;

/// Added to every stylesheet by `theme::install`, before the app's own, so an app can still adjust its buttons.
pub const CSS: &str = "
windowcontrols.autark-controls {
  margin: 0;
}

windowcontrols.autark-controls button {
  min-width: 28px;
  min-height: 28px;
  margin: 0 0 0 2px;
  padding: 0;
  border: none;
  border-radius: 8px;
  box-shadow: none;
  background: none;
  color: @text2;
}

windowcontrols.autark-controls button:hover {
  background-color: alpha(@text, 0.08);
  color: @text;
}

windowcontrols.autark-controls button:active {
  background-color: alpha(@text, 0.14);
}

windowcontrols.autark-controls button.close:hover {
  background-color: @danger;
  color: #FFFFFF;
}

windowcontrols.autark-controls button image {
  -gtk-icon-size: 14px;
}
";

/// Minimise, maximise (when `maximize`) and close, to go at the right end of a bar.
pub fn controls(maximize: bool) -> gtk::WindowControls {
    let controls = gtk::WindowControls::new(gtk::PackType::End);
    controls.set_decoration_layout(Some(if maximize { ":minimize,maximize,close" } else { ":minimize,close" }));
    controls.add_css_class("autark-controls");
    controls.set_halign(gtk::Align::End);
    controls.set_valign(gtk::Align::Center);
    controls
}
