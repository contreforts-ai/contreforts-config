//! Pins `ConfigStore::open`'s own-store behaviour (contreforts/contreforts-workspace#58
//! D2): a named error on an unusable path -- never a panic, never a silent
//! fallback to a temp or current directory -- and that two stores opened at
//! different datadirs never observe each other's data.

use contreforts_config::ConfigStore;
use oxigraph::model::{GraphName, NamedNode, Quad, Term};

#[test]
fn open_at_unusable_path_returns_a_named_error_not_a_panic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // A regular file sits where a directory component is required, so no
    // directory can ever be created under it -- this path can never become a
    // valid store, on any platform.
    let blocker = tmp.path().join("not-a-directory");
    std::fs::write(&blocker, b"not a directory").expect("write blocker file");
    let unusable = blocker.join("config_store");

    let result = ConfigStore::open(&unusable);

    let err = result.err().unwrap_or_else(|| {
        panic!(
            "opening a config store at {unusable:?}, blocked by a file rather than a \
             directory, must return an error -- it must not silently succeed by falling \
             back to a temp or current directory"
        )
    });

    let message = err.to_string();
    assert!(
        message.contains(&unusable.to_string_lossy().into_owned()),
        "the error {message:?} must name the unusable path {unusable:?}, so an operator \
         can tell which datadir failed"
    );
}

#[test]
fn two_datadirs_do_not_observe_each_others_data() {
    let tmp_a = tempfile::tempdir().expect("tempdir a");
    let tmp_b = tempfile::tempdir().expect("tempdir b");
    let path_a = tmp_a.path().join("config_store");
    let path_b = tmp_b.path().join("config_store");

    let store_a = ConfigStore::open(&path_a)
        .unwrap_or_else(|e| panic!("store at {path_a:?} must open cleanly: {e}"));
    let store_b = ConfigStore::open(&path_b)
        .unwrap_or_else(|e| panic!("store at {path_b:?} must open cleanly: {e}"));

    let quad = Quad::new(
        NamedNode::new("https://contreforts.test/subject").unwrap(),
        NamedNode::new("https://contreforts.test/predicate").unwrap(),
        Term::NamedNode(NamedNode::new("https://contreforts.test/object").unwrap()),
        GraphName::DefaultGraph,
    );

    store_a
        .inner()
        .insert(&quad)
        .expect("insert into the store at path_a");

    assert!(
        store_a.inner().contains(&quad).unwrap(),
        "the store opened at {path_a:?} must contain what was written to it"
    );
    assert!(
        !store_b.inner().contains(&quad).unwrap(),
        "the store opened at {path_b:?} must not observe data written to the store at \
         {path_a:?} -- distinct datadirs must not share data"
    );
}
