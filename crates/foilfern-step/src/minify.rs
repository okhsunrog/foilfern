//! Port of libs/librepcb/core/3d/occmodel.cpp's minifyStep().
//! The existing step-p21 AST and CAD writers do not reproduce upstream's
//! lexical layout or selective fixed-point deduplication. This adapter keeps
//! header/footer and entity values instead of round-tripping through BReps.
//! Unlike upstream's regular expressions, references and negative zero are
//! recognized only outside quoted literals; comments cannot split entities.
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("STEP data section not found.")]
    MissingData,
    #[error("STEP data section end not found.")]
    MissingEnd,
    #[error("Failed to parse data section of STEP file.")]
    InvalidData,
    #[error("Duplicate STEP entity identifier.")]
    DuplicateId,
    #[error("Unresolved STEP entity reference.")]
    MissingReference,
    #[error("Multiple STEP data sections are not supported by minification.")]
    MultipleSections,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Value {
    fragments: Vec<Vec<u8>>,
    references: Vec<u64>,
    /// Product/representation identities must survive, even when identical.
    identity: Option<u64>,
}

fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &bytes[start..end]
}

fn identifier(bytes: &[u8]) -> Result<u64, Error> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(Error::InvalidData);
    }
    std::str::from_utf8(bytes)
        .map_err(|_| Error::InvalidData)?
        .parse()
        .map_err(|_| Error::InvalidData)
}

/// Index immediately after a quoted literal. STEP escapes apostrophes by doubling.
fn literal_end(bytes: &[u8], start: usize) -> Result<usize, Error> {
    let delimiter = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == delimiter {
            if delimiter == b'\'' && bytes.get(index + 1) == Some(&delimiter) {
                index += 2;
                continue;
            }
            return Ok(index + 1);
        }
        index += 1;
    }
    Err(Error::InvalidData)
}

fn clean_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut quote = None;
    let mut comment = false;
    for raw in bytes.split_inclusive(|b| *b == b'\n') {
        let mut line = Vec::new();
        let mut index = 0;
        while index < raw.len() && raw[index] != b'\n' {
            let byte = raw[index];
            if let Some(delimiter) = quote {
                line.push((byte, true));
                if byte == delimiter {
                    if delimiter == b'\'' && raw.get(index + 1) == Some(&delimiter) {
                        line.push((delimiter, true));
                        index += 1;
                    } else {
                        quote = None;
                    }
                }
            } else {
                if comment {
                    if raw.get(index..index + 2) == Some(b"*/") {
                        line.extend([(b'*', false), (b'/', false)]);
                        index += 2;
                        comment = false;
                        continue;
                    }
                } else if raw.get(index..index + 2) == Some(b"/*") {
                    comment = true;
                } else if matches!(byte, b'\'' | b'"') {
                    quote = Some(byte);
                }
                if byte != b'\r' {
                    line.push((byte, quote.is_some()));
                }
            }
            index += 1;
        }
        let whitespace = |(byte, literal): &(u8, bool)| !literal && byte.is_ascii_whitespace();
        let first = line
            .iter()
            .position(|b| !whitespace(b))
            .unwrap_or(line.len());
        let start = if line.get(first) == Some(&(b'*', false)) {
            0
        } else {
            first
        };
        let mut end = line
            .iter()
            .rposition(|b| !whitespace(b))
            .map_or(start, |i| i + 1);
        while end >= start + 2 && line[end - 2..end] == [(b' ', false), (b';', false)] {
            line.remove(end - 2);
            end -= 1;
        }
        let mut line: Vec<_> = line[start..end].iter().map(|(byte, _)| *byte).collect();
        // Layout newlines are unwrapped; literal newlines remain part of the
        // value, including any leading/trailing whitespace on the next line.
        if quote.is_some() && raw.last() == Some(&b'\n') {
            line.push(b'\n');
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

fn value(bytes: &[u8], id: u64) -> Result<Value, Error> {
    let mut result = Value {
        fragments: Vec::new(),
        references: Vec::new(),
        identity: None,
    };
    let mut fragment = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => {
                let end = literal_end(bytes, index)?;
                fragment.extend_from_slice(&bytes[index..end]);
                index = end;
            }
            b'#' => {
                let start = index + 1;
                index = start;
                while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                    index += 1;
                }
                result.references.push(identifier(&bytes[start..index])?);
                fragment.push(b'#');
                result.fragments.push(std::mem::take(&mut fragment));
            }
            b'-' if bytes.get(index..index + 3) == Some(b"-0.")
                && bytes.get(index + 3).is_some_and(|b| !b.is_ascii_digit()) =>
            {
                fragment.extend_from_slice(b"0.");
                index += 3;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = index;
                while bytes
                    .get(index)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    index += 1;
                }
                let word = &bytes[start..index];
                if word
                    .windows(b"PRODUCT_DEFINITION".len())
                    .any(|s| s == b"PRODUCT_DEFINITION")
                    || word
                        .windows(b"REPRESENTATION".len())
                        .any(|s| s == b"REPRESENTATION")
                {
                    result.identity = Some(id);
                }
                fragment.extend_from_slice(word);
            }
            byte => {
                fragment.push(byte);
                index += 1;
            }
        }
    }
    result.fragments.push(fragment);
    Ok(result)
}

/// Remove layout and repeated entity values while retaining all representations,
/// product definitions, references, units, styles and header/footer records.
/// Does not validate BRep geometry; callers validate before committing to a library.
pub fn minify_step(bytes: &[u8]) -> crate::Result<Vec<u8>> {
    if bytes.len() > 256 * 1024 * 1024 {
        return Err(crate::Error::InputLimit);
    }
    let lines = clean_lines(bytes);
    let start = lines
        .iter()
        .position(|s| s == b"DATA;")
        .ok_or(Error::MissingData)?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(i, s)| (s == b"ENDSEC;").then_some(i))
        .ok_or(Error::MissingEnd)?;
    super::check_input(bytes)?;
    if lines[end + 1..].iter().any(|s| s == b"DATA;") {
        return Err(Error::MultipleSections.into());
    }
    let body: Vec<_> = lines[start + 1..end].concat();
    let mut data = BTreeMap::new();
    let mut record = Vec::new();
    let mut index = 0;
    while index < body.len() {
        if matches!(body[index], b'\'' | b'"') {
            let end = literal_end(&body, index)?;
            record.extend_from_slice(&body[index..end]);
            index = end;
        } else if body.get(index..index + 2) == Some(b"/*") {
            let end = body[index + 2..]
                .windows(2)
                .position(|s| s == b"*/")
                .ok_or(Error::InvalidData)?;
            record.push(b' ');
            index += end + 4;
        } else if body[index] == b';' {
            let statement = trim(&record);
            let equal = statement
                .iter()
                .position(|b| *b == b'=')
                .ok_or(Error::InvalidData)?;
            if statement.first() != Some(&b'#') {
                return Err(Error::InvalidData.into());
            }
            let id = identifier(trim(&statement[1..equal]))?;
            let text = trim(&statement[equal + 1..]);
            if id == 0 || text.is_empty() {
                return Err(Error::InvalidData.into());
            }
            if data.insert(id, value(text, id)?).is_some() {
                return Err(Error::DuplicateId.into());
            }
            record.clear();
            index += 1;
        } else {
            record.push(body[index]);
            index += 1;
        }
    }
    if !trim(&record).is_empty() {
        return Err(Error::InvalidData.into());
    }
    for value in data.values() {
        if value.references.iter().any(|id| !data.contains_key(id)) {
            return Err(Error::MissingReference.into());
        }
    }
    loop {
        let mut unique = HashMap::new();
        let mut mapping = BTreeMap::new();
        for (&id, value) in &data {
            let next = unique.len() as u64 + 1;
            mapping.insert(id, *unique.entry(value.clone()).or_insert(next));
        }
        if unique.len() == data.len() {
            break;
        }
        data = unique
            .into_iter()
            .map(|(mut value, id)| {
                for reference in &mut value.references {
                    // Every reference was checked against the complete input graph.
                    *reference = mapping[reference];
                }
                (id, value)
            })
            .collect();
    }
    let mut output = Vec::new();
    for line in &lines[..=start] {
        output.extend_from_slice(line);
        if !line.ends_with(b"\n") {
            output.push(b'\n');
        }
    }
    for (id, value) in data {
        output.extend_from_slice(format!("#{id}=").as_bytes());
        for (index, fragment) in value.fragments.iter().enumerate() {
            output.extend_from_slice(fragment);
            if let Some(reference) = value.references.get(index) {
                output.extend_from_slice(reference.to_string().as_bytes());
            }
        }
        output.extend_from_slice(b";\n");
    }
    for line in &lines[end..] {
        output.extend_from_slice(line);
        if !line.ends_with(b"\n") {
            output.push(b'\n');
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_fixed_point_vector_preserves_representation_identity() {
        let input = b"header;\nDATA;\n#1 = 42;\n#2 = 42;\n#3 = FOO(#1, #2);\n#4 = FOO(#2, #1);\n#5 = PRODUCT_DEFINITION(#2, #3);\n#6 = PRODUCT_DEFINITION(#2, #3);\n#7 = SHAPE_REPRESENTATION(#2, #3);\n#8 = SHAPE_REPRESENTATION(#2, #3);\n#9 = ANYREPRESENTATION(#2, #3);\n#10 = ANYREPRESENTATION(#2, #3);\nENDSEC;\nfooter;\n";
        let expected = b"header;\nDATA;\n#1=42;\n#2=FOO(#1, #1);\n#3=PRODUCT_DEFINITION(#1, #2);\n#4=PRODUCT_DEFINITION(#1, #2);\n#5=SHAPE_REPRESENTATION(#1, #2);\n#6=SHAPE_REPRESENTATION(#1, #2);\n#7=ANYREPRESENTATION(#1, #2);\n#8=ANYREPRESENTATION(#1, #2);\nENDSEC;\nfooter;\n";
        assert_eq!(minify_step(input).unwrap(), expected);
        assert_eq!(minify_step(expected).unwrap(), expected);
    }
    #[test]
    fn strings_comments_cycles_and_invalid_graphs() {
        let input = b"DATA;\n/* comment #99; */ #1=42;\n#2=42;\n#3=FOO('O''Brien; #2 -0.',#2,-0.,-0.1);\n#4=BAR(#4);\nENDSEC;\n";
        let output = minify_step(input).unwrap();
        assert_eq!(
            output,
            b"DATA;\n#1=42;\n#2=FOO('O''Brien; #2 -0.',#1,0.,-0.1);\n#3=BAR(#3);\nENDSEC;\n"
        );
        let multiline = b"HEADER;\nNAME('keep \n  spaces');\nENDSEC;\nDATA;\n#1=FOO('keep ;\n  #99 -0.\n  spaces');\nENDSEC;\n";
        assert_eq!(minify_step(multiline).unwrap(), multiline);
        for invalid in [
            b"".as_slice(),
            b"DATA;",
            b"DATA;\n#1=1;#1=2;\nENDSEC;",
            b"DATA;\n#1=FOO(#2);\nENDSEC;",
            b"DATA;\n#0=1;\nENDSEC;",
            b"DATA;\n#1=1\nENDSEC;",
            b"DATA;\nENDSEC;\nDATA;\nENDSEC;",
        ] {
            assert!(minify_step(invalid).is_err(), "{invalid:?}");
        }
    }
}
