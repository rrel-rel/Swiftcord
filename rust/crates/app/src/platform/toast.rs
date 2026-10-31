//! Windows toasts, shown from a worker thread of their own (the WinRT calls can block, and in order they can't
//! race: a toast is shown before it's updated or removed). Each is tagged, so a newer toast with the same tag
//! replaces it, and its text is bound to data, so it can be changed in place without popping up again.

use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

use windows::core::{Result, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::UI::Notifications::{NotificationData, NotificationUpdateResult, ToastNotification, ToastNotificationManager};

use super::APP_ID;

/// Every toast of ours is in this group (tags are only unique within one).
const GROUP: &str = "swiftcord";
/// Silent: the app plays its own sounds, as Discord does.
const TEMPLATE: &str = r#"<toast><visual><binding template="ToastGeneric"><text>{title}</text><text>{body}</text></binding></visual><audio silent="true"/></toast>"#;

enum Command {
    Show { tag: String, title: String, body: String, popup: bool },
    Update { tag: String, title: String, body: String, sequence: u32 },
    Remove { tag: String },
}

fn send(command: Command) {
    static WORKER: OnceLock<Sender<Command>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            for command in rx {
                if let Err(e) = run(command) {
                    tracing::warn!("toast failed: {e}");
                }
            }
        });
        tx
    });
    let _ = worker.send(command);
}

/// A toast for `tag`, replacing the one before it; `popup: false` puts it in the notification centre only.
pub fn show(tag: impl Into<String>, title: String, body: String, popup: bool) {
    send(Command::Show { tag: tag.into(), title, body, popup });
}

/// New text for the toast for `tag`, in place, without popping up; `sequence` must grow with each update. If
/// that toast is gone (dismissed), the text goes in a new one in the notification centre.
pub fn update(tag: impl Into<String>, title: String, body: String, sequence: u32) {
    send(Command::Update { tag: tag.into(), title, body, sequence });
}

/// Takes the toast for `tag` off the screen and out of the notification centre.
pub fn remove(tag: impl Into<String>) {
    send(Command::Remove { tag: tag.into() });
}

fn data(title: &str, body: &str, sequence: u32) -> Result<NotificationData> {
    let data = NotificationData::new()?;
    let values = data.Values()?;
    values.Insert(&HSTRING::from("title"), &HSTRING::from(title))?;
    values.Insert(&HSTRING::from("body"), &HSTRING::from(body))?;
    data.SetSequenceNumber(sequence)?;
    Ok(data)
}

fn run(command: Command) -> Result<()> {
    let (app, group) = (HSTRING::from(APP_ID), HSTRING::from(GROUP));
    match command {
        Command::Show { tag, title, body, popup } => {
            let xml = XmlDocument::new()?;
            xml.LoadXml(&HSTRING::from(TEMPLATE))?;
            let toast = ToastNotification::CreateToastNotification(&xml)?;
            toast.SetTag(&HSTRING::from(tag))?;
            toast.SetGroup(&group)?;
            toast.SetSuppressPopup(!popup)?;
            toast.SetData(&data(&title, &body, 0)?)?;
            ToastNotificationManager::CreateToastNotifierWithId(&app)?.Show(&toast)
        }
        Command::Update { tag, title, body, sequence } => {
            let notifier = ToastNotificationManager::CreateToastNotifierWithId(&app)?;
            let result = notifier.UpdateWithTagAndGroup(&data(&title, &body, sequence)?, &HSTRING::from(&tag), &group)?;
            if result == NotificationUpdateResult::NotificationNotFound {
                return run(Command::Show { tag, title, body, popup: false });
            }
            Ok(())
        }
        Command::Remove { tag } => ToastNotificationManager::History()?.RemoveGroupedTagWithId(&HSTRING::from(tag), &group, &app),
    }
}
