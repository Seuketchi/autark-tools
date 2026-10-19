//! Talking to whoever is at the keyboard: messages, and a secret typed without echo. A trait so the unlock logic can be
//! tested with a script of answers instead of a keyboard.

use std::io::{BufRead, Write};

use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};
use zeroize::Zeroizing;

pub trait Console {
    fn say(&mut self, text: &str);
    /// Asks for a secret. `None` when there's nothing more to read (end of input).
    fn secret(&mut self, prompt: &str) -> Option<Zeroizing<Vec<u8>>>;
    fn wait_for_enter(&mut self, text: &str);
}

/// Standard input and output, which in the initramfs are the console.
pub struct Terminal;

impl Console for Terminal {
    fn say(&mut self, text: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }

    fn secret(&mut self, prompt: &str) -> Option<Zeroizing<Vec<u8>>> {
        {
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "{prompt}");
            let _ = out.flush();
        }
        let input = rustix::stdio::stdin();
        // Echo goes off for the line and comes back after; when input isn't a terminal (a test, a pipe) there's nothing to turn off.
        let saved = tcgetattr(input).ok();
        if let Some(saved) = &saved {
            let mut quiet = saved.clone();
            quiet.local_modes.remove(LocalModes::ECHO);
            let _ = tcsetattr(input, OptionalActions::Flush, &quiet);
        }
        let mut line = Zeroizing::new(Vec::new());
        let read = std::io::stdin().lock().read_until(b'\n', &mut line);
        if let Some(saved) = &saved {
            let _ = tcsetattr(input, OptionalActions::Flush, saved);
        }
        let _ = writeln!(std::io::stdout());
        match read {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                while line.last().is_some_and(|last| matches!(last, b'\n' | b'\r')) {
                    line.pop();
                }
                Some(line)
            }
        }
    }

    fn wait_for_enter(&mut self, text: &str) {
        self.say(text);
        let mut ignored = String::new();
        let _ = std::io::stdin().lock().read_line(&mut ignored);
    }
}
