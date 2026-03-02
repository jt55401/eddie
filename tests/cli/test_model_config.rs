use std::process::Command;

#[test]
fn index_help_shows_default_embedding_model() {
    let exe = env!("CARGO_BIN_EXE_eddie");
    let output = Command::new(exe)
        .args(["index", "--help"])
        .output()
        .expect("run eddie index --help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf8");
    assert!(
        stdout.contains("sentence-transformers/all-MiniLM-L6-v2"),
        "expected default model in help output, got:\n{stdout}"
    );
}
