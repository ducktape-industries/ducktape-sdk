//! The refusal frame names no refuser, so a caller attributes a module-state
//! class to the module it addressed (see `refusal_class`'s "Who may mint
//! what"). This host therefore never mints one.

#[test]
fn the_host_never_mints_a_class_about_module_state() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let module_only = [
        "NOT_FOUND",
        "ALREADY_EXISTS",
        "STALE",
        "WRONG_STATE",
        "NOT_YET",
        "UNAUTHORIZED",
    ];
    let mut read = 0;
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            read += 1;
            for class in module_only {
                assert!(
                    !text.contains(&format!("refusal::{class}")),
                    "{} mints refusal::{class}, a class only the addressed module may mint",
                    path.display()
                );
            }
        }
    }
    assert!(read > 0, "no host source was read");
}
