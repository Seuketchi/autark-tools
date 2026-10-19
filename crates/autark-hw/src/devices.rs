//! The devices in the machine, from udev's database (`udevadm info --export-db`), which already has names from the
//! hardware databases, the driver each is bound to and its modalias. Only PCI devices and USB devices (with their
//! interfaces, where the drivers are) are listed.

use std::collections::HashMap;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    Graphics,
    Network,
    Wireless,
    Bluetooth,
    Audio,
    Camera,
    Storage,
    Input,
    UsbController,
    /// Bridges, hubs and the like, which need no driver of their own to be useful.
    Bridge,
    Other,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Graphics => "Graphics",
            Class::Network => "Network",
            Class::Wireless => "Wi-Fi and wireless",
            Class::Bluetooth => "Bluetooth",
            Class::Audio => "Audio",
            Class::Camera => "Cameras",
            Class::Storage => "Storage",
            Class::Input => "Input devices",
            Class::UsbController => "USB controllers",
            Class::Bridge => "Bridges and hubs",
            Class::Other => "Other",
        }
    }

    /// Whether going without a driver is a loss, as it is for a GPU, and not for a bridge.
    pub fn needs_driver(self) -> bool {
        !matches!(self, Class::Bridge | Class::Other)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Pci,
    Usb,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub path: String,
    pub bus: Bus,
    /// The slot or port: `0000:00:02.0`, `1-6`.
    pub address: String,
    /// `8086:a7a8`.
    pub id: String,
    pub vendor: String,
    pub model: String,
    pub class: Class,
    /// The drivers bound, once each; a USB device's are its interfaces'.
    pub drivers: Vec<String>,
    /// What kernel modules are looked up by: the device's own, and for USB its interfaces'.
    pub modaliases: Vec<String>,
}

impl Device {
    /// How to call it: the model, else the vendor and the ID.
    pub fn title(&self) -> String {
        match (self.model.is_empty(), self.vendor.is_empty()) {
            (false, false) if !self.model.starts_with(&self.vendor) => format!("{} {}", short_vendor(&self.vendor), self.model),
            (false, _) => self.model.clone(),
            (true, false) => format!("{} ({})", short_vendor(&self.vendor), self.id),
            _ => self.id.clone(),
        }
    }
}

/// `Intel Corporation` as `Intel`, and so on, for a line that has to fit.
fn short_vendor(vendor: &str) -> String {
    for suffix in [" Corporation", " Corp.", " Corp", ", Inc.", " Inc.", " Co., Ltd.", " Ltd.", " Technology", " Semiconductor"] {
        if let Some(short) = vendor.strip_suffix(suffix) {
            return short.to_owned();
        }
    }
    vendor.to_owned()
}

/// One entry of udev's database: where it is and its properties.
#[derive(Debug, Default)]
pub struct Record {
    pub path: String,
    pub env: HashMap<String, String>,
}

pub fn parse_db(text: &str) -> Vec<Record> {
    let mut records = Vec::new();
    let mut current = Record::default();
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !current.path.is_empty() {
                records.push(std::mem::take(&mut current));
            }
        } else if let Some(path) = line.strip_prefix("P: ") {
            current.path = path.to_owned();
        } else if let Some(pair) = line.strip_prefix("E: ")
            && let Some((key, value)) = pair.split_once('=')
        {
            current.env.insert(key.to_owned(), value.to_owned());
        }
    }
    records
}

fn pci_class(class: u32) -> Class {
    match (class >> 16, (class >> 8) & 0xff) {
        (0x03, _) => Class::Graphics,
        (0x02, 0x80) => Class::Wireless,
        (0x02, _) => Class::Network,
        (0x04, 0x01 | 0x03) => Class::Audio,
        (0x04, 0x00) => Class::Camera,
        (0x01, _) => Class::Storage,
        (0x0c, 0x03) => Class::UsbController,
        (0x0d, 0x11) => Class::Bluetooth,
        (0x0d, _) => Class::Wireless,
        (0x06, _) => Class::Bridge,
        _ => Class::Other,
    }
}

/// A USB device's kind, from its interfaces' classes (`:030100:0e0100:`): the most telling one wins.
fn usb_class(interfaces: &str) -> Class {
    let classes: Vec<&str> = interfaces.split(':').filter(|part| part.len() >= 6).collect();
    let has = |wanted: &str| classes.iter().any(|class| class.starts_with(wanted));
    if has("0e") {
        Class::Camera
    } else if has("e00101") || has("e0") {
        Class::Bluetooth
    } else if has("01") {
        Class::Audio
    } else if has("08") {
        Class::Storage
    } else if has("03") {
        Class::Input
    } else if has("02") || has("0a") {
        Class::Network
    } else if has("09") {
        Class::Bridge
    } else {
        Class::Other
    }
}

fn name(env: &HashMap<String, String>, database: &str, descriptor: &str) -> String {
    env.get(database).or_else(|| env.get(descriptor)).map(|name| name.replace('_', " ")).unwrap_or_default()
}

pub fn devices_from(records: &[Record]) -> Vec<Device> {
    let mut devices = Vec::new();
    for record in records {
        let env = &record.env;
        let subsystem = env.get("SUBSYSTEM").map(String::as_str);
        match (subsystem, env.get("DEVTYPE").map(String::as_str)) {
            (Some("pci"), _) => {
                let class = env.get("PCI_CLASS").and_then(|class| u32::from_str_radix(class, 16).ok()).map_or(Class::Other, pci_class);
                devices.push(Device {
                    path: record.path.clone(),
                    bus: Bus::Pci,
                    address: env.get("PCI_SLOT_NAME").cloned().unwrap_or_default(),
                    id: env.get("PCI_ID").map(|id| id.to_lowercase()).unwrap_or_default(),
                    vendor: name(env, "ID_VENDOR_FROM_DATABASE", "ID_VENDOR"),
                    model: name(env, "ID_MODEL_FROM_DATABASE", "ID_MODEL"),
                    class,
                    drivers: env.get("DRIVER").cloned().into_iter().collect(),
                    modaliases: env.get("MODALIAS").cloned().into_iter().collect(),
                });
            }
            (Some("usb"), Some("usb_device")) => {
                let id = env.get("PRODUCT").map(|product| product.split('/').take(2).map(|part| format!("{part:0>4}")).collect::<Vec<_>>().join(":")).unwrap_or_default();
                let mut class = usb_class(env.get("ID_USB_INTERFACES").map_or("", String::as_str));
                // The root hub of a controller is part of the controller.
                if env.get("ID_MODEL").is_some_and(|model| model.contains("Host_Controller")) {
                    class = Class::Bridge;
                }
                devices.push(Device {
                    path: record.path.clone(),
                    bus: Bus::Usb,
                    address: record.path.rsplit('/').next().unwrap_or("").to_owned(),
                    id,
                    vendor: name(env, "ID_VENDOR_FROM_DATABASE", "ID_VENDOR"),
                    model: name(env, "ID_MODEL_FROM_DATABASE", "ID_MODEL"),
                    class,
                    drivers: Vec::new(),
                    modaliases: env.get("MODALIAS").cloned().into_iter().collect(),
                });
            }
            _ => {}
        }
    }
    // A USB device is run by its interfaces' drivers, and found by their modaliases.
    for record in records {
        let env = &record.env;
        if env.get("SUBSYSTEM").map(String::as_str) != Some("usb") || env.get("DEVTYPE").map(String::as_str) != Some("usb_interface") {
            continue;
        }
        let Some(parent) = record.path.rsplit_once('/').map(|(parent, _)| parent) else { continue };
        if let Some(device) = devices.iter_mut().find(|device| device.bus == Bus::Usb && device.path == parent) {
            if let Some(driver) = env.get("DRIVER")
                && !device.drivers.contains(driver)
            {
                device.drivers.push(driver.clone());
            }
            if let Some(alias) = env.get("MODALIAS") {
                device.modaliases.push(alias.clone());
            }
        }
    }
    devices.sort_by(|a, b| (a.class, &a.address).cmp(&(b.class, &b.address)));
    devices
}

/// The devices as udev knows them now.
pub fn read() -> Vec<Device> {
    let output = Command::new("udevadm").args(["info", "--export-db"]).output();
    output.ok().filter(|output| output.status.success()).map(|output| devices_from(&parse_db(&String::from_utf8_lossy(&output.stdout)))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = "\
P: /devices/pci0000:00/0000:00:02.0
M: 0000:00:02.0
U: pci
E: DRIVER=i915
E: PCI_CLASS=30000
E: PCI_ID=8086:A7A8
E: PCI_SLOT_NAME=0000:00:02.0
E: SUBSYSTEM=pci
E: MODALIAS=pci:v00008086d0000A7A8sv00001462sd000013B9bc03sc00i00
E: ID_VENDOR_FROM_DATABASE=Intel Corporation
E: ID_MODEL_FROM_DATABASE=Raptor Lake-P [UHD Graphics]

P: /devices/pci0000:00/0000:01:00.0
U: pci
E: PCI_CLASS=30000
E: PCI_ID=10DE:25A2
E: PCI_SLOT_NAME=0000:01:00.0
E: SUBSYSTEM=pci
E: MODALIAS=pci:v000010DEd000025A2sv00001462sd000013B9bc03sc00i00
E: ID_VENDOR_FROM_DATABASE=NVIDIA Corporation
E: ID_MODEL_FROM_DATABASE=GA107M [GeForce RTX 3050 Mobile]

P: /devices/pci0000:00/0000:00:14.3
U: pci
E: DRIVER=iwlwifi
E: PCI_CLASS=28000
E: PCI_ID=8086:51F1
E: PCI_SLOT_NAME=0000:00:14.3
E: SUBSYSTEM=pci

P: /devices/pci0000:00/0000:00:14.0/usb1/1-7
U: usb
T: usb_device
E: DEVTYPE=usb_device
E: DRIVER=usb
E: PRODUCT=1462/1601/300
E: SUBSYSTEM=usb
E: ID_VENDOR_FROM_DATABASE=Micro Star International
E: ID_MODEL=MysticLight_MS-1565
E: ID_USB_INTERFACES=:030000:
E: MODALIAS=usb:v1462p1601d0300dc00dsc00dp00ic00isc00ip00in00

P: /devices/pci0000:00/0000:00:14.0/usb1/1-7/1-7:1.0
U: usb
T: usb_interface
E: DEVTYPE=usb_interface
E: DRIVER=usbhid
E: SUBSYSTEM=usb
E: MODALIAS=usb:v1462p1601d0300dc00dsc00dp00ic03isc00ip00in00

P: /devices/pci0000:00/0000:00:14.0/usb1/1-6
U: usb
T: usb_device
E: DEVTYPE=usb_device
E: PRODUCT=30c9/42/1
E: SUBSYSTEM=usb
E: ID_MODEL=Integrated_Camera
E: ID_USB_INTERFACES=:0e0100:0e0200:
";

    #[test]
    fn pci_and_usb_devices_with_their_drivers() {
        let devices = devices_from(&parse_db(DB));
        let find = |id: &str| devices.iter().find(|device| device.id == id).unwrap();
        let gpu = find("8086:a7a8");
        assert_eq!((gpu.class, gpu.drivers.as_slice(), gpu.address.as_str()), (Class::Graphics, &["i915".to_owned()][..], "0000:00:02.0"));
        assert_eq!(gpu.title(), "Intel Raptor Lake-P [UHD Graphics]");
        let nvidia = find("10de:25a2");
        assert_eq!((nvidia.class, nvidia.drivers.is_empty()), (Class::Graphics, true));
        assert_eq!(find("8086:51f1").class, Class::Wireless);
        // A USB device gets its driver from its interface, and its modalias list grows by the interface's.
        let lights = find("1462:1601");
        assert_eq!((lights.class, lights.drivers.as_slice(), lights.modaliases.len()), (Class::Input, &["usbhid".to_owned()][..], 2));
        assert_eq!(lights.title(), "Micro Star International MysticLight MS-1565");
        let camera = find("30c9:0042");
        assert_eq!((camera.class, camera.drivers.is_empty()), (Class::Camera, true));
    }

    #[test]
    fn multimedia_devices_are_told_apart() {
        assert_eq!(pci_class(0x040100), Class::Audio);
        assert_eq!(pci_class(0x040300), Class::Audio);
        assert_eq!(pci_class(0x040000), Class::Camera);
        assert_eq!(pci_class(0x048000), Class::Other);
    }

    #[test]
    fn devices_are_ordered_by_kind() {
        let devices = devices_from(&parse_db(DB));
        let classes: Vec<Class> = devices.iter().map(|device| device.class).collect();
        let mut sorted = classes.clone();
        sorted.sort();
        assert_eq!(classes, sorted);
    }
}
