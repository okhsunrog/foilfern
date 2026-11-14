//! Port of tests/unittests/core/attribute/attributesubstitutortest.cpp.

use foilfern_core::attribute::substitute;

fn lookup(key: &str) -> Option<String> {
    let value = match key {
        "KEY" => "",
        "KEY_1" => "Normal value",
        "KEY_2" => "Value with {}}}{{ noise",
        "KEY_3" => "Recursive {{UNDEFINED}} value",
        "KEY_4" => "Recursive {{KEY_1}} value",
        "KEY_5" => "Recursive {{KEY_4}} value",
        "KEY_6" => "Endless {{KEY_7}} part 1",
        "KEY_7" => "Endless {{KEY_6}} part 2",
        "KEY_8" => "{{KEY}}",
        _ => return None,
    };
    Some(value.to_owned())
}

// Disabled upstream test cases (bugs of the substitutor, same here):
//   ("{{KEY_1}} {{KEY_1}}", "Normal value Normal value"),
//   ("{{KEY_8 or KEY_1}}", "Normal value"),
//   ("{{KEY or KEY_4 or KEY_3}} {{KEY_1}}", "Recursive Normal value value Normal value"),
//   ("{{KEY_1}} {{FOO or KEY or KEY_5}}!", "Normal value Recursive Recursive Normal value value value!"),
#[rustfmt::skip]
const DATA: &[(&str, &str)] = &[
    ("",                                   ""),
    ("Hello { World! }} {{",               "Hello { World! }} {{"),
    ("{{NONEXISTENT}}",                    ""),
    ("{{KEY}}",                            ""),
    ("{{KEY_1}}",                          "Normal value"),
    ("some {}}}{{ noise",                  "some {}}}{{ noise"),
    ("{{KEY_2}}",                          "Value with {}}}{{ noise"),
    ("{{KEY_3}}",                          "Recursive value"),
    ("{{KEY_4}}",                          "Recursive Normal value value"),
    ("{{KEY_5}}",                          "Recursive Recursive Normal value value value"),
    ("{{KEY_6}}",                          "Endless Endless part 2 part 1"),
    ("{{KEY_7}}",                          "Endless Endless part 1 part 2"),
    ("Foo {KEY_7 }}{{KEY_7}} {{KEYY}}",    "Foo {KEY_7 }}Endless Endless part 1 part 2"),
    ("{{KEY_3}} foo{ { KEY_5}} {{KEY}}",   "Recursive value foo{ { KEY_5}}"),
    ("{{KEY_1}} {{KEY_2 or KEY_3}} foo",   "Normal value Value with {}}}{{ noise foo"),
    ("{{FOO or BAR or BAR or FOO}}",       ""),
    ("{{FOO or BAR or KEY or KEY_1}}",     "Normal value"),
    ("{{FOO or 'a literal!' or KEY_1}}",   "a literal!"),
    ("{{FOO or KEY_1 or 'literal 2!'}}",   "Normal value"),
    ("{{ '{{' }}",                         "{{"),
    ("{{ '}}' }}",                         "}}"),
    ("{{KEY_1}}KEY_2",                     "Normal valueKEY_2"),
    ("{{KEY_1 or FOO}} or KEY_1",          "Normal value or KEY_1"),
    // Whitespace trimming
    ("{{KEY_1}} {{KEY_8}} foo",            "Normal value foo"),
    ("{{KEY_1}} {{NONEXISTENT}} foo",      "Normal value foo"),
    ("{{NONEXISTENT}} {{KEY_1}} foo",      "Normal value foo"),
    ("{{KEY}} {{KEY_1}} foo",              "Normal value foo"),
    ("{{KEY}} {{KEY_1}} {{KEY}}",          "Normal value"),
    ("{{KEY}} {{KEY}} {{KEY}}",            ""),
    ("{{NONEXISTENT}} {{NONEXISTENT}}",    ""),
    // Newline trimming
    ("{{KEY_1}}\n{{KEY_8}}\nfoo",          "Normal value\nfoo"),
    ("{{KEY_1}}\n{{NONEXISTENT}}\nfoo",    "Normal value\nfoo"),
    ("{{NONEXISTENT}}\n{{KEY_1}}\nfoo",    "Normal value\nfoo"),
    ("{{KEY}}\n{{KEY_1}}\nfoo",            "Normal value\nfoo"),
    ("{{KEY}}\n{{KEY_1}}\n{{KEY}}",        "Normal value"),
    ("{{KEY}}\n{{KEY}}\n{{KEY}}",          ""),
    ("{{NONEXISTENT}}\n{{NONEXISTENT}}",   ""),
    ("Foo\n\nBar",                         "Foo\n\nBar"),
];

#[test]
fn test_data() {
    for &(input, output) in DATA {
        assert_eq!(substitute(input, lookup, None), output, "{input:?}");
    }
}
