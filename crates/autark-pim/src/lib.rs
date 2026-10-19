//! Personal information over open protocols, for Autark's Calendar, Tasks, Contacts and Mail and for Kith:
//! iCalendar and vCard text, CalDAV and CardDAV over HTTP, and IMAP and SMTP mail. Nothing here knows about
//! accounts or windows; callers pass an `Auth` and get plain values back.

pub mod dav;
pub mod http;
pub mod ical;
pub mod imap;
pub mod mime;
pub mod smtp;
pub mod tls;
pub mod vcard;
