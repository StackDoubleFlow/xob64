pub mod callbacks;
mod compiler;

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use nix::libc;

use crate::loader::PAGE_SIZE;

const CHUNK_SIZE: usize = 512;
const EXECUTABLE_ALLOC_SIZE: usize = 1024 * 16;
const SHADOW_STACK_SIZE: usize = 4096;

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
    eprintln!(
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
#[derive(Debug)]
pub struct ExecCtx {
    indirect_regs: [u64; 18],
    indirect_fp_regs: [u128; 17],
    // Used to pass info to a callback function
    param: u64,
    shadow_sp: *mut u64,
    shadow_stack_alloc: *mut libc::c_void,
}

impl ExecCtx {
    pub const PARAM_OFFSET: usize = std::mem::offset_of!(Self, param);
    pub const SHADOW_SP_OFFSET: usize = std::mem::offset_of!(Self, shadow_sp);

    pub fn new() -> Self {
        let shadow_stack_alloc = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SHADOW_STACK_SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_GROWSDOWN,
                -1,
                0,
            )
        };
        Self {
            indirect_regs: Default::default(),
            indirect_fp_regs: Default::default(),
            param: 0,
            shadow_sp: shadow_stack_alloc.wrapping_byte_add(SHADOW_STACK_SIZE) as _,
            shadow_stack_alloc,
        }
    }

    pub fn push_shadow_stack(&mut self, arm_ptr: *const u8, native_ptr: *const u8) {
        self.shadow_sp = self.shadow_sp.wrapping_byte_sub(16);
        unsafe {
            self.shadow_sp.wrapping_byte_add(8).write(arm_ptr as u64);
            self.shadow_sp.write(native_ptr as u64);
        }
    }
}

impl Drop for ExecCtx {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.shadow_stack_alloc, SHADOW_STACK_SIZE);
        }
    }
}

pub fn call(ptr: *const u8, args: &[*const u8]) {
    let exec_ptr = get_exec(ptr);
    eprintln!("calling {:?} -> {:?}", ptr, exec_ptr);
    let mut ctx = ExecCtx::new();
    // The pointers will be filled in by the inline asm
    ctx.push_shadow_stack(std::ptr::null(), std::ptr::null());
    let ctx_ptr = &mut ctx as *mut ExecCtx;
    eprintln!("ctx_ptr: {:?}", ctx_ptr);

    let argc = args.len();
    let argv = args.as_ptr();

    // envp, argc and argv take up:
    // - 8 bytes for add_align
    // - 8 bytes for envp terminator
    // - 8 bytes for argc terminator
    // - argc*8 bytes for argv
    // - 8 bytes for argc
    // This totals to 32 + argc*8 bytes
    // To maintain 16 byte rsp alignment, if argc is odd, we need to add an extra 8 bytes
    let add_align = (argc % 2 == 1) as usize;

    unsafe {
        std::arch::asm!(
            // Load emulation context register.
            "mov r15, {ctx_ptr}",

            // Stack alignment correction
            "push {add_align}",
            "test {add_align}, {add_align}",
            "jz 5f",
            "sub rsp, 8",
            "5:",

            // Zero terminators for envp and argv
            "push 0",
            "push 0",
            // argc
            "mov {add_align}, {argc}", // Re-use add_align to save a copy of argc
            "shl {add_align}, 3",
            "sub rsp, {add_align}", // Reserve space for argv
            "push {argc}",
            // argv
            "4: test {argc}, {argc}",
            "jz 3f",
            "sub {argc}, 1",
            "mov {temp}, [{argv} + {argc} * 8]",
            "mov [rsp + {argc} * 8 + 8], {temp}",
            "jmp 4b",
            "3:",
            // Make room for argv using argc copy

            // Load link register
            "lea r11, [rip + 2f]",
            // Store return address on shadow stack
            "mov {temp}, [r15 + {shadow_sp}]",
            "mov [{temp} + 9], r11",
            "mov [{temp}], r11",
            // Jump to emulation
            "jmp {target}",
            "2:",

            // Pop argc
            "pop rax",
            // Pop argv
            "shl rax, 3",
            "add rsp, rax",
            // Pop argv and envp terminators
            "add rsp, 16",

            // Reverse rsp alignment correction
            "pop rax",
            "test rax, rax",
            "jz 6f",
            "pop rax",
            "6:",

            target = in(reg) exec_ptr,
            ctx_ptr = in(reg) ctx_ptr,
            argc = in(reg) argc,
            argv = in(reg) argv,
            add_align = in(reg) add_align,
            temp = in(reg) 0u64,
            shadow_sp = const ExecCtx::SHADOW_SP_OFFSET,
            // Emulated code mostly follows the C calling convention except for r15 which additionally
            clobber_abi("C"),
            out("r15") _
        )
    }
    eprintln!("returned from initializer call");
}
