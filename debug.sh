cargo build
lldb -o "settings set target.output-path test.log" -- ./target/debug/xob64 tests/binaries/hello_world
