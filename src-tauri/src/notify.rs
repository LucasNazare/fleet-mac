//! macOS notifications. Sent asynchronously (never block a thread per notification);
//! a click activates fleet, and the window-focus handler then jumps to the session
//! that notified most recently (or the one whose notification was clicked).
//!
//! The bundled app uses UserNotifications (`UNUserNotificationCenter`): it asks for
//! permission on first launch and shows up in System Settings → Notifications.
//! `tauri dev` runs an unbundled binary, where UNUserNotificationCenter throws, so it
//! falls back to the deprecated NSUserNotification path (attributed to Terminal.app).
use parking_lot::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static LAST: Mutex<Option<(String, Instant)>> = Mutex::new(None);
static BUNDLED: OnceLock<bool> = OnceLock::new();

fn bundled() -> bool {
    *BUNDLED.get_or_init(|| std::env::current_exe().unwrap_or_default().to_string_lossy().contains(".app/Contents/MacOS"))
}

pub fn init() {
    if bundled() {
        un::init();
    } else {
        let _ = mac_notification_sys::set_application("com.apple.Terminal");
    }
}

pub fn send(session_id: &str, title: &str, message: &str) {
    send_with_sound(session_id, title, message, None);
}

pub fn send_with_sound(session_id: &str, title: &str, message: &str, sound: Option<&str>) {
    *LAST.lock() = Some((session_id.to_string(), Instant::now()));
    if bundled() {
        un::send(session_id, title, message, sound, None);
        return;
    }
    let (title, message, sound) = (title.to_string(), message.to_string(), sound.map(String::from));
    std::thread::spawn(move || {
        let mut n = mac_notification_sys::Notification::new();
        n.title(&title).message(&message).asynchronous(true);
        if let Some(s) = &sound {
            n.sound(s.as_str());
        }
        if let Err(e) = n.send() {
            eprintln!("[fleet] notification failed: {e}");
        }
    });
}

/// Sends a test notification and reports what macOS said (permission off, error…).
pub fn test(sound: Option<&str>) -> Result<String, String> {
    if !bundled() {
        let mut n = mac_notification_sys::Notification::new();
        n.title("fleet").message("Test notification").asynchronous(true);
        if let Some(s) = sound {
            n.sound(s);
        }
        n.send().map_err(|e| e.to_string())?;
        return Ok("Sent (dev build: shows as Terminal).".into());
    }
    match un::authorization() {
        Some(un::Auth::Denied) => {
            return Err("Notifications are turned off for fleet. Turn them on in System Settings → Notifications → fleet.".into())
        }
        Some(un::Auth::NotDetermined) => un::request_authorization(),
        _ => {}
    }
    let (tx, rx) = std::sync::mpsc::channel();
    un::send("", "fleet", "Test notification: this is how fleet tells you a session is done or needs you.", sound, Some(tx));
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(match un::authorization() {
            Some(un::Auth::Authorized) => "Sent.".into(),
            _ => "Sent. If nothing appeared, allow fleet in System Settings → Notifications.".into(),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("macOS didn't answer. Check System Settings → Notifications → fleet.".into()),
    }
}

/// Session to jump to when the app is activated shortly after a notification.
pub fn take_recent(within: Duration) -> Option<String> {
    let mut l = LAST.lock();
    match l.take() {
        Some((id, at)) if at.elapsed() <= within => Some(id),
        _ => None,
    }
}

mod un {
    use block2::RcBlock;
    use objc2_app_kit::NSSound;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AllocAnyThread};
    use objc2_foundation::{NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
        UNNotificationRequest, UNNotificationResponse, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::Sender;
    use std::time::{Duration, Instant};

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "FleetNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// Show banners even while fleet is the active app (fleet already skips
            /// notifying about the session you're looking at).
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }

            /// A click: remember which session it was for, so the focus handler jumps there.
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, handler: &block2::DynBlock<dyn Fn()>) {
                let id = response.notification().request().identifier().to_string();
                if let Some((session, _)) = id.rsplit_once('#') {
                    if !session.is_empty() {
                        *super::LAST.lock() = Some((session.to_string(), Instant::now()));
                    }
                }
                handler.call(());
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    pub enum Auth {
        NotDetermined,
        Denied,
        Authorized,
        Other,
    }

    fn center() -> Retained<UNUserNotificationCenter> {
        UNUserNotificationCenter::currentNotificationCenter()
    }

    pub fn init() {
        let c = center();
        // The delegate property is weak: keep ours alive for the life of the app.
        let d = Delegate::new();
        c.setDelegate(Some(ProtocolObject::from_ref(&*d)));
        std::mem::forget(d);
        request_authorization();
    }

    pub fn request_authorization() {
        let block = RcBlock::new(|granted: Bool, err: *mut NSError| {
            if let Some(e) = unsafe { err.as_ref() } {
                eprintln!("[fleet] notification permission request failed: {}", e.localizedDescription());
            } else if !granted.as_bool() {
                eprintln!("[fleet] notifications not allowed for fleet (System Settings → Notifications)");
            }
        });
        center().requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound | UNAuthorizationOptions::Badge,
            &block,
        );
    }

    /// Current permission, or None if macOS didn't answer within 2 s.
    pub fn authorization() -> Option<Auth> {
        let (tx, rx) = std::sync::mpsc::channel();
        let block = RcBlock::new(move |s: NonNull<UNNotificationSettings>| {
            let st = unsafe { s.as_ref() }.authorizationStatus();
            let _ = tx.send(match st {
                UNAuthorizationStatus::NotDetermined => Auth::NotDetermined,
                UNAuthorizationStatus::Denied => Auth::Denied,
                UNAuthorizationStatus::Authorized => Auth::Authorized,
                _ => Auth::Other,
            });
        });
        center().getNotificationSettingsWithCompletionHandler(&block);
        rx.recv_timeout(Duration::from_secs(2)).ok()
    }

    static SEQ: AtomicU64 = AtomicU64::new(0);

    pub fn send(session_id: &str, title: &str, body: &str, sound: Option<&str>, done: Option<Sender<Result<(), String>>>) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        if let Some(s) = sound {
            // System sounds (Glass, Hero, Basso…) live in /System/Library/Sounds, which
            // UNNotificationSound doesn't search; NSSound does. Fall back to the default.
            match NSSound::soundNamed(&NSString::from_str(s)) {
                Some(snd) => {
                    // soundNamed returns AppKit's cached instance, so dropping ours is fine.
                    snd.play();
                }
                None => content.setSound(Some(&UNNotificationSound::defaultSound())),
            }
        }
        if !session_id.is_empty() {
            // Groups a session's notifications together in Notification Center.
            content.setThreadIdentifier(&NSString::from_str(session_id));
        }
        // "<session>#<n>": unique per notification; the click handler reads the session back.
        let id = format!("{session_id}#{}", SEQ.fetch_add(1, Ordering::Relaxed));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&id), &content, None);
        let done = std::sync::Mutex::new(done);
        let block = RcBlock::new(move |err: *mut NSError| {
            let res = match unsafe { err.as_ref() } {
                Some(e) => {
                    let msg = e.localizedDescription().to_string();
                    eprintln!("[fleet] notification failed: {msg}");
                    Err(msg)
                }
                None => Ok(()),
            };
            if let Some(tx) = done.lock().ok().and_then(|mut d| d.take()) {
                let _ = tx.send(res);
            }
        });
        center().addNotificationRequest_withCompletionHandler(&request, Some(&block));
    }
}

/// Settings → Notifications → "Send test notification".
#[tauri::command]
pub async fn notify_test() -> Result<String, String> {
    let sound = crate::settings::sounds().then_some("Glass");
    tauri::async_runtime::spawn_blocking(move || test(sound)).await.map_err(|e| e.to_string())?
}
