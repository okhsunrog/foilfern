//! Serialization, presets and default jobs of `librepcb_core::job`.

use std::collections::BTreeSet;

use librepcb_core::job::{
    ArchiveOutputJob, BomOutputJob, GerberExcellonOutputJob, GraphicsOutputJob, ObjectSet,
    OutputJob, OutputJobKind, OutputJobList, OutputJobType, default_output_jobs,
};
use librepcb_core::project::{AssemblyVariantId, BoardId};
use librepcb_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};
use librepcb_core::types::{ElementName, Uuid};

use crate::helpers::test_data_dir;

fn parse(content: &str) -> SExpression {
    SExpression::parse(content.as_bytes(), None, Mode::LibrePcb).unwrap()
}

/// Serializes `jobs` like `project/jobs.lp`.
fn serialize_jobs(jobs: &OutputJobList) -> String {
    let mut root = List::new("librepcb_jobs");
    jobs.serialize(&mut root);
    SExpression::List(root)
        .to_string_with_mode(Mode::LibrePcb)
        .unwrap()
}

fn serialize_job(job: &OutputJob) -> String {
    let mut root = List::new("job");
    job.serialize(&mut root);
    SExpression::List(root)
        .to_string_with_mode(Mode::LibrePcb)
        .unwrap()
}

fn test_project_jobs() -> String {
    std::fs::read_to_string(
        test_data_dir().join("projects/Project With Two Boards/project/jobs.lp"),
    )
    .unwrap()
}

/// Every job type of the upstream test data is recognized and written back
/// byte by byte.
#[test]
fn test_roundtrip_test_data() {
    let content = test_project_jobs();
    let jobs = OutputJobList::deserialize(&parse(&content)).unwrap();
    let types: BTreeSet<&str> = jobs.iter().map(OutputJob::type_name).collect();
    assert_eq!(
        types,
        BTreeSet::from([
            "3d_model",
            "archive",
            "bom",
            "copy",
            "gerber_excellon",
            "gerber_x3",
            "graphics",
            "interactive_bom",
            "lppz",
            "netlist",
            "pnp",
            "project_json",
        ])
    );
    assert!(
        jobs.iter()
            .all(|job| !matches!(job.kind(), OutputJobKind::Unknown(_)))
    );
    assert_eq!(serialize_jobs(&jobs), content);

    // Serde round trip (MCP wire format).
    let json = serde_json::to_string(&jobs).unwrap();
    let jobs2: OutputJobList = serde_json::from_str(&json).unwrap();
    assert_eq!(jobs2, jobs);
}

#[test]
fn test_unknown_job_is_kept_verbatim() {
    let content = "(librepcb_jobs\n (job 2f4ad4c6-d1e5-4d21-a1b3-38e5ae0e0e8f (name \"Future\")\n  \
                   (type future_job) (foo \"bar\") (baz 1)\n )\n)\n";
    let jobs = OutputJobList::deserialize(&parse(content)).unwrap();
    let job = jobs.first().unwrap();
    assert_eq!(job.type_name(), "future_job");
    assert_eq!(job.type_tr(), "Unknown (future_job)");
    assert_eq!(job.name().as_str(), "Future");
    assert!(matches!(job.kind(), OutputJobKind::Unknown(_)));
    assert_eq!(serialize_jobs(&jobs), content);
}

#[test]
fn test_options_are_kept() {
    let content = "(job 2f4ad4c6-d1e5-4d21-a1b3-38e5ae0e0e8f (name \"Data\")\n \
                   (type project_json)\n (output \"x.json\")\n (option b 1)\n (option a 2)\n)\n";
    let job = OutputJob::deserialize(&parse(content)).unwrap();
    assert_eq!(job.options().len(), 2);
    // Written in key order, like upstream's `QMap`.
    assert_eq!(
        serialize_job(&job),
        "(job 2f4ad4c6-d1e5-4d21-a1b3-38e5ae0e0e8f (name \"Data\")\n \
         (type project_json)\n (output \"x.json\")\n (option a 2)\n (option b 1)\n)\n"
    );
}

#[test]
fn test_invalid_job() {
    let invalid = [
        // Missing type.
        "(job 2f4ad4c6-d1e5-4d21-a1b3-38e5ae0e0e8f (name \"X\"))",
        // Invalid UUID.
        "(job foo (name \"X\") (type lppz) (output \"x\"))",
        // Missing output.
        "(job 2f4ad4c6-d1e5-4d21-a1b3-38e5ae0e0e8f (name \"X\") (type lppz))",
        // Invalid boolean.
        "(job 2f4ad4c6-d1e5-4d21-a1b3-38e5ae0e0e8f (name \"X\") (type copy) \
         (substitute_variables yes) (board none) (variant none) (input \"a\") (output \"b\"))",
    ];
    for content in invalid {
        assert!(
            OutputJob::deserialize(&parse(content)).is_err(),
            "{content}"
        );
    }
}

#[test]
fn test_object_set() {
    let node = parse(
        "(x (board 7ae7d236-08b5-4cae-9c5b-2d9cfd3bca10) (board none) \
         (board 03c3d9da-e2e0-4ce5-8e5c-a2f9a5dcdbf3))",
    );
    let set = ObjectSet::<Option<BoardId>>::deserialize(&node, "board").unwrap();
    assert!(set.is_custom());
    let mut root = List::new("x");
    set.serialize(&mut root, "board");
    // Sorted, `none` first.
    assert_eq!(
        SExpression::List(root)
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        "(x\n (board none)\n (board 03c3d9da-e2e0-4ce5-8e5c-a2f9a5dcdbf3)\n \
         (board 7ae7d236-08b5-4cae-9c5b-2d9cfd3bca10)\n)\n"
    );

    for (content, expected) in [
        ("(x (variant all))", ObjectSet::All),
        ("(x (variant default))", ObjectSet::Default),
        ("(x)", ObjectSet::custom([])),
    ] {
        assert_eq!(
            ObjectSet::<AssemblyVariantId>::deserialize(&parse(content), "variant").unwrap(),
            expected
        );
    }
    // `none` is not a valid UUID.
    assert!(ObjectSet::<BoardId>::deserialize(&parse("(x (board none))"), "board").is_err());
}

/// Replaces the UUID of a serialized job by `uuid`.
fn with_uuid(job: &OutputJob, uuid: &str) -> OutputJob {
    job.with_uuid(uuid.parse().unwrap())
}

/// Returns the job with UUID `uuid` from the test project, serialized.
fn test_project_job(uuid: &str) -> String {
    let jobs = OutputJobList::deserialize(&parse(&test_project_jobs())).unwrap();
    serialize_job(jobs.by_uuid(&uuid.parse().unwrap()).unwrap())
}

/// The presets produce the jobs upstream created in the test project,
/// except that the board documentation colors changed since then in the
/// upstream color scheme (see `tests/data/projects/v1` for the current
/// ones).
#[test]
fn test_graphics_presets() {
    let schematic = "416ac5d4-4df9-4705-8d45-8cfebc2e912a";
    assert_eq!(
        serialize_job(&with_uuid(&GraphicsOutputJob::schematic_pdf(), schematic)),
        test_project_job(schematic)
    );
    let assembly = "ec258064-dc13-4814-ba3e-98aa93c263b6";
    let expected = test_project_job(assembly);
    let expected = ["", "_top", "_bottom"].iter().fold(expected, |s, suffix| {
        s.replace(
            &format!("(layer board_documentation{suffix} (color \"#ca707070\"))"),
            &format!("(layer board_documentation{suffix} (color \"#ba7d624b\"))"),
        )
    });
    assert_eq!(
        serialize_job(&with_uuid(
            &GraphicsOutputJob::board_assembly_pdf(),
            assembly
        )),
        expected
    );
    let rendering = "c02b67e3-0c57-4cd3-b5fa-d6dbb3143f8f";
    assert_eq!(
        serialize_job(&with_uuid(
            &GraphicsOutputJob::board_rendering_pdf(),
            rendering
        )),
        test_project_job(rendering)
    );
}

#[test]
fn test_gerber_styles() {
    let default = GerberExcellonOutputJob::default_style();
    assert_eq!(default.name().as_str(), "Gerber/Excellon");
    assert_eq!(
        serialize_job(&with_uuid(&default, "ee12ed43-dad6-409b-b57a-92ce95e03ca0")),
        test_project_job("ee12ed43-dad6-409b-b57a-92ce95e03ca0")
            .replace("gbr/{{PROJECT}}", "gerber/{{PROJECT}}")
    );
    let protel = GerberExcellonOutputJob::protel_style();
    let OutputJobKind::GerberExcellon(settings) = protel.kind() else {
        panic!("not a Gerber job");
    };
    assert_eq!(settings.suffix_copper_top, ".gtl");
    assert!(settings.merge_drill_files);
}

#[test]
fn test_default_output_jobs() {
    let jobs = default_output_jobs(GerberExcellonOutputJob::default(), vec!["MPN".into()]);
    let names: Vec<&str> = jobs.iter().map(|j| j.name().as_str()).collect();
    assert_eq!(
        names,
        [
            "Schematic PDF",
            "Board Assembly PDF",
            "Gerber/Excellon",
            "Pick&Place CSV",
            "Bill of Materials",
            "Output Archive",
            "Project Archive",
        ]
    );
    let gerber = jobs[2].uuid();
    assert_eq!(jobs[5].dependencies(), BTreeSet::from([gerber]));
    let OutputJobKind::Bom(bom) = jobs[4].kind() else {
        panic!("not a BOM job");
    };
    assert_eq!(bom.custom_attributes, ["MPN"]);

    // Serialization round trip.
    let content = serialize_jobs(&jobs);
    let jobs2 = OutputJobList::deserialize(&parse(&content)).unwrap();
    assert_eq!(jobs2, jobs);
    assert!(content.contains(&format!("  (input {gerber} (destination \"\"))\n")));
}

#[test]
fn test_remove_dependency() {
    let input = Uuid::new_random();
    let mut job = OutputJob::new(
        Uuid::new_random(),
        ElementName::new("Zip").unwrap(),
        ArchiveOutputJob {
            input_jobs: [(input, "gerber".to_owned())].into(),
            ..ArchiveOutputJob::default()
        },
    );
    assert!(!job.remove_dependency(&Uuid::new_random()));
    assert!(job.remove_dependency(&input));
    assert!(job.dependencies().is_empty());
    let mut bom = OutputJob::new_default::<BomOutputJob>();
    assert!(!bom.remove_dependency(&input));
    assert_eq!(bom.name().as_str(), "Bill of Materials");
    assert_eq!(bom.type_tr(), "Bill Of Materials (*.csv)");
    assert_eq!(bom.type_name(), BomOutputJob::TYPE_NAME);
}
