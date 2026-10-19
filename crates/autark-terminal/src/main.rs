//! Terminal: tabs of VTE terminals in the Linen look, with Kith's `Check` on pastes that could do damage. Ctrl+Shift+T
//! opens a tab (in the folder the current one is in), Ctrl+Shift+N a window, Ctrl+Shift+W closes a tab, Ctrl+Shift+C and
//! Ctrl+Shift+V copy and paste, Ctrl+Shift+F searches, Ctrl+plus, minus and 0 zoom. `autark-terminal [folder]` opens
//! in that folder.

mod check;
mod colors;
mod tab;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.autark.Terminal").flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    app.connect_startup(|_| autark_common::theme::install(include_str!("terminal.css")));
    app.connect_command_line(|app, line| {
        // A folder given on the command line is where the first tab opens; otherwise the shell's own default (home).
        let folder = line.arguments().get(1).map(|arg| std::path::PathBuf::from(arg)).filter(|path| path.is_dir());
        window::Window::new(app, folder.as_deref()).window.present();
        0.into()
    });
    app.run()
}
