//! Does the compositor accept the keymap Flow uploads?
//!
//! The keymap in `wayland_vk` is a hand-written xkb string, and libxkbcommon is
//! the only thing that can say whether it parses. A compositor that rejects it
//! answers `invalid_keymap` on the protocol, which surfaces here as an error
//! from the roundtrip inside `VirtualKeyboard::new`. Nothing else catches it:
//! the daemon would fall back to uinput and paste the wrong chord instead.
//!
//! Only uploads a keymap - no keys are sent, so it is safe to run while the
//! daemon is up. Needs a Wayland session whose compositor implements
//! zwp_virtual_keyboard_manager_v1:
//!   cargo test --release --test keymap_live -- --ignored --nocapture

use flow::wayland_vk::VirtualKeyboard;
use std::time::Duration;

#[test]
#[ignore]
fn the_compositor_accepts_the_uploaded_keymap() {
    let keyboard = VirtualKeyboard::new(Duration::from_millis(8))
        .expect("compositor rejected the connection, the keymap, or the protocol");
    drop(keyboard);
    eprintln!("keymap accepted");
}
