cargo build
lldb -o "settings set target.error-path test.log" -- ./target/debug/xob64 test-emu/usr/bin/echo one two
