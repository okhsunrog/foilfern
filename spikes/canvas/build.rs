fn main() {
    slint_build::compile_with_config(
        "ui/canvas.slint",
        slint_build::CompilerConfiguration::new().with_edition((1, 20)),
    )
    .expect("slint build failed");
}
