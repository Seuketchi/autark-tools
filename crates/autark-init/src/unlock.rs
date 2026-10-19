//! How a volume is opened: the TPM first when the command line asks for it, and the PIN if the token wants one, then a
//! passphrase or recovery key. It is what `tpm2-device=auto` does in systemd-cryptsetup, with the same three tries, so a
//! wrong PIN can't be tried without limit against the TPM's own lockout counter. The actual libcryptsetup calls are behind
//! [`Volume`], which is what makes the order and the limits testable here.

use zeroize::Zeroizing;

use crate::console::Console;

pub const TRIES: u32 = 3;

#[derive(Debug, PartialEq, Eq)]
pub enum Attempt {
    Unlocked,
    /// The token needs its PIN.
    NeedsPin,
    /// The secret was refused.
    Wrong,
    /// This way of opening can't work now (no TPM, no such token, the TPM is locked out); the message says why.
    Unavailable(String),
}

pub trait Volume {
    /// The TPM2 token without a PIN.
    fn token(&mut self) -> Attempt;
    fn token_with_pin(&mut self, pin: &[u8]) -> Attempt;
    fn passphrase(&mut self, passphrase: &[u8]) -> Attempt;
}

#[derive(Debug, PartialEq, Eq)]
pub enum How {
    Tpm,
    Passphrase,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// Nobody answered (end of input).
    NoAnswer,
    TooManyTries,
    /// Opening failed for a reason typing again won't fix.
    Device(String),
}

pub fn unlock(volume: &mut impl Volume, console: &mut impl Console, tpm2: bool) -> Result<How, Failure> {
    if tpm2 {
        match volume.token() {
            Attempt::Unlocked => return Ok(How::Tpm),
            Attempt::NeedsPin => {
                if let Some(how) = ask_pin(volume, console)? {
                    return Ok(how);
                }
            }
            Attempt::Wrong => console.say("The TPM didn't release the key. The boot state may have changed."),
            Attempt::Unavailable(why) => console.say(&format!("The TPM can't unlock the disk: {why}")),
        }
        console.say("Use the passphrase or the recovery key instead.");
    }
    for _ in 0..TRIES {
        let Some(secret) = console.secret("Passphrase or recovery key: ") else {
            return Err(Failure::NoAnswer);
        };
        match volume.passphrase(&secret) {
            Attempt::Unlocked => return Ok(How::Passphrase),
            Attempt::Wrong | Attempt::NeedsPin => console.say("That isn't right."),
            Attempt::Unavailable(why) => return Err(Failure::Device(why)),
        }
    }
    Err(Failure::TooManyTries)
}

/// Up to [`TRIES`] PINs. `Ok(None)` means give up on the TPM and fall back to the passphrase.
fn ask_pin(volume: &mut impl Volume, console: &mut impl Console) -> Result<Option<How>, Failure> {
    for _ in 0..TRIES {
        let Some(pin): Option<Zeroizing<Vec<u8>>> = console.secret("TPM PIN: ") else {
            return Err(Failure::NoAnswer);
        };
        match volume.token_with_pin(&pin) {
            Attempt::Unlocked => return Ok(Some(How::Tpm)),
            Attempt::Wrong | Attempt::NeedsPin => console.say("That PIN isn't right."),
            Attempt::Unavailable(why) => {
                console.say(&format!("The TPM can't unlock the disk: {why}"));
                return Ok(None);
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A volume that answers from a script, and remembers what it was asked.
    #[derive(Default)]
    struct Fake {
        token: Option<Attempt>,
        pins: Vec<(&'static str, Attempt)>,
        passphrases: Vec<(&'static str, Attempt)>,
        asked: Vec<String>,
    }

    impl Fake {
        fn answer(list: &mut Vec<(&'static str, Attempt)>, given: &[u8]) -> Attempt {
            let given = String::from_utf8_lossy(given).into_owned();
            match list.iter().position(|(secret, _)| *secret == given) {
                Some(at) => list.remove(at).1,
                None => Attempt::Wrong,
            }
        }
    }

    impl Volume for Fake {
        fn token(&mut self) -> Attempt {
            self.asked.push("token".into());
            self.token.take().unwrap_or(Attempt::Unavailable("no token".into()))
        }
        fn token_with_pin(&mut self, pin: &[u8]) -> Attempt {
            self.asked.push(format!("pin {}", String::from_utf8_lossy(pin)));
            Self::answer(&mut self.pins, pin)
        }
        fn passphrase(&mut self, passphrase: &[u8]) -> Attempt {
            self.asked.push(format!("passphrase {}", String::from_utf8_lossy(passphrase)));
            Self::answer(&mut self.passphrases, passphrase)
        }
    }

    /// A console that types what it's told, and keeps what was said.
    struct Typist {
        typed: Vec<&'static str>,
        said: Vec<String>,
        prompts: Vec<String>,
    }

    impl Typist {
        fn new(typed: &[&'static str]) -> Self {
            Self { typed: typed.iter().rev().copied().collect(), said: Vec::new(), prompts: Vec::new() }
        }
    }

    impl Console for Typist {
        fn say(&mut self, text: &str) {
            self.said.push(text.to_owned());
        }
        fn secret(&mut self, prompt: &str) -> Option<Zeroizing<Vec<u8>>> {
            self.prompts.push(prompt.to_owned());
            self.typed.pop().map(|text| Zeroizing::new(text.as_bytes().to_vec()))
        }
        fn wait_for_enter(&mut self, _: &str) {}
    }

    #[test]
    fn the_tpm_alone_opens_it_without_asking_anything() {
        let mut volume = Fake { token: Some(Attempt::Unlocked), ..Fake::default() };
        let mut console = Typist::new(&[]);
        assert_eq!(unlock(&mut volume, &mut console, true), Ok(How::Tpm));
        assert!(console.prompts.is_empty());
    }

    #[test]
    fn the_pin_opens_it() {
        let mut volume = Fake { token: Some(Attempt::NeedsPin), pins: vec![("1234", Attempt::Unlocked)], ..Fake::default() };
        let mut console = Typist::new(&["1234"]);
        assert_eq!(unlock(&mut volume, &mut console, true), Ok(How::Tpm));
        assert_eq!(console.prompts, ["TPM PIN: "]);
    }

    #[test]
    fn a_wrong_pin_can_be_retried_but_only_three_times() {
        let mut volume = Fake { token: Some(Attempt::NeedsPin), passphrases: vec![("correct horse", Attempt::Unlocked)], ..Fake::default() };
        let mut console = Typist::new(&["1", "2", "3", "correct horse"]);
        assert_eq!(unlock(&mut volume, &mut console, true), Ok(How::Passphrase));
        // Three PINs reached the TPM, no more: its lockout counter is the thing being protected.
        assert_eq!(volume.asked.iter().filter(|asked| asked.starts_with("pin ")).count(), 3);
        assert!(console.said.iter().any(|said| said.contains("passphrase")));
    }

    #[test]
    fn the_second_pin_can_be_right() {
        let mut volume = Fake { token: Some(Attempt::NeedsPin), pins: vec![("5678", Attempt::Unlocked)], ..Fake::default() };
        let mut console = Typist::new(&["0000", "5678"]);
        assert_eq!(unlock(&mut volume, &mut console, true), Ok(How::Tpm));
    }

    #[test]
    fn no_tpm_goes_straight_to_the_passphrase_with_a_reason() {
        let mut volume = Fake { token: Some(Attempt::Unavailable("no TPM found".into())), passphrases: vec![("secret", Attempt::Unlocked)], ..Fake::default() };
        let mut console = Typist::new(&["secret"]);
        assert_eq!(unlock(&mut volume, &mut console, true), Ok(How::Passphrase));
        assert!(console.said.iter().any(|said| said.contains("no TPM found")));
    }

    #[test]
    fn a_locked_out_tpm_during_the_pin_falls_back_without_more_tries() {
        let mut volume = Fake { token: Some(Attempt::NeedsPin), pins: vec![("1", Attempt::Unavailable("locked out".into()))], passphrases: vec![("secret", Attempt::Unlocked)], ..Fake::default() };
        let mut console = Typist::new(&["1", "secret"]);
        assert_eq!(unlock(&mut volume, &mut console, true), Ok(How::Passphrase));
        assert_eq!(volume.asked.iter().filter(|asked| asked.starts_with("pin ")).count(), 1);
    }

    #[test]
    fn without_tpm2_in_the_options_the_token_is_never_tried() {
        let mut volume = Fake { passphrases: vec![("secret", Attempt::Unlocked)], ..Fake::default() };
        let mut console = Typist::new(&["secret"]);
        assert_eq!(unlock(&mut volume, &mut console, false), Ok(How::Passphrase));
        assert!(!volume.asked.iter().any(|asked| asked == "token"));
    }

    #[test]
    fn three_wrong_passphrases_give_up() {
        let mut volume = Fake::default();
        let mut console = Typist::new(&["a", "b", "c", "d"]);
        assert_eq!(unlock(&mut volume, &mut console, false), Err(Failure::TooManyTries));
        assert_eq!(volume.asked.len(), 3);
    }

    #[test]
    fn nobody_answering_stops_it() {
        let mut volume = Fake::default();
        let mut console = Typist::new(&[]);
        assert_eq!(unlock(&mut volume, &mut console, false), Err(Failure::NoAnswer));
    }

    #[test]
    fn a_device_failure_is_not_retried() {
        let mut volume = Fake { passphrases: vec![("x", Attempt::Unavailable("I/O error".into()))], ..Fake::default() };
        let mut console = Typist::new(&["x", "y"]);
        assert_eq!(unlock(&mut volume, &mut console, false), Err(Failure::Device("I/O error".into())));
        assert_eq!(volume.asked.len(), 1);
    }
}
