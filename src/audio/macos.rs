//! What CoreAudio knows about the inputs that cpal does not say: how each one
//! is connected.

use super::Transport;
use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectPropertyAddress,
    kAudioDevicePropertyStreams, kAudioDevicePropertyTransportType,
    kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE,
    kAudioDeviceTransportTypeBuiltIn, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject,
};
use objc2_core_foundation::{CFRetained, CFString};
use std::ffi::c_void;
use std::ptr::NonNull;

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn size_of_property(object: u32, mut at: AudioObjectPropertyAddress) -> Option<u32> {
    let mut size = 0u32;
    // SAFETY: the address and the out-size are live locals.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    (status == 0).then_some(size)
}

fn devices() -> Vec<u32> {
    let at = address(
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    );
    let Some(size) = size_of_property(kAudioObjectSystemObject as u32, at) else {
        return Vec::new();
    };
    let mut ids = vec![0u32; size as usize / size_of::<u32>()];
    let mut size = size;
    let mut at = at;
    // SAFETY: `ids` is `size` bytes long, which is what is passed and what the
    // property writes at most.
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as u32,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(ids.as_mut_ptr().cast::<c_void>()).expect("a vector's pointer"),
        )
    };
    if status != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / size_of::<u32>());
    ids
}

fn has_input(device: u32) -> bool {
    size_of_property(
        device,
        address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput),
    )
    .is_some_and(|size| size > 0)
}

fn name(device: u32) -> Option<String> {
    let mut at = address(kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal);
    let mut string: *mut CFString = std::ptr::null_mut();
    let mut size = size_of::<*mut CFString>() as u32;
    // SAFETY: the property hands back a retained CFString through the pointer,
    // which is wrapped straight away so it is released exactly once.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut string).cast::<c_void>(),
        )
    };
    if status != 0 {
        return None;
    }
    let string = NonNull::new(string)?;
    // SAFETY: a non-null CFString returned under the create rule.
    Some(unsafe { CFRetained::from_raw(string) }.to_string())
}

fn transport(device: u32) -> Transport {
    let mut at = address(
        kAudioDevicePropertyTransportType,
        kAudioObjectPropertyScopeGlobal,
    );
    let mut kind = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the address, size and out-buffer are live locals, and the buffer
    // is the u32 this property holds.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut kind).cast::<c_void>(),
        )
    };
    if status != 0 {
        return Transport::Other;
    }
    match kind {
        kind if kind == kAudioDeviceTransportTypeBluetooth
            || kind == kAudioDeviceTransportTypeBluetoothLE =>
        {
            Transport::Bluetooth
        }
        kind if kind == kAudioDeviceTransportTypeBuiltIn => Transport::BuiltIn,
        _ => Transport::Other,
    }
}

/// Every input and how it is connected, by the name cpal reports for it.
pub fn input_transports() -> Vec<(String, Transport)> {
    devices()
        .into_iter()
        .filter(|device| has_input(*device))
        .filter_map(|device| Some((name(device)?, transport(device))))
        .collect()
}
