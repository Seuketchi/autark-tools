//! System actions offered in results: lock, sleep, restart, shut down and log out, through logind.

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login {
    fn lock_session(&self, session_id: &str) -> zbus::Result<()>;
    fn terminate_session(&self, session_id: &str) -> zbus::Result<()>;
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
    fn reboot(&self, interactive: bool) -> zbus::Result<()>;
    fn power_off(&self, interactive: bool) -> zbus::Result<()>;
}

#[derive(Clone, Copy)]
pub enum Action {
    Lock,
    Sleep,
    Restart,
    ShutDown,
    LogOut,
}

pub const ALL: [Action; 5] = [Action::Lock, Action::Sleep, Action::Restart, Action::ShutDown, Action::LogOut];

impl Action {
    pub fn title(self) -> &'static str {
        match self {
            Action::Lock => "Lock screen",
            Action::Sleep => "Sleep",
            Action::Restart => "Restart",
            Action::ShutDown => "Shut down",
            Action::LogOut => "Log out",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Action::Lock => "Super + L",
            Action::Sleep => "Suspend to memory",
            Action::Restart => "Restart the computer",
            Action::ShutDown => "Turn the computer off",
            Action::LogOut => "End this session",
        }
    }

    /// Other words people type for the action.
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Action::Lock => &["lock"],
            Action::Sleep => &["suspend"],
            Action::Restart => &["reboot"],
            Action::ShutDown => &["power off", "poweroff", "turn off"],
            Action::LogOut => &["logout", "sign out", "exit"],
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Action::Lock => "system-lock-screen-symbolic",
            Action::Sleep => "weather-clear-night-symbolic",
            Action::Restart => "system-reboot-symbolic",
            Action::ShutDown => "system-shutdown-symbolic",
            Action::LogOut => "system-log-out-symbolic",
        }
    }

    pub async fn run(self) -> zbus::Result<()> {
        let login = LoginProxy::new(&zbus::Connection::system().await?).await?;
        // logind takes "auto" to mean the caller's own session.
        match self {
            Action::Lock => login.lock_session("auto").await,
            Action::Sleep => login.suspend(true).await,
            Action::Restart => login.reboot(true).await,
            Action::ShutDown => login.power_off(true).await,
            Action::LogOut => login.terminate_session("auto").await,
        }
    }
}
