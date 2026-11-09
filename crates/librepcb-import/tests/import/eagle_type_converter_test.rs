//! Port of tests/unittests/eagleimport/eagletypeconvertertest.cpp.

use std::collections::BTreeMap;

use librepcb_core::attribute::AttributeType;
use librepcb_core::geometry::{ComponentSide, PadShape, Path, Vertex};
use librepcb_core::types::{
    Alignment, Angle, HAlign, Layer, Length, Point, PositiveLength, StrokeTextSpacing,
    UnsignedLength, VAlign,
};
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_import::eagle::EagleTypeConverter;
use librepcb_import::eagle::model::{self, DomElement};

fn dom(xml: &str) -> DomElement {
    DomElement::parse(xml).unwrap()
}

fn nm(x: i64, y: i64) -> Point {
    Point::new(Length::new(x), Length::new(y))
}

fn ul(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap()
}

fn pl(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

fn path(vertices: &[(i64, i64, i32)]) -> Path {
    Path::new(
        vertices
            .iter()
            .map(|&(x, y, a)| Vertex::new(nm(x, y), Angle::new(a)))
            .collect(),
    )
}

fn wire(xml: &str) -> model::Wire {
    model::Wire::from_dom(&dom(xml), &mut Vec::new()).unwrap()
}

#[test]
fn test_convert_element_name() {
    let c = EagleTypeConverter::default();
    assert_eq!("Valid Name", c.convert_element_name("Valid Name").as_str());
    assert_eq!("X", c.convert_element_name(" \nX ").as_str());
    assert_eq!("Unnamed", c.convert_element_name("\n").as_str());
}

#[test]
fn test_convert_element_description() {
    let c = EagleTypeConverter::default();
    assert_eq!("", c.convert_element_description(""));
    assert_eq!("Text", c.convert_element_description(" Text "));
    assert_eq!("X\nY", c.convert_element_description("X\nY"));
    assert_eq!("X\nY", c.convert_element_description("<b>X</b><br/>Y"));
    assert_eq!("X\nY", c.convert_element_description("<b>X</b>\n<br/>Y"));
}

#[test]
fn test_convert_component_name() {
    let c = EagleTypeConverter::default();
    assert_eq!(
        "Valid Name",
        c.convert_component_name("Valid Name").as_str()
    );
    assert_eq!("X", c.convert_component_name(" \nX ").as_str());
    assert_eq!("Foo - Bar", c.convert_component_name("Foo - Bar-").as_str());
    assert_eq!("Foo _ Bar", c.convert_component_name("Foo _ Bar_").as_str());
    assert_eq!("-", c.convert_component_name("-").as_str());
    assert_eq!("Unnamed", c.convert_component_name("\n").as_str());
}

#[test]
fn test_convert_device_name() {
    let c = EagleTypeConverter::default();
    let cases = [
        ("Valid Name", "", "Valid Name"),
        ("Valid Name", "Foo", "Valid Name-Foo"),
        ("Valid Name-", "Foo", "Valid Name-Foo"),
        ("Valid Name_", "Foo", "Valid Name_Foo"),
        ("Valid Name", "-Foo", "Valid Name-Foo"),
        ("Valid Name", "_Foo", "Valid Name_Foo"),
        (" \nX ", "", "X"),
        ("\n", "", "Unnamed"),
        ("", "", "Unnamed"),
    ];
    for (set, dev, expected) in cases {
        assert_eq!(expected, c.convert_device_name(set, dev).as_str());
    }
}

#[test]
fn test_convert_component_prefix() {
    let c = EagleTypeConverter::default();
    assert_eq!("", c.convert_component_prefix("").as_str());
    assert_eq!("", c.convert_component_prefix("$42+").as_str());
    assert_eq!("C", c.convert_component_prefix("C").as_str());
    assert_eq!("Foo_Bar", c.convert_component_prefix(" Foo Bar ").as_str());
}

#[test]
fn test_convert_gate_name() {
    let c = EagleTypeConverter::default();
    assert_eq!("", c.convert_gate_name("").as_str());
    assert_eq!("G42", c.convert_gate_name("G$42").as_str());
    assert_eq!("1", c.convert_gate_name("-1").as_str());
    assert_eq!("Foo_Bar", c.convert_gate_name(" Foo Bar ").as_str());
}

#[test]
fn test_convert_pin_or_pad_name() {
    let c = EagleTypeConverter::default();
    assert_eq!("Unnamed", c.convert_pin_or_pad_name(" ").as_str());
    assert_eq!("42", c.convert_pin_or_pad_name("P$42").as_str());
    assert_eq!("3", c.convert_pin_or_pad_name("3").as_str());
    assert_eq!("Foo_Bar", c.convert_pin_or_pad_name(" Foo Bar ").as_str());
    assert_eq!("!FOO!/BAR", c.convert_pin_or_pad_name("!FOO/BAR").as_str());
}

#[test]
fn test_convert_inversion_syntax() {
    let c = EagleTypeConverter::default();
    let cases = [
        ("FOO", "FOO"),
        ("!FOO", "!FOO"),
        ("!FOO!", "!FOO"),
        ("!FOO!/BAR", "!FOO/BAR"),
        ("!FOO/BAR", "!FOO!/BAR"),
        ("FOO/!BAR", "FOO/!BAR"),
        ("FOO/!BAR!", "FOO/!BAR"),
        ("A/!B!/C", "A/!B/C"),
    ];
    for (input, expected) in cases {
        assert_eq!(expected, c.convert_inversion_syntax(input), "{input}");
    }
}

#[test]
fn test_convert_attribute_valid() {
    let c = EagleTypeConverter::default();
    let log = MessageLogger::new();
    let xml = "<attribute name=\"Foo Bar\" value=\"hello world!\"/>";
    let a = model::Attribute::from_dom(&dom(xml), &mut Vec::new()).unwrap();
    let out = c.try_convert_attribute(&a, &log).unwrap();
    assert_eq!("FOO_BAR", out.key().as_str());
    assert_eq!("hello world!", out.value());
    assert_eq!(AttributeType::String, out.attribute_type());
    assert_eq!(0, log.messages().len());
}

#[test]
fn test_convert_attribute_invalid() {
    let c = EagleTypeConverter::default();
    let log = MessageLogger::new();
    let xml = "<attribute name=\"!\" value=\"hello world!\"/>";
    let a = model::Attribute::from_dom(&dom(xml), &mut Vec::new()).unwrap();
    assert!(c.try_convert_attribute(&a, &log).is_none());
    assert_eq!(1, log.messages().len());
}

#[test]
fn test_try_convert_schematic_layer() {
    let c = EagleTypeConverter::default();
    assert_eq!(None, c.try_convert_schematic_layer(1)); // tCu
    assert_eq!(
        Some(Layer::SYMBOL_OUTLINES),
        c.try_convert_schematic_layer(94)
    );
    assert_eq!(None, c.try_convert_schematic_layer(999)); // non existent
}

#[test]
fn test_try_convert_board_layer() {
    let c = EagleTypeConverter::default();
    assert_eq!(Some(Layer::TOP_COPPER), c.try_convert_board_layer(1));
    assert_eq!(Layer::inner_copper(2), c.try_convert_board_layer(3));
    assert_eq!(Some(Layer::BOT_COPPER), c.try_convert_board_layer(16));
    assert_eq!(None, c.try_convert_board_layer(94)); // symbols
    assert_eq!(None, c.try_convert_board_layer(999)); // non existent
}

#[test]
fn test_convert_layer_setup() {
    let c = EagleTypeConverter::default();
    let inner = |n| Layer::inner_copper(n).unwrap();
    let top_bot = BTreeMap::from([
        (Layer::TOP_COPPER, Layer::TOP_COPPER),
        (Layer::BOT_COPPER, Layer::BOT_COPPER),
    ]);
    assert_eq!(BTreeMap::new(), c.convert_layer_setup("").unwrap());
    assert_eq!(top_bot, c.convert_layer_setup("1*16").unwrap());
    assert_eq!(top_bot, c.convert_layer_setup("(1*16)").unwrap());
    let mut expected = top_bot.clone();
    expected.extend([
        (inner(1), inner(1)),
        (inner(2), inner(2)),
        (inner(13), inner(3)),
        (inner(14), inner(4)),
    ]);
    assert_eq!(
        expected,
        c.convert_layer_setup("[2:1+((2*3)+(14*15))+16:15]")
            .unwrap()
    );
    let mut expected = top_bot;
    expected.extend((1..=4).map(|i| (inner(i), inner(i))));
    assert_eq!(
        expected,
        c.convert_layer_setup("[2:1+[3:2+(3*4)+5:4]+16:5]").unwrap()
    );
    assert!(c.convert_layer_setup("1*Foo*16").is_err());
}

#[test]
fn test_convert_alignment() {
    let c = EagleTypeConverter::default();
    assert_eq!(
        Alignment::new(HAlign::Right, VAlign::Bottom),
        c.convert_alignment(model::Alignment::BottomRight)
    );
    assert_eq!(
        Alignment::new(HAlign::Center, VAlign::Top),
        c.convert_alignment(model::Alignment::TopCenter)
    );
}

#[test]
fn test_convert_length() {
    let c = EagleTypeConverter::default();
    assert_eq!(Length::new(0), c.convert_length(0.0).unwrap());
    assert_eq!(Length::new(-1234567), c.convert_length(-1.234567).unwrap());
    assert_eq!(Length::new(1234567), c.convert_length(1.234567).unwrap());
}

#[test]
fn test_convert_line_width() {
    let c = EagleTypeConverter::default();
    assert_eq!(ul(0), c.convert_line_width(0.0, 20).unwrap()); // dimension
    assert_eq!(ul(0), c.convert_line_width(0.0, 46).unwrap()); // milling
    assert_eq!(ul(0), c.convert_line_width(1.23, 20).unwrap()); // dimension
    assert_eq!(ul(0), c.convert_line_width(1.23, 46).unwrap()); // milling
    assert_eq!(ul(1230000), c.convert_line_width(1.23, 1).unwrap()); // tCu
    assert_eq!(ul(1230000), c.convert_line_width(1.23, 94).unwrap()); // symbols
    assert!(c.convert_line_width(-1.23, 94).is_err());
}

#[test]
fn test_convert_point() {
    let c = EagleTypeConverter::default();
    assert_eq!(
        nm(0, 0),
        c.convert_point(model::Point { x: 0.0, y: 0.0 }).unwrap()
    );
    assert_eq!(
        nm(-1234567, 1234567),
        c.convert_point(model::Point {
            x: -1.234567,
            y: 1.234567
        })
        .unwrap()
    );
}

#[test]
fn test_convert_angle() {
    let c = EagleTypeConverter::default();
    assert_eq!(Angle::new(0), c.convert_angle(0.0).unwrap());
    assert_eq!(Angle::new(-1234567), c.convert_angle(-1.234567).unwrap());
    assert_eq!(Angle::new(1234567), c.convert_angle(1.234567).unwrap());
}

#[test]
fn test_convert_vertex() {
    let c = EagleTypeConverter::default();
    let v = |xml| model::Vertex::from_dom(&dom(xml)).unwrap();
    assert_eq!(
        Vertex::new(nm(0, 0), Angle::new(0)),
        c.convert_vertex(&v("<vertex x=\"0\" y=\"0\"/>")).unwrap()
    );
    assert_eq!(
        Vertex::new(nm(-6350000, 2540000), Angle::new(90000000)),
        c.convert_vertex(&v("<vertex x=\"-6.35\" y=\"2.54\" curve=\"90\"/>"))
            .unwrap()
    );
}

#[test]
fn test_convert_vertices() {
    let c = EagleTypeConverter::default();
    let vertices: Vec<model::Vertex> = [
        "<vertex x=\"-45.72\" y=\"-5.08\" curve=\"45\"/>",
        "<vertex x=\"-35.56\" y=\"-5.08\"/>",
        "<vertex x=\"-38.1\" y=\"-12.7\"/>",
    ]
    .iter()
    .map(|xml| model::Vertex::from_dom(&dom(xml)).unwrap())
    .collect();
    let expected = path(&[
        (-45720000, -5080000, 45000000),
        (-35560000, -5080000, 0),
        (-38100000, -12700000, 0),
        (-45720000, -5080000, 0),
    ]);
    assert_eq!(expected, c.convert_vertices(&vertices, true).unwrap());
}

#[test]
fn test_convert_and_join_wires() {
    let c = EagleTypeConverter::default();
    let log = MessageLogger::new();
    let wires = [
        wire("<wire x1=\"1\" y1=\"2\" x2=\"3\" y2=\"4\" width=\"0.254\" layer=\"1\"/>"),
        wire("<wire x1=\"3\" y1=\"4\" x2=\"5\" y2=\"6\" width=\"0.254\" layer=\"1\"/>"),
        wire("<wire x1=\"5\" y1=\"6\" x2=\"7\" y2=\"8\" width=\"0.567\" layer=\"1\"/>"),
        wire(
            "<wire x1=\"7\" y1=\"8\" x2=\"9\" y2=\"8\" width=\"0.567\" layer=\"1\" cap=\"flat\"/>",
        ),
        wire("<wire x1=\"7\" y1=\"8\" x2=\"9\" y2=\"9\" width=\"0.567\" layer=\"2\"/>"),
        wire("<wire x1=\"7\" y1=\"8\" x2=\"9\" y2=\"9\" width=\"-1\" layer=\"2\"/>"),
    ];
    let out = c.convert_and_join_wires(&wires, true, &log);
    assert_eq!(4, out.len());
    assert_eq!(1, log.messages().len());

    let g = &out[0];
    assert_eq!(1, g.layer_id);
    assert_eq!(ul(0), g.line_width); // Converted to area
    assert!(g.filled);
    assert!(g.grab_area);
    assert_eq!(
        path(&[
            (7000000, 8283500, 0),
            (9000000, 8283500, 0),
            (9000000, 7716500, 0),
            (7000000, 7716500, 0),
            (7000000, 8283500, 0),
        ]),
        g.path
    );

    let g = &out[1];
    assert_eq!(1, g.layer_id);
    assert_eq!(ul(254000), g.line_width);
    assert!(!g.filled);
    assert!(!g.grab_area);
    assert_eq!(
        path(&[
            (1000000, 2000000, 0),
            (3000000, 4000000, 0),
            (5000000, 6000000, 0)
        ]),
        g.path
    );

    let g = &out[2];
    assert_eq!(1, g.layer_id);
    assert_eq!(ul(567000), g.line_width);
    assert!(!g.filled);
    assert!(!g.grab_area);
    assert_eq!(
        path(&[(5000000, 6000000, 0), (7000000, 8000000, 0)]),
        g.path
    );

    let g = &out[3];
    assert_eq!(2, g.layer_id);
    assert_eq!(ul(567000), g.line_width);
    assert!(!g.filled);
    assert!(!g.grab_area);
    assert_eq!(
        path(&[(7000000, 8000000, 0), (9000000, 9000000, 0)]),
        g.path
    );
}

#[test]
fn test_convert_rectangle() {
    let c = EagleTypeConverter::default();
    let xml = "<rectangle x1=\"1\" y1=\"2\" x2=\"4\" y2=\"3\" layer=\"1\"/>";
    let out = c
        .convert_rectangle(&model::Rectangle::from_dom(&dom(xml)).unwrap(), true)
        .unwrap();
    assert_eq!(1, out.layer_id);
    assert_eq!(ul(0), out.line_width);
    assert!(out.filled); // EAGLE rectangles are always filled.
    assert!(out.grab_area); // Passed to function under test.
    assert_eq!(
        path(&[
            (1000000, 2000000, 0),
            (4000000, 2000000, 0),
            (4000000, 3000000, 0),
            (1000000, 3000000, 0),
            (1000000, 2000000, 0),
        ]),
        out.path
    );
    assert_eq!(None, out.circle);
}

#[test]
fn test_convert_rectangle_rotated() {
    let c = EagleTypeConverter::default();
    let xml = "<rectangle x1=\"1\" y1=\"2\" x2=\"4\" y2=\"3\" layer=\"1\" rot=\"R90\"/>";
    let out = c
        .convert_rectangle(&model::Rectangle::from_dom(&dom(xml)).unwrap(), false)
        .unwrap();
    assert_eq!(1, out.layer_id);
    assert_eq!(ul(0), out.line_width);
    assert!(out.filled);
    assert!(!out.grab_area);
    assert_eq!(
        path(&[
            (3000000, 1000000, 0),
            (3000000, 4000000, 0),
            (2000000, 4000000, 0),
            (2000000, 1000000, 0),
            (3000000, 1000000, 0),
        ]),
        out.path
    );
    assert_eq!(None, out.circle);
}

#[test]
fn test_convert_polygon() {
    let c = EagleTypeConverter::default();
    let xml = "<polygon width=\"2.54\" layer=\"1\"><vertex x=\"1\" y=\"2\" curve=\"45\"/>\
               <vertex x=\"3\" y=\"4\"/></polygon>";
    let p = model::Polygon::from_dom(&dom(xml), &mut Vec::new()).unwrap();
    let out = c.convert_polygon(&p, false).unwrap();
    assert_eq!(1, out.layer_id);
    assert_eq!(ul(2540000), out.line_width);
    assert!(out.filled); // EAGLE polygons are always filled.
    assert!(!out.grab_area);
    assert_eq!(
        path(&[
            (1000000, 2000000, 45000000),
            (3000000, 4000000, 0),
            (1000000, 2000000, 0),
        ]),
        out.path
    );
    assert_eq!(None, out.circle);
}

#[test]
fn test_convert_circle() {
    let c = EagleTypeConverter::default();
    for (width, filled, line_width) in [("0.254", false, 254000), ("0", true, 0)] {
        let xml = format!("<circle x=\"1\" y=\"2\" radius=\"3.5\" width=\"{width}\" layer=\"1\"/>");
        let out = c
            .convert_circle(&model::Circle::from_dom(&dom(&xml)).unwrap(), !filled)
            .unwrap();
        assert_eq!(1, out.layer_id);
        assert_eq!(ul(line_width), out.line_width);
        assert_eq!(filled, out.filled); // Filled if line width == 0.
        assert_eq!(!filled, out.grab_area);
        assert_eq!(
            path(&[
                (4500000, 2000000, -180000000),
                (-2500000, 2000000, -180000000),
                (4500000, 2000000, 0),
            ]),
            out.path
        );
        assert_eq!(Some((nm(1000000, 2000000), pl(7000000))), out.circle);
    }
}

#[test]
fn test_convert_hole() {
    let c = EagleTypeConverter::default();
    let xml = "<hole x=\"1\" y=\"2\" drill=\"3.5\"/>";
    let out = c
        .convert_hole(&model::Hole::from_dom(&dom(xml)).unwrap())
        .unwrap();
    assert_eq!(pl(3500000), out.diameter());
    assert_eq!(1, out.path().get().vertices().len());
    assert_eq!(nm(1000000, 2000000), out.path().get().vertices()[0].pos);
}

#[test]
fn test_convert_frame() {
    let c = EagleTypeConverter::default();
    let xml =
        "<frame x1=\"10\" y1=\"20\" x2=\"40\" y2=\"30\" columns=\"6\" rows=\"4\" layer=\"94\"/>";
    let out = c
        .convert_frame(&model::Frame::from_dom(&dom(xml)).unwrap())
        .unwrap();
    assert_eq!(94, out.layer_id);
    assert_eq!(ul(200000), out.line_width);
    assert!(!out.filled);
    assert!(!out.grab_area);
    assert_eq!(
        path(&[
            (13810000, 23810000, 0),
            (36190000, 23810000, 0),
            (36190000, 26190000, 0),
            (13810000, 26190000, 0),
            (13810000, 23810000, 0),
        ]),
        out.path
    );
    assert_eq!(None, out.circle);
}

#[test]
fn test_convert_text_value() {
    let c = EagleTypeConverter::default();
    assert_eq!("", c.convert_text_value(""));
    assert_eq!("{{NAME}}", c.convert_text_value(">NAME"));
    assert_eq!("{{VALUE}}", c.convert_text_value(">VALUE"));
    assert_eq!("Some Text", c.convert_text_value("Some Text"));
}

#[test]
fn test_try_convert_schematic_text_size() {
    let c = EagleTypeConverter::default();
    assert_eq!(pl(2500000), c.convert_schematic_text_size(1.778).unwrap());
}

#[test]
fn test_try_convert_schematic_text() {
    let c = EagleTypeConverter::default();
    let xml = "<text x=\"1\" y=\"2\" size=\"1.778\" layer=\"94\">foo\nbar</text>";
    let t = model::Text::from_dom(&dom(xml), &mut Vec::new()).unwrap();
    let out = c.try_convert_schematic_text(&t, true).unwrap().unwrap();
    assert_eq!(Layer::SYMBOL_OUTLINES, out.layer());
    assert_eq!(nm(1000000, 2000000), out.position());
    assert_eq!(Angle::new(0), out.rotation());
    assert_eq!(pl(2500000), out.height()); // Scaled.
    assert_eq!(Alignment::new(HAlign::Left, VAlign::Bottom), out.align());
    assert_eq!("foo\nbar", out.text());
    assert!(out.locked()); // Because of the layer.
}

#[test]
fn test_try_convert_board_text_size() {
    let c = EagleTypeConverter::default();
    assert_eq!(pl(1700000), c.convert_board_text_size(1, 2.0).unwrap());
}

#[test]
fn test_try_convert_board_text_stroke_width() {
    let c = EagleTypeConverter::default();
    assert_eq!(
        ul(1050000),
        c.convert_board_text_stroke_width(1, 2.5, 42).unwrap()
    );
    // It seems the ratio is sometimes not defined and is thus set to 0%. In
    // this case, we fall back to a default ratio of 15%.
    assert_eq!(
        ul(375000),
        c.convert_board_text_stroke_width(1, 2.5, 0).unwrap()
    );
}

#[test]
fn test_try_convert_board_text() {
    let c = EagleTypeConverter::default();
    let xml = "<text x=\"1\" y=\"2\" size=\"3\" layer=\"1\">&gt;NAME</text>";
    let t = model::Text::from_dom(&dom(xml), &mut Vec::new()).unwrap();
    let out = c.try_convert_board_text(&t, true).unwrap().unwrap();
    assert_eq!(Layer::TOP_COPPER, out.layer());
    assert_eq!(nm(1000000, 2000000), out.position());
    assert_eq!(Angle::new(0), out.rotation());
    assert_eq!(pl(2550000), out.height()); // Scaled.
    assert_eq!(ul(240000), out.stroke_width()); // Default ratio.
    assert_eq!(StrokeTextSpacing::default(), out.letter_spacing());
    assert_eq!(StrokeTextSpacing::default(), out.line_spacing());
    assert_eq!(Alignment::new(HAlign::Left, VAlign::Bottom), out.align());
    assert!(!out.mirrored());
    assert!(out.auto_rotate());
    assert_eq!("{{NAME}}", out.text());
    assert!(out.locked()); // Because of the layer.
}

#[test]
fn test_convert_symbol_pin() {
    let c = EagleTypeConverter::default();
    let xml = "<pin name=\"P$1\" x=\"1\" y=\"2\" length=\"point\"/>";
    let out = c
        .convert_symbol_pin(&model::Pin::from_dom(&dom(xml), &mut Vec::new()).unwrap())
        .unwrap();
    assert_eq!("1", out.pin.name().as_str());
    assert_eq!(nm(1000000, 2000000), out.pin.position());
    assert_eq!(ul(0), out.pin.length());
    assert_eq!(Angle::new(0), out.pin.rotation());
    assert!(out.circle.is_none());
    assert!(out.polygon.is_none());
}

#[test]
fn test_convert_symbol_pin_rotated() {
    let c = EagleTypeConverter::default();
    let xml = "<pin name=\"P$1\" x=\"1\" y=\"2\" length=\"middle\" rot=\"R90\"/>";
    let out = c
        .convert_symbol_pin(&model::Pin::from_dom(&dom(xml), &mut Vec::new()).unwrap())
        .unwrap();
    assert_eq!("1", out.pin.name().as_str());
    assert_eq!(nm(1000000, 2000000), out.pin.position());
    assert_eq!(ul(5080000), out.pin.length());
    assert_eq!(Angle::new(90000000), out.pin.rotation());
    assert!(out.circle.is_none());
    assert!(out.polygon.is_none());
}

fn tht_pad(xml: &str) -> model::ThtPad {
    model::ThtPad::from_dom(&dom(xml), &mut Vec::new()).unwrap()
}

#[test]
fn test_convert_tht_pad() {
    let c = EagleTypeConverter::default();
    let xml = "<pad name=\"P$1\" x=\"1\" y=\"2\" drill=\"1.5\" shape=\"square\"/>";
    let (pkg_pad, fpt_pad) = c
        .convert_tht_pad(
            &tht_pad(xml),
            &EagleTypeConverter::default_auto_tht_annular_width(),
        )
        .unwrap();
    let pad = fpt_pad.pad();
    assert_eq!("1", pkg_pad.name().as_str());
    assert_eq!(Some(pkg_pad.uuid()), fpt_pad.package_pad_uuid());
    assert_eq!(nm(1000000, 2000000), pad.position());
    assert_eq!(Angle::new(0), pad.rotation());
    assert_eq!(PadShape::RoundedRect, pad.shape());
    assert_eq!(pl(2250000), pad.width()); // 1.5*drill
    assert_eq!(pl(2250000), pad.height()); // 1.5*drill
    assert_eq!(ComponentSide::Top, pad.component_side());
    assert_eq!(1, pad.holes().len());
    assert_eq!(pl(1500000), pad.holes().as_slice()[0].diameter());
}

#[test]
fn test_convert_tht_pad_rotated() {
    let c = EagleTypeConverter::default();
    let xml = "<pad name=\"P$1\" x=\"1\" y=\"2\" drill=\"1.5\" diameter=\"2.54\" \
               shape=\"octagon\" rot=\"R90\"/>";
    let (pkg_pad, fpt_pad) = c
        .convert_tht_pad(
            &tht_pad(xml),
            &EagleTypeConverter::default_auto_tht_annular_width(),
        )
        .unwrap();
    let pad = fpt_pad.pad();
    assert_eq!("1", pkg_pad.name().as_str());
    assert_eq!(Some(pkg_pad.uuid()), fpt_pad.package_pad_uuid());
    assert_eq!(nm(1000000, 2000000), pad.position());
    assert_eq!(Angle::new(90000000), pad.rotation());
    assert_eq!(PadShape::RoundedOctagon, pad.shape());
    assert_eq!(pl(2540000), pad.width());
    assert_eq!(pl(2540000), pad.height());
    assert_eq!(ComponentSide::Top, pad.component_side());
    assert_eq!(1, pad.holes().len());
    assert_eq!(pl(1500000), pad.holes().as_slice()[0].diameter());
}

#[test]
fn test_convert_smt_pad() {
    let c = EagleTypeConverter::default();
    for (layer, rot, rotation, side) in [
        ("1", "", 0, ComponentSide::Top),
        ("16", " rot=\"R90\"", 90000000, ComponentSide::Bottom),
    ] {
        let xml =
            format!("<smd name=\"P$1\" x=\"1\" y=\"2\" dx=\"3\" dy=\"4\" layer=\"{layer}\"{rot}/>");
        let (pkg_pad, fpt_pad) = c
            .convert_smt_pad(&model::SmtPad::from_dom(&dom(&xml)).unwrap())
            .unwrap();
        let pad = fpt_pad.pad();
        assert_eq!("1", pkg_pad.name().as_str());
        assert_eq!(Some(pkg_pad.uuid()), fpt_pad.package_pad_uuid());
        assert_eq!(nm(1000000, 2000000), pad.position());
        assert_eq!(Angle::new(rotation), pad.rotation());
        assert_eq!(PadShape::RoundedRect, pad.shape());
        assert_eq!(pl(3000000), pad.width());
        assert_eq!(pl(4000000), pad.height());
        assert_eq!(side, pad.component_side());
        assert_eq!(0, pad.holes().len());
    }
}
