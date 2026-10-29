//! Paste by staging the transcript on the general pasteboard and sending Cmd+V.
//!
//! Cmd+V is the paste chord in terminals too, so unlike the Linux backend there
//! is no focused-window lookup and no second chord to choose between.

use super::{
    CLIPBOARD_BUDGET, CLIPBOARD_GENERATION, CLIPBOARD_RESTORE_DELAY, KEY_DELAY, give_back,
    put_aside, take_put_aside, wait_out_held_modifiers, with_deadline,
};
use crate::hotkey;
use anyhow::{Context, Result, ensure};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting};
use objc2_core_graphics::{
    CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGKeyCode,
    CGPreflightPostEventAccess,
};
use objc2_foundation::{NSArray, NSData, NSString};
use std::sync::atomic::Ordering;
use std::time::Duration;

/// kVK_ANSI_V. A virtual keycode names a position on the keyboard, and macOS
/// matches Cmd shortcuts against the character the active layout produces for
/// it, so this is V only on QWERTY-positioned layouts.
///
/// ponytail: fixed ANSI keycode, so non-QWERTY layouts (Dvorak) paste nothing;
/// look up "v" via TISCopyCurrentKeyboardLayoutInputSource + UCKeyTranslate
/// when someone needs it.
const KEY_V: CGKeyCode = 0x09;

/// Every item on the pasteboard, each as the formats it offered. Owned bytes
/// rather than `NSPasteboardItem`s: those belong to the pasteboard they were
/// read from and cannot be put back after it has been cleared.
pub type Saved = Vec<Vec<(String, Vec<u8>)>>;

pub struct Injector;

impl Injector {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    pub fn backend(&self) -> &'static str {
        "cgevent"
    }

    /// Put `text` in the focused window by staging it on the pasteboard and
    /// sending Cmd+V, then hand the user's own pasteboard back.
    pub fn inject(&mut self, text: &str) -> Result<()> {
        let generation = CLIPBOARD_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let saved = take_put_aside().unwrap_or_else(snapshot);
        let staged = stage(text)?;

        // CGEventPost is silently refused without Accessibility, so ask first.
        // After staging, so the daemon's "text is on your clipboard" promise
        // holds; the user's old pasteboard is deliberately not restored here.
        ensure!(
            CGPreflightPostEventAccess(),
            "macOS will not let flow send keystrokes - grant Accessibility to the app running \
             flow in System Settings > Privacy & Security"
        );

        let modifiers_clear =
            hotkey::wait_for_modifiers_released(Duration::ZERO) || wait_out_held_modifiers();
        send_paste()?;

        if !saved.is_empty() && modifiers_clear {
            put_aside(generation, saved);
            std::thread::spawn(move || {
                std::thread::sleep(CLIPBOARD_RESTORE_DELAY);
                give_back(generation, |saved| {
                    if let Err(err) = restore(saved, staged) {
                        eprintln!("clipboard: {err:#}");
                    }
                });
            });
        }
        Ok(())
    }
}

/// Replaces the pasteboard with `text` and returns the change count that
/// replacement produced, which is how a later restore knows nobody copied
/// something newer in the meantime.
fn stage(text: &str) -> Result<isize> {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: a constant string exported by AppKit.
    let kind = unsafe { NSPasteboardTypeString };
    ensure!(
        pasteboard.setString_forType(&NSString::from_str(text), kind),
        "staging text on the pasteboard"
    );
    Ok(pasteboard.changeCount())
}

/// Cmd+V as one down and one up. The Command flag is set on the events
/// themselves, so it does not matter which physical modifiers the HID state
/// reports.
fn send_paste() -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
    for key_down in [true, false] {
        let event = CGEvent::new_keyboard_event(source.as_deref(), KEY_V, key_down)
            .context("creating the paste keystroke")?;
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        if key_down {
            std::thread::sleep(KEY_DELAY);
        }
    }
    Ok(())
}

/// Every format of every item, under the same budget the Linux clipboard gets:
/// an owner that provides its data lazily can block the read indefinitely, and
/// one stuck read must not freeze every dictation after it. Losing the saved
/// clipboard is the smaller cost.
fn snapshot() -> Saved {
    with_deadline(CLIPBOARD_BUDGET, read_pasteboard).unwrap_or_else(|| {
        eprintln!(
            "clipboard: no answer within {CLIPBOARD_BUDGET:?}; pasting without saving what was there"
        );
        Vec::new()
    })
}

fn read_pasteboard() -> Saved {
    let Some(items) = NSPasteboard::generalPasteboard().pasteboardItems() else {
        return Vec::new();
    };
    items
        .to_vec()
        .iter()
        .map(|item| {
            item.types()
                .to_vec()
                .iter()
                .filter_map(|kind| {
                    item.dataForType(kind)
                        .map(|data| (kind.to_string(), data.to_vec()))
                })
                .collect::<Vec<_>>()
        })
        .filter(|formats| !formats.is_empty())
        .collect()
}

/// Put the user's pasteboard back, unless something has been copied since the
/// transcript was staged - that copy is newer than anything saved here.
fn restore(saved: Saved, staged: isize) -> Result<()> {
    let pasteboard = NSPasteboard::generalPasteboard();
    if pasteboard.changeCount() != staged {
        return Ok(());
    }
    let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = saved
        .iter()
        .map(|formats| {
            let item = NSPasteboardItem::new();
            for (kind, bytes) in formats {
                item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(kind));
            }
            ProtocolObject::from_retained(item)
        })
        .collect();
    pasteboard.clearContents();
    ensure!(
        pasteboard.writeObjects(&NSArray::from_retained_slice(&items)),
        "putting the saved clipboard back failed, so it is now empty"
    );
    Ok(())
}
