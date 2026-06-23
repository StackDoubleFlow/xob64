use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter};

use crate::runner::{CompiledChunk, compiler::get_arm_chunk};

#[derive(Default)]
struct X86Formatter {
    inner: IntelFormatter,
    str: String,
}

impl X86Formatter {
    fn format(&mut self, instr: &Instruction) -> &str {
        self.str.clear();
        self.inner.format(instr, &mut self.str);
        &self.str
    }
}

fn dump_instr_pair(
    arm_instr: Option<(usize, &Result<bad64::Instruction, bad64::DecodeError>)>,
    x86_instr: Option<&Instruction>,
    formatter: &mut X86Formatter,
) {
    if let Some((arm_instr_addr, arm_instr)) = arm_instr {
        let arm_asm = match arm_instr {
            Ok(arm_instr) => format!("{}", arm_instr),
            Err(_) => "invalid instruction".to_string(),
        };
        print!("{:?}: {:<70}", arm_instr_addr as *const u8, arm_asm);
    } else {
        print!("{:<86}", "");
    }

    if let Some(x86_instr) = x86_instr {
        print!(
            "{:?}: {}",
            x86_instr.ip() as *const u8,
            formatter.format(x86_instr)
        );
    }
    println!();
}

pub fn dump_translation(chunk_addr: usize, compiled_chunk: &CompiledChunk) {
    let mut arm_instrs = get_arm_chunk(chunk_addr);

    let x86_code = unsafe { std::slice::from_raw_parts(compiled_chunk.addr, compiled_chunk.len) };
    let mut x86_decoder = Decoder::with_ip(
        64,
        x86_code,
        compiled_chunk.addr as u64,
        DecoderOptions::NONE,
    );
    let mut formatter = X86Formatter::default();

    let mut decoded_x86_instr = Instruction::new();
    let x86_offset = |instr: &Instruction| instr.ip() as usize - compiled_chunk.addr as usize;

    x86_decoder.decode_out(&mut decoded_x86_instr);
    dump_instr_pair(
        Some((chunk_addr, &arm_instrs.next().unwrap())),
        Some(&decoded_x86_instr),
        &mut formatter,
    );

    for (arm_instr_idx, arm_instr) in arm_instrs.enumerate() {
        let arm_instr_idx = arm_instr_idx + 1;
        let arm_addr = chunk_addr + arm_instr_idx * 4;

        let cur_arm_x86_offset = compiled_chunk.instr_map[arm_instr_idx] as usize;
        if x86_offset(&decoded_x86_instr) == cur_arm_x86_offset {
            // We've already printed the corresponding x86 instruction, so we only print the ARM instruction;
            dump_instr_pair(Some((arm_addr, &arm_instr)), None, &mut formatter);
            continue;
        }

        x86_decoder.decode_out(&mut decoded_x86_instr);
        while x86_offset(&decoded_x86_instr) < cur_arm_x86_offset {
            // We haven't reached the ARM instruction yet, so we print the x86 instruction and decode the next one.
            dump_instr_pair(None, Some(&decoded_x86_instr), &mut formatter);
            x86_decoder.decode_out(&mut decoded_x86_instr);
        }

        dump_instr_pair(
            Some((arm_addr, &arm_instr)),
            Some(&decoded_x86_instr),
            &mut formatter,
        );
    }

    loop {
        x86_decoder.decode_out(&mut decoded_x86_instr);
        if decoded_x86_instr.is_invalid() {
            break;
        } else {
            dump_instr_pair(None, Some(&decoded_x86_instr), &mut formatter);
        }
    }
}
