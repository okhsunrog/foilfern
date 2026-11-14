//! Port of tests/unittests/core/types/uuidtest.cpp.

use foilfern_core::serialization::{FromSExpression, SExpression, ToSExpression};
use foilfern_core::types::Uuid;

/// (valid, uuid)
#[rustfmt::skip]
const DATA: &[(bool, &str)] = &[
    // DCE Version 4 (random, the only accepted UUID type for us)
    (true , "bdf7bea5-b88e-41b2-be85-c1604e8ddfca"  ),
    (true , "587539af-1c39-40ed-9bdd-2ca2e6aeb18d"  ),
    (true , "27556d27-fe33-4334-a8ee-b05b402a21d6"  ),
    (true , "91172d44-bdcc-41b2-8e07-4f8cf44eb108"  ),
    (true , "ecb3a5fe-1cbc-4a1b-bf8f-5d6e26deaee1"  ),
    (true , "908f9c33-40be-46aa-97b4-be2cd7477881"  ),
    (true , "74ca6127-e785-4355-8580-1ced4f0a0e9e"  ),
    (true , "568eb40d-cd69-47a5-8932-4f5cc4b2d3fa"  ),
    (true , "29401dcb-6cb6-47a1-8f7d-72dd7f9f4939"  ),
    (true , "e367d539-3163-4530-ab47-3b4cb2df2a40"  ),
    (true , "00000000-0000-4001-8000-000000000000"  ),
    // DCE Version 1 (time based)
    (false, "15edb784-76df-11e6-8b77-86f30ca893d3"  ),
    (false, "232872b8-76df-11e6-8b77-86f30ca893d3"  ),
    (false, "1d5a3bd6-76e0-11e6-b25e-0401beb96201"  ),
    (false, "F0CDE9F0-76DF-11E6-BDF4-0800200C9A66"  ),
    (false, "EA9A1590-76DF-11E6-BDF4-0800200C9A66"  ),
    // DCE Version 3 (name based, md5)
    (false, "1a32cba8-79ba-3f01-bd8a-46c5ae17ccd8"  ),
    (false, "BBCB4DF8-95FB-38E8-A398-187EA35A1655"  ),
    // DCE Version 5 (name based, sha1)
    (false, "74738ff5-5367-5958-9aee-98fffdcd1876"  ),
    // Microsoft GUID
    (false, "00000000-0000-0000-C000-000000000046"  ),
    // NULL UUID
    (false, "00000000-0000-0000-0000-000000000000"  ),
    // Invalid UUIDs
    (false, ""                                      ),    // empty
    (false, "                                    "  ),    // empty
    (false, "\nbdf7bea5-b88e-41b2-be85-c1604e8ddfca"),    // newline
    (false, "bdf7bea5-b88e-41b2-be85-c1604e8ddfca\n"),    // newline
    (false, "74CA6127-E785-4355-8580-1CED4F0A0E9E"  ),    // uppercase
    (false, "568EB40D-CD69-47A5-8932-4F5CC4B2D3FA"  ),    // uppercase
    (false, "29401DCB-6CB6-47A1-8F7D-72DD7F9F4939"  ),    // uppercase
    (false, "E367D539-3163-4530-AB47-3B4CB2DF2A40"  ),    // uppercase
    (false, "C56A4180-65AA-42EC-A945-5FD21DEC"      ),    // too short
    (false, "bdf7bea5-b88e-41b2-be85-c1604e8ddfca " ),    // too long
    (false, " bdf7bea5-b88e-41b2-be85-c1604e8ddfca" ),    // too long
    (false, "bdf7bea5b88e41b2be85c1604e8ddfca"      ),    // missing '-'
    (false, "{bdf7bea5-b88e-41b2-be85-c1604e8ddfca}"),    // '{', '}'
    (false, "bdf7bea5-b88g-41b2-be85-c1604e8ddfca"  ),    // 'g'
    (false, "bdf7bea5_b88e_41b2_be85_c1604e8ddfca"  ),    // '_'
    (false, "bdf7bea5 b88e 41b2 be85 c1604e8ddfca"  ),    // spaces
];

const OTHER: &str = "d2c30518-5cd1-4ce9-a569-44f783a3f66a";

fn valid() -> impl Iterator<Item = (&'static str, Uuid)> {
    DATA.iter()
        .filter(|(valid, _)| *valid)
        .map(|(_, s)| (*s, s.parse().unwrap()))
}

#[test]
fn test_copy_and_to_str() {
    for (s, uuid) in valid() {
        let copy = uuid;
        assert_eq!(copy, uuid);
        assert_eq!(uuid.to_string(), s);
        assert_eq!(uuid.to_string().len(), 36);
    }
}

#[test]
fn test_operator_assign_equals() {
    for (_, uuid1) in valid() {
        let mut uuid2: Uuid = OTHER.parse().unwrap();
        assert_ne!(uuid1.to_string(), uuid2.to_string());
        assert!(uuid1 != uuid2);
        uuid2 = uuid1;
        assert!(uuid1 == uuid2);
        assert_eq!(uuid1.to_string(), uuid2.to_string());
    }
}

#[test]
fn test_operator_comparisons() {
    for (s, uuid1) in valid() {
        let uuid2: Uuid = OTHER.parse().unwrap();
        assert_eq!(OTHER < s, uuid2 < uuid1);
        assert_eq!(s < OTHER, uuid1 < uuid2);
        assert_eq!(OTHER > s, uuid2 > uuid1);
        assert_eq!(s > OTHER, uuid1 > uuid2);
        assert_eq!(OTHER <= s, uuid2 <= uuid1);
        assert_eq!(s <= OTHER, uuid1 <= uuid2);
        assert_eq!(OTHER >= s, uuid2 >= uuid1);
        assert_eq!(s >= OTHER, uuid1 >= uuid2);
    }
}

#[test]
fn test_create_random() {
    for _ in 0..1000 {
        let uuid = Uuid::new_random();
        assert!(Uuid::is_valid(&uuid.to_string()));
    }
}

#[test]
fn test_is_valid_and_from_string() {
    for &(valid, s) in DATA {
        assert_eq!(Uuid::is_valid(s), valid, "{s:?}");
        let result = s.parse::<Uuid>();
        if valid {
            assert_eq!(result.unwrap().to_string(), s);
        } else {
            assert!(result.is_err(), "{s:?}");
        }
    }
}

#[test]
fn test_serialize() {
    for (s, uuid) in valid() {
        assert_eq!(uuid.to_sexpression().value().unwrap(), s);
        assert_eq!(Some(uuid).to_sexpression().value().unwrap(), s);
    }
}

#[test]
fn test_deserialize() {
    for &(valid, s) in DATA {
        let sexpr = SExpression::token(s);
        if valid {
            assert_eq!(Uuid::from_sexpression(&sexpr).unwrap().to_string(), s);
            assert_eq!(
                Option::<Uuid>::from_sexpression(&sexpr)
                    .unwrap()
                    .unwrap()
                    .to_string(),
                s
            );
        } else {
            assert!(Uuid::from_sexpression(&sexpr).is_err());
            assert!(Option::<Uuid>::from_sexpression(&sexpr).is_err());
        }
    }
}

#[test]
fn test_serialize_optional() {
    assert_eq!(None::<Uuid>.to_sexpression().value().unwrap(), "none");
}

#[test]
fn test_deserialize_optional() {
    let sexpr = SExpression::token("none");
    assert_eq!(Option::<Uuid>::from_sexpression(&sexpr), Ok(None));
}
