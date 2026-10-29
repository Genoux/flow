//! A notification from Flow itself, through UserNotifications, so it carries
//! Flow's name and icon.

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNUserNotificationCenter,
};
use std::sync::atomic::{AtomicU64, Ordering};

/// Best effort, like `notify-send` being absent: a process with no bundle
/// (`cargo run`, a bare binary) cannot use UserNotifications, which raises an
/// exception for it rather than returning an error, and a user who said Don't
/// Allow has made a choice that is not ours to route around. Either way the
/// log line `failure` already wrote is all there is.
///
/// Needs the daemon signed with the bundle's identifier: under any other the
/// notification centre refuses at once, with no prompt.
pub fn post(summary: &str, body: &str) {
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return;
    }

    let (summary, body) = (summary.to_owned(), body.to_owned());
    let center = UNUserNotificationCenter::currentNotificationCenter();
    // Asked on every post rather than remembered: once the user has answered,
    // this returns that answer at once, and it is the only way to learn it.
    // The first ever call is what shows the "Would Like to Send You
    // Notifications" prompt, and nothing is posted until it is answered.
    let answered = RcBlock::new(move |granted: Bool, _: *mut NSError| {
        if granted.as_bool() {
            add(&summary, &body);
        }
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &answered,
    );
}

fn add(summary: &str, body: &str) {
    static POSTED: AtomicU64 = AtomicU64::new(0);

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(summary));
    content.setBody(&NSString::from_str(body));
    // Distinct, because a request with the identifier of one still on screen
    // replaces it, and these are separate failures.
    let identifier = format!(
        "flow-{}-{}",
        std::process::id(),
        POSTED.fetch_add(1, Ordering::Relaxed)
    );
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&identifier),
        &content,
        None,
    );
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, None);
}
