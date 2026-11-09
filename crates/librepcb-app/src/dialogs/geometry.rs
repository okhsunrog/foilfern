//! Form fields shared by the geometry dialogs: layers, alignment and path
//! vertices.
//!
//! Port of the parts of libs/librepcb/editor/widgets/layercombobox.{h,cpp},
//! alignment selectors and libs/librepcb/editor/widgets/patheditorwidget.{h,cpp}
//! used by the properties dialogs. The path editor is a list of vertices
//! with X, Y and arc angle; adding or removing vertices is not supported
//! (the editors' context menus do that).

use librepcb_core::geometry::{Path, Vertex};
use librepcb_core::types::{Alignment, HAlign, Layer, Length, Point, VAlign};
use librepcb_i18n::tr;

use super::Form;

/// Adds a layer combo box with `layers` (the current layer is added if
/// missing); the chosen layer is read with [`chosen_layer()`].
pub fn layer_field(form: &mut Form, id: &str, label: &str, layers: &[Layer], current: Layer) {
    let mut layers = layers.to_vec();
    if !layers.contains(&current) {
        layers.push(current);
    }
    let names: Vec<String> = layers.iter().map(|l| l.name_tr()).collect();
    let index = layers.iter().position(|l| *l == current);
    form.choice(id, label, &names, index);
    form.update(id, |f| {
        f.placeholder = layers
            .iter()
            .map(|l| l.id())
            .collect::<Vec<_>>()
            .join(",")
            .into();
    });
}

/// The layer chosen in a field added with [`layer_field()`].
pub fn chosen_layer(form: &Form, id: &str) -> Option<Layer> {
    let field = form.field(id)?;
    let index = usize::try_from(field.index).ok()?;
    field
        .placeholder
        .split(',')
        .nth(index)
        .and_then(Layer::from_id)
}

/// The index of a horizontal alignment for the selector.
pub fn halign_index(a: HAlign) -> usize {
    match a {
        HAlign::Left => 0,
        HAlign::Center => 1,
        HAlign::Right => 2,
    }
}

/// The index of a vertical alignment for the selector (bottom, center,
/// top like upstream's selector).
pub fn valign_index(a: VAlign) -> usize {
    match a {
        VAlign::Bottom => 0,
        VAlign::Center => 1,
        VAlign::Top => 2,
    }
}

/// Adds horizontal and vertical alignment selectors.
pub fn alignment_fields(form: &mut Form, label: &str, align: Alignment) {
    form.alignment("halign", label, false, halign_index(align.h));
    form.alignment("valign", "", true, valign_index(align.v));
}

/// The alignment chosen in the fields of [`alignment_fields()`].
pub fn chosen_alignment(form: &Form) -> Alignment {
    let h = match form.get_index("halign") {
        Some(0) => HAlign::Left,
        Some(1) => HAlign::Center,
        _ => HAlign::Right,
    };
    let v = match form.get_index("valign") {
        Some(0) => VAlign::Bottom,
        Some(1) => VAlign::Center,
        _ => VAlign::Top,
    };
    Alignment::new(h, v)
}

/// Adds position fields ("pos_x", "pos_y").
pub fn position_fields(form: &mut Form, label_x: &str, label_y: &str, pos: Point) {
    form.length("pos_x", label_x, pos.x, Length::MIN);
    form.length("pos_y", label_y, pos.y, Length::MIN);
}

/// The position of the fields of [`position_fields()`].
pub fn chosen_position(form: &Form) -> Point {
    Point::new(form.get_length("pos_x"), form.get_length("pos_y"))
}

/// Adds the vertices of a path (upstream `PathEditorWidget`).
pub fn path_fields(form: &mut Form, path: &Path) {
    form.header(tr!("librepcb::editor::PathEditorWidget", "Vertices"));
    for (i, v) in path.vertices().iter().enumerate() {
        let n = i + 1;
        form.length(
            &format!("vertex_{i}_x"),
            format!("#{n} X:"),
            v.pos.x,
            Length::MIN,
        );
        form.length(&format!("vertex_{i}_y"), "Y:", v.pos.y, Length::MIN);
        form.angle(
            &format!("vertex_{i}_angle"),
            tr!("librepcb::editor::PathModel", "Angle"),
            v.angle,
        );
    }
}

/// The path edited with [`path_fields()`] (`original` gives the vertex
/// count).
pub fn chosen_path(form: &Form, original: &Path) -> Path {
    let vertices = (0..original.vertices().len())
        .map(|i| {
            Vertex::new(
                Point::new(
                    form.get_length(&format!("vertex_{i}_x")),
                    form.get_length(&format!("vertex_{i}_y")),
                ),
                form.get_angle(&format!("vertex_{i}_angle")),
            )
        })
        .collect();
    Path::new(vertices)
}
