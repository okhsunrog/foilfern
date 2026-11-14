//! Read explicitly declared connector directions; no geometric/name heuristics.
//! Uses the native board-device transform, so placement and bottom-side mirroring
//! follow the same numeric semantics as physical pads.

use foilfern_core::project::{Project, board::BoardDevice};
use foilfern_core::types::{Length, Point};
use serde_json::{Value, json};

const ATTRIBUTE: &str = "FOILFERN_CONNECTOR_MATING_DIRECTION";

pub(super) fn describe(project: &Project, device: &BoardDevice) -> Value {
    let instance = project
        .circuit()
        .component_instance(device.component())
        .and_then(|component| component.attributes().by_name(ATTRIBUTE, true));
    let library = project
        .library()
        .device(&device.lib_device())
        .and_then(|device| device.attributes().by_name(ATTRIBUTE, true));
    let Some((attribute, source)) = instance
        .map(|attribute| (attribute, "instance_attribute"))
        .or_else(|| library.map(|attribute| (attribute, "device_attribute")))
    else {
        return json!({"status":"unknown"});
    };
    let axis = attribute.value().trim().to_ascii_lowercase();
    let (x, y) = match axis.as_str() {
        "+x" => (1_000_000, 0),
        "-x" => (-1_000_000, 0),
        "+y" => (0, 1_000_000),
        "-y" => (0, -1_000_000),
        _ => {
            return json!({"status":"invalid_metadata","source":source,"attribute":ATTRIBUTE,"value":attribute.value()});
        }
    };
    // A one-millimetre axis gives unitless vector components after division
    // by one millimetre; native integer geometry rounds to one nanometre.
    let vector = device
        .transform()
        .map(&Point::new(Length::new(x), Length::new(y)))
        - device.position();
    json!({"status":"declared","source":source,"attribute":ATTRIBUTE,
        "local_axis":axis,"board_vector":crate::views::point(vector)})
}
