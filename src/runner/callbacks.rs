pub extern "C" fn invalid_arm_instr() {
    eprintln!("todo: invalid arm instruction");
    std::process::abort();
}

pub extern "C" fn unimplemented_arm_instr() {
    eprintln!("todo: unimplemented arm instruction");
    std::process::abort();
}

pub extern "C" fn end_of_chunk() {
    eprintln!("todo: end of chunk");
    std::process::abort();
}
