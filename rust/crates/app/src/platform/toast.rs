//! Windows toasts, shown from a worker thread of their own (the WinRT calls can block, and in order they can't
//! race: a toast is shown before it's updated or removed). Each is tagged, so a newer toast with the same tag
//! replaces it, and its text is bound to data, so it can be changed in place without popping up again.
//!
//! Clicking one brings the window back and hands its channel to the app ([`take_clicked`]). Windows only
//! tells the process that showed it, while the toast object lives: they're kept until replaced or removed,
//! so a click from the notification centre works too (while Swiftcord is running).

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use windows::core::{IInspectable, Result, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::UI::Notifications::{NotificationData, NotificationUpdateResult, ToastNotification, ToastNotificationManager};

use super::APP_ID;

/// Every toast of ours is in this group (tags are only unique within one).
const GROUP: &str = "swiftcord";
/// Silent: the app plays its own sounds, as Discord does.
const TEMPLATE: &str = r#"<toast><visual><binding template="ToastGeneric"><text>{title}</text><text>{body}</text></binding></visual><audio silent="true"/></toast>"#;

enum Command {
    Show { tag: String, title: String, body: String, popup: bool, open: u64 },
    Update { tag: String, title: String, body: String, sequence: u32, open: u64 },
    Remove { tag: String },
}

static CLICKED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The channels of the toasts clicked since last time, oldest first.
pub fn take_clicked() -> Vec<u64> {
    std::mem::take(&mut *CLICKED.lock().unwrap())
}

fn send(command: Command) {
    static WORKER: OnceLock<Sender<Command>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            // The toasts shown, by tag: kept so their clicks are still reported.
            let mut shown = HashMap::new();
            for command in rx {
                if let Err(e) = run(command, &mut shown) {
                    tracing::warn!("toast failed: {e}");
                }
            }
        });
        tx
    });
    let _ = worker.send(command);
}

/// A toast for `tag`, replacing the one before it; `popup: false` puts it in the notification centre only.
/// Clicking it opens channel `open`.
pub fn show(tag: impl Into<String>, title: String, body: String, popup: bool, open: u64) {
    send(Command::Show { tag: tag.into(), title, body, popup, open });
}

/// New text for the toast for `tag`, in place, without popping up; `sequence` must grow with each update. If
/// that toast is gone (dismissed), the text goes in a new one in the notification centre.
pub fn update(tag: impl Into<String>, title: String, body: String, sequence: u32, open: u64) {
    send(Command::Update { tag: tag.into(), title, body, sequence, open });
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

fn run(command: Command, shown: &mut HashMap<String, ToastNotification>) -> Result<()> {
    let (app, group) = (HSTRING::from(APP_ID), HSTRING::from(GROUP));
    match command {
        Command::Show { tag, title, body, popup, open } => {
            let xml = XmlDocument::new()?;
            xml.LoadXml(&HSTRING::from(TEMPLATE))?;
            let toast = ToastNotification::CreateToastNotification(&xml)?;
            toast.SetTag(&HSTRING::from(&tag))?;
            toast.SetGroup(&group)?;
            toast.SetSuppressPopup(!popup)?;
            toast.SetData(&data(&title, &body, 0)?)?;
            // Called on a thread of Windows': noted, and the window brought back (which runs the app to open it).
            toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, _| {
                CLICKED.lock().unwrap().push(open);
                super::tray::show_window();
                Ok(())
            }))?;
            ToastNotificationManager::CreateToastNotifierWithId(&app)?.Show(&toast)?;
            shown.insert(tag, toast);
            Ok(())
        }
        Command::Update { tag, title, body, sequence, open } => {
            let notifier = ToastNotificationManager::CreateToastNotifierWithId(&app)?;
            let result = notifier.UpdateWithTagAndGroup(&data(&title, &body, sequence)?, &HSTRING::from(&tag), &group)?;
            if result == NotificationUpdateResult::NotificationNotFound {
                return run(Command::Show { tag, title, body, popup: false, open }, shown);
            }
            Ok(())
        }
        Command::Remove { tag } => {
            shown.remove(&tag);
            ToastNotificationManager::History()?.RemoveGroupedTagWithId(&HSTRING::from(tag), &group, &app)
        }
    }
}
