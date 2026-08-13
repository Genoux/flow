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
    let host = cpal::default_host();
    if host.default_input_device().is_none() {
        eprintln!("skipping: no input device on this machine");
        return;
    }

    let device = flow::audio::open_device().expect("open");
    let capture = flow::audio::Capture::open(&device);
    assert!(
        capture.is_ok(),
        "default device refused {}Hz mono: {:?}",
        flow::audio::SAMPLE_RATE,
        capture.err()
    );
}
