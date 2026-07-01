cargo build
lldb -o "settings set target.error-path test.log" -- ./target/debug/xob64 tests/binaries/hello_world test
