mod callbacks;
mod compiler;

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

const CHUNK_SIZE: usize = 512;
const EXECUTABLE_ALLOC_SIZE: usize = 1024 * 16;

struct ExecutableRange {
    start: *const u8,
    end: *const u8,
}

struct CompiledChunk {
    /// This maps ARM64 instruction indices from the original chunk to byte indices in the x86_64 executable data.
    instr_map: Vec<u16>,
    addr: *const u8,
    len: usize,
}

#[derive(Default)]
struct ExecPool {
    // We don't actually mark any ARM64 memory as executable, but we instead keep track of their ranges here.
    // When they are attempted to be executed, we dynamically compile them to x86_64.
    exec_ranges: Vec<ExecutableRange>,
    executable_map: HashMap<usize, CompiledChunk>,
    fully_used_allocs: Vec<*const u8>,
    current_alloc: *const u8,
    current_alloc_utilization: usize,
}

impl ExecPool {
    pub fn is_executable(&self, ptr: *const u8) -> bool {
        let addr = ptr as usize;
        self.exec_ranges
            .iter()
            .any(|r| r.start as usize <= addr && addr < r.end as usize)
    }
}

unsafe impl Send for ExecPool {}

static EXEC_POOL: LazyLock<Arc<Mutex<ExecPool>>> =
    LazyLock::new(|| Arc::new(Mutex::new(ExecPool::default())));

pub fn define_exec_range(start: *const u8, end: *const u8) {
    let mut exec_pool = EXEC_POOL.lock().unwrap();
    let range = ExecutableRange { start, end };
    println!(
        "marking executable range: {:?}-{:?}",
        range.start, range.end
    );
    exec_pool.exec_ranges.push(range);
}

/// Translates the ARM64 code address to the translated x86_64 executable address
pub fn get_exec(ptr: *const u8) -> *const u8 {
    let mut exec_pool = EXEC_POOL.lock().unwrap();

    // First, check to see if it's an emulated ARM64 executable address. If not, return the original pointer.
    if !exec_pool.is_executable(ptr) {
        return ptr;
    }

    let addr = ptr as usize;
    let chunk_offset = addr % CHUNK_SIZE;
    let chunk_addr = addr - chunk_offset;

    let chunk = if let Some(chunk) = exec_pool.executable_map.get(&chunk_addr) {
        chunk
    } else {
        let chunk = compiler::compile_chunk(&mut exec_pool, chunk_addr);
        exec_pool.executable_map.insert(chunk_addr, chunk);
        exec_pool.executable_map.get(&chunk_addr).unwrap()
    };

    let instr_idx = chunk_offset / 4;
    let byte_idx = chunk.instr_map[instr_idx] as usize;
    unsafe { chunk.addr.add(byte_idx) }
}

// Translates the x86_64 executable address to the ARM64 code address
pub fn from_exec(ptr: *const u8) -> *const u8 {
    let addr = ptr as usize;

    let exec_pool = EXEC_POOL.lock().unwrap();

    // TODO: maybe write a faster lookup
    for (&chunk_arm_addr, chunk) in &exec_pool.executable_map {
        let chunk_addr = chunk.addr as usize;

        if addr >= chunk_addr && addr < chunk_addr + chunk.len {
            let chunk_offset = addr - chunk_addr;
            // Start from the end of the chunk and work backwords until we find a matching offset or an offset that is
            // less than what we're looking for, which indicates that the arm instruction maps to multiple x86_64
            // instructions.
            for (arm_offset, &offset) in chunk.instr_map.iter().enumerate().rev() {
                if chunk_offset >= offset as usize {
                    return (chunk_arm_addr + arm_offset as usize * 4) as *const u8;
                }
            }

            panic!(
                "from_exec found matching chunk, but no matching offset: ptr={:?}",
                ptr
            )
        }
    }

    panic!("from_exec failed: ptr={:?}", ptr);
}

#[repr(C)]
#[derive(Default, Debug)]
pub struct ExecCtx {
    indirect_regs: [u64; 18],
    indirect_fp_regs: [u128; 17],
    // Used to pass info to a callback function
    param: u64,
}

impl ExecCtx {
    pub const PARAM_OFFSET: usize = std::mem::offset_of!(Self, param);
}

pub fn call(ptr: *const u8) {
    let exec_ptr = get_exec(ptr);
    println!("calling {:?} -> {:?}", ptr, exec_ptr);
    let mut ctx = ExecCtx::default();
    let ctx_ptr = &mut ctx as *mut ExecCtx;
    println!("ctx_ptr: {:?}", ctx_ptr);
    unsafe {
        std::arch::asm!(
            // Load emulation context register.
            // r15 is callee-saved so we need to preserve it.
            "sub rsp, 16",
            "mov [rsp], r15",
            "mov r15, r11",
            // Load link register
            "lea r11, [rip + 2f]",
            "jmp {}",
            // Restore r15 from the stack.
            "2: mov r15, [rsp]",
            "add rsp, 16",
            in(reg) exec_ptr,
            // r11 is an arbitrary caller-saved register (not related to the emulation context)
            in("r11") ctx_ptr,
            clobber_abi("C")
        )
    }
    println!("returned from call");
}
