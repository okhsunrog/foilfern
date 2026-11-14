//! Port of apps/librepcb-cli/commandlineinterface.cpp's openStep().
//! Uses the independent pure-Rust kernel, without a window system or GPU.
use crate::{
    args::OpenStepArgs,
    error::CliResult as Result,
    output::{absolute_path, pretty_path},
};
use foilfern_core::fileio::file_utils;
use foilfern_step::{StepModel, minify_step};

fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(digit);
    }
    output
}

pub fn run(options: &OpenStepArgs) -> Result<bool> {
    let file = options
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    let path = absolute_path(file);
    println!("Open STEP file '{}'...", pretty_path(&path, file));
    let mut bytes = file_utils::read_file(&path)?;
    let mut success = true;
    if options.minify {
        println!("Perform minify...");
        let minified = minify_step(&bytes)?;
        if minified != bytes {
            let percent = 100. * (minified.len() as f64 - bytes.len() as f64) / bytes.len() as f64;
            println!(
                " - Minified from {} bytes to {} bytes ({:.0}%)",
                grouped(bytes.len()),
                grouped(minified.len()),
                percent.round()
            );
            if minified.len() > bytes.len() {
                eprintln!(" - ERROR: The output is larger than the input!");
                success = false;
            }
            bytes = minified;
        } else {
            println!(" - File is already minified");
        }
    }
    // Upstream deliberately saves before validation so invalid input can be
    // inspected. FileUtils retains its atomic replacement semantics.
    if let Some(destination) = &options.save_to {
        let path = absolute_path(destination);
        println!("Save to '{}'...", pretty_path(&path, destination));
        file_utils::write_file(&path, &bytes)?;
    }
    println!("Load model...");
    let model = StepModel::from_step(&bytes)?;
    if options.tesselate {
        println!("Tesselate model...");
        let stats = model.tessellation_stats(0.01)?;
        println!(
            " - Built {} vertices with {} different colors",
            stats.vertices, stats.colors
        );
        if stats.vertices == 0 {
            eprintln!(" - ERROR: No content found in model!");
            return Ok(false);
        }
    }
    Ok(success)
}
