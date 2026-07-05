mod elf_lookup;
mod overrides;
mod proxy;

use std::{
    collections::HashMap,
    ffi::{CStr, CString},
};

use object::elf;

use crate::wrapped::{elf_lookup::SymbolFinder, proxy::create_lib_proxy};

pub struct WrappedLib {
    handle: *mut nix::libc::c_void,
    base_addr: u64,
    overrides: HashMap<&'static CStr, *const u8>,
    symbol_finder: SymbolFinder,
    pub path: CString,
}

impl WrappedLib {
    pub fn try_load(name: &CStr) -> Option<Self> {
        let handle = dlopen(name);
        if handle.is_null() {
            return None;
        }

        let overrides = overrides::get_overrides(name);

        let obj_info = elf_lookup::loaded_object_info(name).unwrap();
        eprintln!("Loaded wrapped library: {:?}", &obj_info.path);
        let symbol_finder = SymbolFinder::new(&obj_info);

        Some(Self {
            base_addr: obj_info.base_addr,
            handle,
            overrides,
            symbol_finder,
            path: obj_info.path,
        })
    }

    pub fn get_symbol(&self, name: &CStr) -> Option<*const u8> {
        if let Some(&ov) = self.overrides.get(name) {
            return Some(ov);
        }

        let addr = unsafe { nix::libc::dlsym(self.handle, name.as_ptr()) };
        if addr.is_null() {
            return None;
        }

        if let Some(sym) = self.symbol_finder.lookup(name) {
            let sym = unsafe { &*sym };
            let bind = sym.st_info >> 4;
            let ty = sym.st_info & 0xf;
            if (bind == elf::STB_GLOBAL || bind == elf::STB_WEAK) && sym.st_shndx != elf::SHN_UNDEF
            {
                let addr = sym.st_value + self.base_addr;
                let addr = if ty == elf::STT_FUNC {
                    create_lib_proxy(addr).unwrap()
                } else {
                    addr as _
                };
                return Some(addr);
            }
        }

        None
    }
}

fn dlopen(name: &CStr) -> *mut nix::libc::c_void {
    unsafe { nix::libc::dlopen(name.as_ptr(), nix::libc::RTLD_LAZY) }
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

pub(self) use wrapped_landing_pad;
