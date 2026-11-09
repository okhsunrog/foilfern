//! The command parameters are the wire format of the MCP layer (JSON).

use librepcb_core::project::ComponentInstanceId;
use librepcb_core::types::Uuid;
use librepcb_editor::commands::*;

#[test]
fn test_component_ref() {
    let r: ComponentRef = serde_json::from_str("\"R1\"").unwrap();
    assert_eq!(r, ComponentRef::Name("R1".into()));
    let uuid = Uuid::new_random();
    let r: ComponentRef = serde_json::from_str(&format!("\"{uuid}\"")).unwrap();
    assert_eq!(r, ComponentRef::Id(ComponentInstanceId(uuid)));
    assert_eq!(serde_json::to_string(&r).unwrap(), format!("\"{uuid}\""));
}

#[test]
fn test_commands_from_json() {
    let cmd: DrawWire = serde_json::from_str(
        r#"{
            "start": {"Pin": {"component": "R1", "pin": "1"}},
            "end": {"Point": {"x": 2540000, "y": 0}},
            "net": "VCC"
        }"#,
    )
    .unwrap();
    assert_eq!(cmd.start, WireAnchor::Pin(PinRef::new("R1", "1")));
    assert!(cmd.points.is_empty());
    assert_eq!(cmd.net.unwrap().as_str(), "VCC");

    let uuid = Uuid::new_random();
    let cmd: AddComponent = serde_json::from_str(&format!(
        r#"{{"component": "{uuid}", "place": {{"position": {{"x": 0, "y": 0}}}}}}"#
    ))
    .unwrap();
    assert_eq!(cmd.component, uuid);
    assert!(cmd.place.is_some());

    let cmd: AddTrace = serde_json::from_str(
        r#"{
            "start": {"Pad": {"component": "R1", "pad": "2"}},
            "end": {"Pad": {"component": "R2", "pad": "1"}},
            "layer": "top_cu"
        }"#,
    )
    .unwrap();
    assert_eq!(cmd.end, TraceEndpoint::Pad(PadRef::new("R2", "1")));

    // Round trip.
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(serde_json::from_str::<AddTrace>(&json).unwrap(), cmd);
}
