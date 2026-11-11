//! Ports of tests/unittests/core/utils/*.cpp.

mod clipper_helpers_test;
mod math_parser_test;
mod overline_markup_parser_test;
mod tangent_path_joiner_test;
mod toolbox_test;
mod transform_test;

#[cfg(feature = "experimental-exact-copper")]
mod rational_union_test;
