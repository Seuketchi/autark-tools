//! Password checks through PAM, on a thread of their own so the lock screen keeps drawing.

use pam_client::conv_mock::Conversation;
use pam_client::{Context, Flag};
use zeroize::Zeroizing;

/// Whether `password` is `user`'s, according to the PAM service.
pub async fn check(service: String, user: String, password: Zeroizing<String>) -> bool {
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = (|| {
            let conversation = Conversation::with_credentials(user.as_str(), password.as_str());
            let mut context = Context::new(&service, Some(&user), conversation)?;
            context.authenticate(Flag::NONE)?;
            context.acct_mgmt(Flag::NONE)
        })();
        if let Err(err) = &result {
            eprintln!("lock: {err}");
        }
        let _ = sender.send_blocking(result.is_ok());
    });
    receiver.recv().await.unwrap_or(false)
}
