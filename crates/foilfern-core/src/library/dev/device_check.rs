//! Port of libs/librepcb/core/library/dev/devicecheck.{h,cpp}.

use super::device::Device;
use super::device_check_messages::DeviceCheckMessage;
use crate::library::{LibraryCheckMessage, LibraryElement, run_element_checks};

/// Runs the device check, including the checks common to all elements
/// (upstream `DeviceCheck::runChecks()`).
pub fn run_device_checks(device: &Device, msgs: &mut Vec<LibraryCheckMessage>) {
    run_element_checks(device.element_metadata(), msgs);
    check_no_pads_connected(device, msgs);
    check_parts(device, msgs);
}

fn check_no_pads_connected(device: &Device, msgs: &mut Vec<LibraryCheckMessage>) {
    let map = device.pad_signal_map();
    if !map.is_empty() && map.iter().all(|item| item.signal_uuid().is_none()) {
        msgs.push(DeviceCheckMessage::NoPadsConnected.into());
    }
}

fn check_parts(device: &Device, msgs: &mut Vec<LibraryCheckMessage>) {
    if device.parts().is_empty() {
        msgs.push(DeviceCheckMessage::DeviceHasNoParts.into());
    }
}
