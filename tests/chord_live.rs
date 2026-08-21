//! Does the real evdev path see the chord? The state machine is unit-tested, but
//! only a genuine key event proves the reader, the keycodes and the chord agree.
//!
//! Creates its own uinput keyboard and presses itself. Needs /dev/uinput and a
//! readable /dev/input, and the compositor will also see these keys - so stop
//! flow.service first or the daemon will dictate into whatever has focus:
//!   systemctl --user stop flow.service
//!   cargo test --release --test chord_live -- --ignored --nocapture
//!   systemctl --user start flow.service

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, KeyCode, KeyEvent};
use flow::hotkey::{self, Chord, Event};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn keyboard() -> VirtualDevice {
    let mut keys = AttributeSet::<KeyCode>::new();
    for key in [
        KeyCode::KEY_A,
        KeyCode::KEY_D,
        KeyCode::KEY_C,
        KeyCode::KEY_LEFTMETA,
        KeyCode::KEY_LEFTSHIFT,
    ] {
        keys.insert(key);
    }
    VirtualDevice::builder()
        .expect("open /dev/uinput")
        .name("flow chord test keyboard")
        .with_keys(&keys)
        .expect("keys")
        .build()
        .expect("build")
}

fn press(device: &mut VirtualDevice, key: KeyCode, down: bool) {
    device
        .emit(&[*KeyEvent::new(key, i32::from(down))])
        .expect("emit");
    std::thread::sleep(Duration::from_millis(12));
}

#[test]
#[ignore]
fn the_real_keyboard_path_sees_the_chord() {
    let mut device = keyboard();
    // udev has to create the node and flow has to enumerate it, in that order.
    std::thread::sleep(Duration::from_millis(400));

    let (events, incoming) = std::sync::mpsc::channel();
    hotkey::spawn(events, Arc::new(Mutex::new(Chord::default()))).expect("spawn");
    std::thread::sleep(Duration::from_millis(200));

    press(&mut device, KeyCode::KEY_LEFTMETA, true);
    press(&mut device, KeyCode::KEY_LEFTSHIFT, true);
    press(&mut device, KeyCode::KEY_D, true);

    let pressed = incoming
        .recv_timeout(Duration::from_secs(2))
        .expect("no Pressed event - the chord never reached flow");
    assert_eq!(pressed, Event::Pressed);

    // Lifting Super first: the case Hyprland's release bind gets wrong.
    press(&mut device, KeyCode::KEY_LEFTMETA, false);
    let released = incoming
        .recv_timeout(Duration::from_secs(2))
        .expect("no Released event - lifting a modifier did not stop the hold");
    assert!(
        matches!(released, Event::Released { .. }),
        "got {released:?}"
    );

    press(&mut device, KeyCode::KEY_D, false);
    press(&mut device, KeyCode::KEY_LEFTSHIFT, false);
    std::thread::sleep(Duration::from_millis(50));

    // A plain letter must stay a plain letter.
    press(&mut device, KeyCode::KEY_D, true);
    press(&mut device, KeyCode::KEY_D, false);
    assert!(
        incoming.recv_timeout(Duration::from_millis(400)).is_err(),
        "a bare d fired dictation"
    );

    eprintln!("chord seen, modifier-lift release seen, bare letter ignored");
}

/// The reported failure, end to end: a stray key arriving mid-hold used to cancel
/// and bin the audio with nothing in the log, so a dictation simply disappeared
/// and the key "did not register". A remapper echoing a physical key beside its
/// virtual one produces exactly this.
#[test]
#[ignore]
fn a_stray_key_mid_hold_does_not_lose_the_recording() {
    let mut device = keyboard();
    std::thread::sleep(Duration::from_millis(400));

    let (events, incoming) = std::sync::mpsc::channel();
    hotkey::spawn(events, Arc::new(Mutex::new(Chord::default()))).expect("spawn");
    std::thread::sleep(Duration::from_millis(200));

    press(&mut device, KeyCode::KEY_LEFTMETA, true);
    press(&mut device, KeyCode::KEY_LEFTSHIFT, true);
    press(&mut device, KeyCode::KEY_D, true);
    assert_eq!(
        incoming
            .recv_timeout(Duration::from_secs(2))
            .expect("Pressed"),
        Event::Pressed
    );

    // The intruder: a key nobody meant to press, in the middle of dictating.
    press(&mut device, KeyCode::KEY_C, true);
    press(&mut device, KeyCode::KEY_C, false);
    assert!(
        incoming.recv_timeout(Duration::from_millis(400)).is_err(),
        "a stray key cancelled the hold and threw the dictation away"
    );

    press(&mut device, KeyCode::KEY_D, false);
    let released = incoming
        .recv_timeout(Duration::from_secs(2))
        .expect("Released");
    assert!(
        matches!(released, Event::Released { .. }),
        "got {released:?}"
    );

    press(&mut device, KeyCode::KEY_LEFTSHIFT, false);
    press(&mut device, KeyCode::KEY_LEFTMETA, false);
    eprintln!("stray key survived, recording still ended cleanly");
}
