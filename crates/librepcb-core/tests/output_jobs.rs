//! Port of libs/librepcb/core/job/outputjob.{h,cpp} contracts. The upstream
//! project/organization fixture round trips cover every known job kind.

use librepcb_core::job::{JobKind, ObjectSet, OutputJob, OutputJobList};
use librepcb_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};
use librepcb_core::types::Uuid;

fn node(text: &str) -> SExpression {
    SExpression::parse(text.as_bytes(), None, Mode::LibrePcb).unwrap()
}
fn bytes(job: &OutputJob) -> Vec<u8> {
    let mut n = List::new("job");
    job.serialize(&mut n);
    SExpression::from(n).to_byte_array(Mode::LibrePcb).unwrap()
}

#[test]
fn selections_preserve_none_and_canonical_order() {
    let n = node("(job (board none) (board 11111111-1111-4111-8111-111111111111) (board none))");
    let selection = ObjectSet::<Option<Uuid>>::deserialize(&n, "board").unwrap();
    let ObjectSet::Custom(values) = &selection else {
        panic!("custom selection")
    };
    assert_eq!(values.len(), 2);
    assert!(values.contains(&None));
    let mut r = List::new("job");
    selection.serialize(&mut r, "board");
    assert_eq!(
        ObjectSet::<Option<Uuid>>::deserialize(&r.into(), "board").unwrap(),
        selection
    );
    assert_eq!(
        ObjectSet::<Uuid>::deserialize(&node("(job (board all) (board invalid))"), "board")
            .unwrap(),
        ObjectSet::All
    );
    assert!(ObjectSet::<Uuid>::deserialize(&n, "board").is_err());
    assert_eq!(
        ObjectSet::<Uuid>::deserialize(&node("(job)"), "board").unwrap(),
        ObjectSet::Custom(Default::default())
    );
}
#[test]
fn options_survive_cloning_and_unknown_jobs_are_preserved() {
    let n = node(
        "(job 11111111-1111-4111-8111-111111111111 (name \"JSON\") (type project_json) (output \"test.json\") (option z 1) (option a 2) (option z 3))",
    );
    let job = OutputJob::deserialize(&n).unwrap();
    let mut copy = job.clone();
    copy.set_name("New name".parse().unwrap());
    copy.set_uuid(Uuid::new_random());
    assert_eq!(copy.options, job.options);
    let serialized = String::from_utf8(bytes(&copy)).unwrap();
    assert!(serialized.find("(option a").unwrap() < serialized.find("(option z").unwrap());
    assert_eq!(OutputJob::deserialize(&node(&serialized)).unwrap(), copy);
    let n = node(
        "(job 11111111-1111-4111-8111-111111111111 (name \"Future\") (type future_job) (nested (foo bar)))",
    );
    let mut job = OutputJob::deserialize(&n).unwrap();
    assert!(matches!(job.kind, JobKind::Unknown(_)));
    job.set_name("Ignored".parse().unwrap());
    assert_eq!(bytes(&job), n.to_byte_array(Mode::LibrePcb).unwrap());
}
#[test]
fn output_job_mutation_has_a_serializable_inverse() {
    use librepcb_core::{
        fileio::TransactionalDirectory,
        project::{Change, ChangesSince, Mutation, Project},
    };
    let dir = TransactionalDirectory::new_temporary().unwrap();
    let mut project = Project::create(dir, "test.lpp", Uuid::new_random).unwrap();
    let jobs=OutputJobList::deserialize(&node("(librepcb_jobs (job 11111111-1111-4111-8111-111111111111 (name \"Archive\") (type archive) (input 22222222-2222-4222-8222-222222222222 (destination \"nested\")) (output \"archive.zip\")))")).unwrap();
    assert_eq!(jobs[0].dependencies().len(), 1);
    let encoded = serde_json::to_string(&Mutation::SetOutputJobs(jobs.clone())).unwrap();
    let before = project.revision();
    let inverse = project
        .apply(serde_json::from_str(&encoded).unwrap())
        .unwrap();
    assert_eq!(project.output_jobs(), &jobs);
    assert!(matches!(
        project.changes_since(before),
        ChangesSince::Changes([Change::OutputJobs])
    ));
    project.apply(inverse).unwrap();
    assert!(project.output_jobs().is_empty());
    let mut job = jobs[0].clone();
    job.remove_dependency("22222222-2222-4222-8222-222222222222".parse().unwrap());
    assert!(job.dependencies().is_empty());
}
