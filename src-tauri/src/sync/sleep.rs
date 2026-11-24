//! Mac going to sleep while a session is mid-prompt: make the wip commit and push first.
use std::ptr::NonNull;
use std::time::Duration;
use tauri::AppHandle;

pub fn observe(app: AppHandle) {
    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceWillSleepNotification};
    use objc2_foundation::NSNotification;
    let block = RcBlock::new(move |_n: NonNull<NSNotification>| {
        // Called on the main thread; macOS waits a little for apps to finish.
        super::wip_all(&app, Duration::from_secs(8));
    });
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: the name is a static NSString, no object filter, main queue (None = posting thread).
    let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(NSWorkspaceWillSleepNotification), None, None, &block) };
    // Keep the observer for the life of the app.
    std::mem::forget(token);
    std::mem::forget(block);
}
