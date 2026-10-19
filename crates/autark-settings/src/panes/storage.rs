//! Storage: how full each mounted disk is, what the Trash and cache hold, and the biggest folders in your home. Sizes of
//! folders are worked out in the background (`du` can take a while on a big disk), so the pane opens at once.

use std::path::PathBuf;
use std::process::Command;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::ui;

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Storage", None));

    let disks = ui::group();
    for disk in mounted() {
        let fraction = if disk.size == 0 { 0.0 } else { disk.used as f64 / disk.size as f64 };
        let bar = gtk::LevelBar::builder().min_value(0.0).max_value(1.0).value(fraction).width_request(220).valign(gtk::Align::Center).build();
        bar.add_offset_value("low", 0.8);
        bar.add_offset_value("high", 0.9);
        bar.add_offset_value("full", 1.0);
        let note = format!("{} used of {} · {}", size(disk.used), size(disk.size), disk.kind);
        disks.append(&ui::row(&disk.target, Some(&note), &bar));
    }
    column.append(&ui::section("Disks", None, &disks));

    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let trash = home.join(".local/share/Trash");
    let tidy = ui::group();
    let trash_size = ui::value("Working it out…");
    let empty = ui::button("Empty Trash", {
        let (trash, trash_size) = (trash.clone(), trash_size.clone());
        move |button| {
            for folder in ["files", "info", "expunged"] {
                let path = trash.join(folder);
                let _ = std::fs::remove_dir_all(&path);
                if folder != "expunged" {
                    let _ = std::fs::create_dir_all(&path);
                }
            }
            trash_size.set_label("Empty");
            button.set_sensitive(false);
        }
    });
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.append(&trash_size);
    line.append(&empty);
    tidy.append(&ui::row("Trash", Some("Shared with Files and every app that moves things to the Trash"), &line));
    let cache_size = ui::value("Working it out…");
    tidy.append(&ui::row("Caches", Some("~/.cache: thumbnails and downloads apps can fetch again"), &cache_size));
    column.append(&ui::section("Clean up", None, &tidy));

    let biggest = ui::group();
    let waiting = ui::note("Looking at your home folder…");
    biggest.append(&waiting);
    column.append(&ui::section("Largest folders in your home", None, &biggest));

    glib::spawn_future_local({
        let (home, trash) = (home.clone(), trash.clone());
        async move {
            let sizes = gio::spawn_blocking(move || {
                let trash_bytes = folder_size(&trash);
                let cache_bytes = folder_size(&home.join(".cache"));
                (trash_bytes, cache_bytes, children(&home))
            })
            .await;
            let Ok((trash_bytes, cache_bytes, top)) = sizes else { return };
            if trash_bytes > 0 {
                trash_size.set_label(&size(trash_bytes));
            } else {
                trash_size.set_label("Empty");
                empty.set_sensitive(false);
            }
            cache_size.set_label(&size(cache_bytes));
            ui::clear(&biggest);
            for (name, bytes) in top.into_iter().take(8) {
                biggest.append(&ui::row(&name, None, &ui::value(&size(bytes))));
            }
        }
    });
    page.upcast()
}

struct Disk {
    target: String,
    size: u64,
    used: u64,
    kind: String,
}

/// The real mounted file systems, from `df`.
fn mounted() -> Vec<Disk> {
    let output = Command::new("df")
        .args(["-B1", "--output=target,size,used,fstype", "-x", "tmpfs", "-x", "devtmpfs", "-x", "efivarfs", "-x", "squashfs", "-x", "overlay"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let [target, total, used, kind] = parts[..] else { return None };
            Some(Disk { target: target.to_owned(), size: total.parse().ok()?, used: used.parse().ok()?, kind: kind.to_owned() })
        })
        .collect()
}

fn folder_size(path: &std::path::Path) -> u64 {
    let output = Command::new("du").args(["-sxb"]).arg(path).output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    output.split_whitespace().next().and_then(|bytes| bytes.parse().ok()).unwrap_or(0)
}

/// Each folder directly in `home`, biggest first (hidden ones left out).
fn children(home: &std::path::Path) -> Vec<(String, u64)> {
    let mut found: Vec<(String, u64)> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()) && !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| (entry.file_name().to_string_lossy().into_owned(), folder_size(&entry.path())))
        .collect();
    found.sort_by(|a, b| b.1.cmp(&a.1));
    found
}

fn size(bytes: u64) -> String {
    glib::format_size(bytes).to_string()
}
