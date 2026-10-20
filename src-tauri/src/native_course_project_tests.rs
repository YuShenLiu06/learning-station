use crate::{
    app::Engine,
    generation::{self, GenerationRequest},
    library_tree, native_session,
};
use std::sync::Arc;

#[tokio::test]
async fn production_course_binding_initializes_its_git_root_before_the_model_gate() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
    let (one, two, other, legacy, root) = {
        let mut core = engine.lock().unwrap();
        let a = library_tree::create_project(&mut core.content, "A", "", "binding-course-a").unwrap();
        let b = library_tree::create_project(&mut core.content, "B", "", "binding-course-b").unwrap();
        let old = library_tree::create(&mut core.content, "course", "Legacy", None, "", "binding-legacy-course").unwrap();
        let one = crate::course_sessions::create(&mut core, &a.id, "binding-session-one").unwrap();
        let two = crate::course_sessions::create(&mut core, &a.id, "binding-session-two").unwrap();
        let other = crate::course_sessions::create(&mut core, &b.id, "binding-session-other").unwrap();
        let legacy = crate::course_sessions::create(&mut core, &old.id, "binding-session-legacy").unwrap();
        let root = crate::course_project::course_project_root(core.content.root(), &a.id).unwrap();
        assert!(!root.join(".git").exists());
        (one, two, other, legacy, root)
    };
    let request = GenerationRequest {
        session_id: one.id.clone(), document_id: None, mode: "explain".into(),
        selection: String::new(), prompt: "Synthetic unconfigured course chat".into(),
        request_id: "binding-production-request".into(), user_entry_id: None,
        injections: vec![], assistant_references: vec![], exercise_references: vec![],
    };
    // Production source/binding preparation precedes credentials, and the default
    // unconfigured gate cannot open Keychain or contact a real model.
    assert_eq!(generation::start(engine.clone(), request, Arc::new(|_| {})).await.unwrap_err().code, "NOT_CONFIGURED");
    assert!(root.join(".git/learning-materials-v2").is_file());
    let core = engine.lock().unwrap();
    assert_eq!(native_session::project_root(&core.content, &one).unwrap(), Some(root.clone()));
    assert_eq!(native_session::project_root(&core.content, &two).unwrap(), Some(root.clone()));
    assert_ne!(native_session::project_root(&core.content, &other).unwrap(), Some(root));
    assert!(native_session::project_root(&core.content, &legacy).unwrap().is_none());
    let a = native_session::prepare(&core.state_dir, &one, "run_binding_a", "user_binding_a", vec![], false, vec![], "").unwrap();
    let b = native_session::prepare(&core.state_dir, &two, "run_binding_b", "user_binding_b", vec![], false, vec![], "").unwrap();
    assert_ne!(a.directory, b.directory);
    assert_eq!(one.course_id, two.course_id);
    assert_ne!(one.course_id, other.course_id);
}
