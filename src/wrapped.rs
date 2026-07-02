pub mod libc;
pub mod libcap;

// Returns true if succeeded
pub fn try_load_wrapped(name: &str, symbol_table: &mut SymbolTable) -> bool {
    if name.starts_with("libc.so") {
        libc::register_symbols(symbol_table);
    } else if name.starts_with("libcap.so") {
        libcap::register_symbols(symbol_table);
    } else if name.starts_with("ld-linux-aarch64.so") {
        // TODO
    } else {
        return false;
    }
    true
}

macro_rules! wrapped_landing_pad {
    ($name:ident, $func:ident) => {
        #[unsafe(naked)]
        extern "C" fn $name() {
            std::arch::naked_asm!(
                "sub rsp, 16",
                "mov [rsp], r10", // x23 is callee-saved but r10 is temporary
                "mov [rsp + 8], r11", // link register
                "call {target}",
                "mov r10, [rsp]",
                "mov r11, [rsp + 8]",
                "add rsp, 16",
                "mov rdi, rax", // Return value
                // Shadow stack return sequence
                "mov rdx, [r15 + {shadow_sp}]",
                "mov rax, [rdx + 8]",
                "cmp r11, rax",
                "mov rax, [rdx]",
                "lea rdx, [rdx + 16]",
                "mov [r15 + {shadow_sp}], rdx",
                "jne 2f",
                "jmp rax",
                "2: mov [r15 + {param_offset}], r11",
                "call {indirect_jump}",
                target = sym $func,
                param_offset = const $crate::runner::ExecCtx::PARAM_OFFSET,
                shadow_sp = const $crate::runner::ExecCtx::SHADOW_SP_OFFSET,
                indirect_jump = sym $crate::runner::callbacks::indirect_jump_landing_pad
            )
        }
    };
}
use std::ffi::CStr;

pub(self) use wrapped_landing_pad;

struct LibProxyInfo {
    name: &'static CStr,
    target: *mut u64,
    proxy_fn: *const (),
}
unsafe impl Sync for LibProxyInfo {}

fn load_proxy(symbol_table: &mut SymbolTable, handle: *mut nix::libc::c_void, info: &LibProxyInfo) {
    let addr = unsafe { nix::libc::dlsym(handle, info.name.as_ptr()) as u64 };
    unsafe { *info.target = addr };
    symbol_table.insert_global(info.name, info.proxy_fn);
}

macro_rules! wrapped_lib_proxy {
    ($name:ident, $sym_name:expr) => {
        mod $name {
            static mut TARGET: u64 = 0;
            pub static INFO: $crate::wrapped::LibProxyInfo = $crate::wrapped::LibProxyInfo {
                name: $sym_name,
                target: &raw mut TARGET,
                proxy_fn: proxy as *const ()
            };

            #[unsafe(naked)]
            extern "C" fn proxy() {
                std::arch::naked_asm!(
                    "sub rsp, 16",
                    "mov [rsp], r10", // x23 is callee-saved but r10 is temporary
                    "mov [rsp + 8], r11", // link register
                    "lea rax, [rip + {target}]",
                    "call [rax]",
                    "mov r10, [rsp]",
                    "mov r11, [rsp + 8]",
                    "add rsp, 16",
                    "mov rdi, rax", // Return value
                    // Shadow stack return sequence
                    "mov rdx, [r15 + {shadow_sp}]",
                    "mov rax, [rdx + 8]",
                    "cmp r11, rax",
                    "mov rax, [rdx]",
                    "lea rdx, [rdx + 16]",
                    "mov [r15 + {shadow_sp}], rdx",
                    "jne 2f",
                    "jmp rax",
                    "2: mov [r15 + {param_offset}], r11",
                    "call {indirect_jump}",
                    target = sym TARGET,
                    param_offset = const $crate::runner::ExecCtx::PARAM_OFFSET,
                    shadow_sp = const $crate::runner::ExecCtx::SHADOW_SP_OFFSET,
                    indirect_jump = sym $crate::runner::callbacks::indirect_jump_landing_pad,
                )
            }
        }
    };
}
pub(self) use wrapped_lib_proxy;

use crate::loader::SymbolTable;
