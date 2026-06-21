use assert_cmd::Command;

#[test]
fn hello_world() {
    Command::cargo_bin("xob64")
        .unwrap()
        .assert()
        .stdout("Hello world!\n")
        .success();
}
