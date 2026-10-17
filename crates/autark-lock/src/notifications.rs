//! The notifications kept in the centre, under the clock: one card per app, newest first, at most three.
//! Without `notification_text` a card says only which app and how many; with it, the newest one's summary
//! and body, and how many more. They come from `autark-notifyd` and follow it while locked.

use futures_util::StreamExt;
use gtk::prelude::*;

#[zbus::proxy(interface = "org.autark.Notifications1", default_service = "org.autark.Notifications1", default_path = "/org/autark/Notifications")]
trait Notifications {
    fn kept(&self) -> zbus::Result<Vec<(String, String, String, String, i64)>>;
    #[zbus(signal)]
    fn kept_changed(&self) -> zbus::Result<()>;
}

const MOST: usize = 3;

/// One app's notifications: its newest, and how many there are.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    app: String,
    icon: String,
    summary: String,
    body: String,
    received: i64,
    count: usize,
}

/// Groups the kept notifications (newest first) by app, in the order each app last sent one.
fn group(kept: Vec<(String, String, String, String, i64)>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (app, icon, summary, body, received) in kept {
        match groups.iter_mut().find(|group| group.app == app) {
            Some(group) => group.count += 1,
            None => groups.push(Group { app, icon, summary, body, received, count: 1 }),
        }
    }
    groups.truncate(MOST);
    groups
}

/// "now", "8 min ago", "2 h ago", or the day for anything older.
fn when(received: i64, now: chrono::DateTime<chrono::Local>) -> String {
    match now.timestamp() - received {
        ..60 => "now".to_owned(),
        seconds @ ..3600 => format!("{} min ago", seconds / 60),
        seconds @ ..86_400 => format!("{} h ago", seconds / 3600),
        _ => chrono::DateTime::from_timestamp(received, 0).map(|time| time.with_timezone(&chrono::Local).format("%-d %b").to_string()).unwrap_or_default(),
    }
}

/// Fetches the notifications now and whenever they change, handing each set to `show`. Returns quietly
/// when notifyd isn't running.
pub async fn watch(show: impl Fn(Vec<Group>)) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let proxy = NotificationsProxy::new(&connection).await?;
    let mut changes = proxy.receive_kept_changed().await?;
    loop {
        match proxy.kept().await {
            Ok(kept) => show(group(kept)),
            Err(zbus::Error::MethodError(..)) => return Ok(()),
            Err(error) => return Err(error),
        }
        if changes.next().await.is_none() {
            return Ok(());
        }
    }
}

/// Replaces `list`'s cards with `groups`.
pub fn fill(list: &gtk::Box, groups: &[Group], text: bool) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let now = chrono::Local::now();
    for group in groups {
        list.append(&card(group, text, now));
    }
    list.set_visible(!groups.is_empty());
}

/// Hides the cards that `scroller` can't show whole, whenever its room or the cards change.
pub fn fit(scroller: &gtk::ScrolledWindow, list: &gtk::Box) {
    let (weak_scroller, weak_list) = (scroller.downgrade(), list.downgrade());
    scroller.vadjustment().connect_changed(move |_| {
        let (weak_scroller, weak_list) = (weak_scroller.clone(), weak_list.clone());
        // Not during layout: hiding queues another.
        gtk::glib::idle_add_local_once(move || {
            let (Some(scroller), Some(list)) = (weak_scroller.upgrade(), weak_list.upgrade()) else { return };
            let room = scroller.height() as f32;
            for card in std::iter::successors(list.first_child(), |card| card.next_sibling()) {
                if card.is_visible() && card.compute_bounds(&scroller).is_some_and(|bounds| bounds.y() + bounds.height() > room + 0.5) {
                    card.set_visible(false);
                }
            }
        });
    });
}

/// A card; its labels ellipsize and ask for one character's width, so every card is the stylesheet's width.
fn card(group: &Group, text: bool, now: chrono::DateTime<chrono::Local>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    card.add_css_class("notification");
    card.append(&icon(group));

    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_hexpand(true);
    column.set_valign(gtk::Align::Center);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("header");
    let app = gtk::Label::new(Some(&group.app));
    app.add_css_class("app");
    app.set_xalign(0.0);
    app.set_hexpand(true);
    app.set_ellipsize(gtk::pango::EllipsizeMode::End);
    app.set_max_width_chars(1);
    header.append(&app);
    header.append(&gtk::Label::new(Some(&when(group.received, now))));
    column.append(&header);

    let line = match (text, group.count) {
        (true, _) => group.summary.clone(),
        (false, 1) => "1 notification".to_owned(),
        (false, count) => format!("{count} notifications"),
    };
    let line = gtk::Label::new(Some(&line));
    line.add_css_class(if text { "summary" } else { "count" });
    line.set_xalign(0.0);
    line.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.set_max_width_chars(1);
    column.append(&line);
    if text && !group.body.is_empty() {
        // Bodies may carry the spec's markup; the first line, as plain text, is enough here.
        let plain = gtk::pango::parse_markup(&group.body, '\0').map(|(_, plain, _)| plain.to_string()).unwrap_or_else(|_| group.body.clone());
        let body = gtk::Label::new(plain.lines().next());
        body.add_css_class("body");
        body.set_xalign(0.0);
        body.set_ellipsize(gtk::pango::EllipsizeMode::End);
        body.set_max_width_chars(1);
        column.append(&body);
    }
    card.append(&column);

    if text && group.count > 1 {
        let more = gtk::Label::new(Some(&format!("+{}", group.count - 1)));
        more.add_css_class("more");
        more.set_valign(gtk::Align::Center);
        card.append(&more);
    }
    let what = if text { format!("{}: {}", group.app, group.summary) } else { format!("{}: {} notifications", group.app, group.count) };
    card.update_property(&[gtk::accessible::Property::Label(&what)]);
    card
}

/// The app's icon from a path or the icon theme, else a tile with its initial, as the centre does.
fn icon(group: &Group) -> gtk::Widget {
    let name = group.icon.strip_prefix("file://").unwrap_or(&group.icon);
    let image = if name.starts_with('/') {
        Some(gtk::Image::from_file(name))
    } else if !name.is_empty() && gtk::gdk::Display::default().is_some_and(|display| gtk::IconTheme::for_display(&display).has_icon(name)) {
        Some(gtk::Image::from_icon_name(name))
    } else {
        None
    };
    if let Some(image) = image {
        image.set_pixel_size(32);
        image.set_valign(gtk::Align::Center);
        image.add_css_class("app-icon");
        return image.upcast();
    }
    let initial = group.app.chars().next().unwrap_or('•').to_uppercase().to_string();
    let tile = gtk::Label::new(Some(&initial));
    tile.add_css_class("tile");
    tile.set_valign(gtk::Align::Center);
    tile.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(app: &str, summary: &str, received: i64) -> (String, String, String, String, i64) {
        (app.into(), String::new(), summary.into(), String::new(), received)
    }

    #[test]
    fn one_card_per_app_newest_first() {
        let groups = group(vec![kept("Messages", "c", 50), kept("Build", "x", 40), kept("Messages", "b", 30), kept("Calendar", "y", 20), kept("Mail", "z", 10)]);
        let shown: Vec<_> = groups.iter().map(|group| (group.app.as_str(), group.summary.as_str(), group.count)).collect();
        assert_eq!(shown, [("Messages", "c", 2), ("Build", "x", 1), ("Calendar", "y", 1)]);
    }

    #[test]
    fn times_read_naturally() {
        let now = chrono::Local::now();
        let at = |ago: i64| when(now.timestamp() - ago, now);
        assert_eq!(at(5), "now");
        assert_eq!(at(8 * 60 + 5), "8 min ago");
        assert_eq!(at(2 * 3600), "2 h ago");
    }
}
