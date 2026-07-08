use assert_cmd::Command;

fn run(expected: &'static str, args: &'static [&'static str]) {
    Command::cargo_bin("xob64")
        .unwrap()
        .arg("./tests/binaries/print_args")
        .args(args)
        .assert()
        .stdout(expected)
        .success();
}

#[test]
fn no_args() {
    let expected = "Arg 0: ./tests/binaries/print_args\n";
    run(expected, &[]);
}

#[test]
fn one_arg() {
    let expected = "Arg 0: ./tests/binaries/print_args\n\
                    Arg 1: first arg\n";
    run(expected, &["first arg"]);
}
