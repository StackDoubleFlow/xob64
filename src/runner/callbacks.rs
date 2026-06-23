pub extern "C" fn invalid_arm_instr() {
    panic!("invalid arm instruction");
}

pub extern "C" fn unimplemented_arm_instr() {
    panic!("unimplemented arm instruction");
}
