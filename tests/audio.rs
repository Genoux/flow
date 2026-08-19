//! Flow must capture from whatever the system calls its default input, with no
//! device of its own baked in - switching source in the OS is the only control.

use cpal::traits::{DeviceTrait, HostTrait};

fn id(device: &cpal::Device) -> String {
    device.id().map(|i| i.to_string()).unwrap_or_default()
}

/// The regression guard: reintroducing a preferred-device match would pin Flow to
/// one soundcard and quietly ignore the user's choice.
#[test]
fn the_capture_device_is_the_system_default() {
    let host = cpal::default_host();
    let Some(expected) = host.default_input_device() else {
        eprintln!("skipping: no input device on this machine");
        return;
    };

    let opened = flow::audio::open_device().expect("open");
    assert_eq!(
        id(&opened),
        id(&expected),
        "flow picked its own device instead of the system default"
    );
}

/// Parakeet wants 16kHz mono, and the default device is expected to convert from
/// whatever the hardware natively runs at. This is the assumption that let the
/// hardcoded "pipewire" device be deleted, so it is worth holding onto.
#[test]
fn the_default_device_converts_to_what_the_model_wants() {
    let device = flow::audio::open_device().expect("open");

    // A listed device is not a working one. CI runners offer ALSA's "default"
    // with no card behind it, so the old is_none() guard waved them through and
    // the assert below blamed the conversion for missing hardware. Asking the
    // device to describe itself is what separates "no soundcard here" from the
    // regression this test exists to catch.
    if device.default_input_config().is_err() {
        eprintln!("skipping: no usable input hardware on this machine");
        return;
    }

    let capture = flow::audio::Capture::open(&device);
    assert!(
        capture.is_ok(),
        "default device refused {}Hz mono: {:?}",
        flow::audio::SAMPLE_RATE,
        capture.err()
    );
}
