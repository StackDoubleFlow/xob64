use std::{
    collections::HashMap,
    ffi::CString,
    fs::{self, File},
    os::fd::AsRawFd,
    sync::{Arc, LazyLock, Mutex},
};

use object::{
    Object, ObjectSegment, ObjectSymbol, ObjectSymbolTable, RelocationFlags, RelocationTarget, elf,
    read::elf::{ElfFile64, ElfSegment64},
};

use crate::runner;

const PAGE_SIZE: LazyLock<usize> =
    LazyLock::new(|| unsafe { nix::libc::sysconf(nix::libc::_SC_PAGE_SIZE) as usize });

#[derive(Default)]
struct ObjectPool {
    objects: Vec<EmuObject>,
    symbol_table: SymbolTable,
}

// *const u8 is not normally Send
unsafe impl Send for ObjectPool {}

struct EmuObject {
    base_ptr: *const u8,
    init_array: Vec<*const u8>,
    fini_array: Vec<*const u8>,
}

#[derive(Default)]
struct SymbolTable {
    global_symbols: HashMap<String, *const u8>,
}

impl SymbolTable {}

static OBJECT_POOL: LazyLock<Arc<Mutex<ObjectPool>>> =
    LazyLock::new(|| Arc::new(Mutex::new(ObjectPool::default())));

fn align_to_next_page(addr: usize) -> usize {
    (addr + *PAGE_SIZE - 1) & !(*PAGE_SIZE - 1)
}

unsafe fn load_segment(segment: ElfSegment64, fd: i32, base_addr: *const u8) {
    let addr = unsafe { base_addr.add(segment.address() as usize) };
    let page_offset = addr as usize % *PAGE_SIZE;
    let aligned_addr = unsafe { addr.offset(-(page_offset as isize)) };
    let aligned_size = page_offset + segment.size() as usize;

    let mut prot = Default::default();
    if segment.permissions().writable() {
        prot |= nix::libc::PROT_WRITE;
    }
    if segment.permissions().readable() {
        prot |= nix::libc::PROT_READ;
    }

    let file_offset = segment.file_range().0;
    let file_size = segment.file_range().1;
    // The map offset determines where in the file the mapping starts.
    // Since the mapping starts at some `page_offset` before the segment address,
    // we need to adjust the file offset accordingly.
    let map_offset = file_offset as i64 - page_offset as i64;
    if map_offset < 0 {
        panic!("file mapping starts before page mapping: {}", map_offset);
    }

    println!(
        "Attempting mapping at address {:?} with size {:#x}",
        aligned_addr, aligned_size
    );
    // Map segment
    let mapped_addr = unsafe {
        nix::libc::mmap(
            aligned_addr as *mut nix::libc::c_void,
            aligned_size,
            prot,
            nix::libc::MAP_PRIVATE | nix::libc::MAP_FIXED_NOREPLACE,
            fd,
            map_offset,
        )
    } as *const u8;
    if mapped_addr as isize == -1 {
        unsafe {
            let c_str = std::ffi::CString::new("mmap failed").unwrap();
            nix::libc::perror(c_str.as_ptr());
        }
        panic!("mapping failed");
    }

    println!("mapping successful: {:?}", mapped_addr);

    let segment_end = mapped_addr as usize + aligned_size;
    let mapping_end = align_to_next_page(segment_end);
    // If the file mapping ends before the segment does, then we need to zero out the rest
    if segment.size() > file_size {
        let file_load_end = mapped_addr as usize + page_offset + file_size as usize;
        let zero_len = mapping_end - file_load_end;
        println!(
            "zeroing out from {:x} to {:x}",
            file_load_end,
            file_load_end + zero_len
        );
        unsafe {
            nix::libc::memset(file_load_end as *mut nix::libc::c_void, 0, zero_len);
        }
    }

    if segment.permissions().executable() {
        runner::define_exec_range(mapped_addr, mapping_end as *const u8);
    }
}

fn generate_suitable_base_addr(elf: &ElfFile64) -> *const u8 {
    let unaligned_end = elf
        .segments()
        .map(|segment| segment.address() + segment.size())
        .max()
        .unwrap_or(0);
    let end = align_to_next_page(unaligned_end as usize);

    let base_addr = unsafe {
        nix::libc::mmap(
            std::ptr::null_mut(),
            end,
            0,
            nix::libc::MAP_PRIVATE | nix::libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };

    unsafe {
        nix::libc::munmap(base_addr, end);
    }

    base_addr as *const u8
}

fn resolve_relocations(elf: &ElfFile64, base_addr: *mut u8, symbol_table: &SymbolTable) {
    let write_u64 = |addr: u64, value: u64| unsafe {
        *base_addr.add(addr as usize).cast() = value;
    };

    for (addr, reloc) in elf.dynamic_relocations().into_iter().flatten() {
        let ty = match reloc.flags() {
            RelocationFlags::Elf { r_type } => r_type,
            _ => unreachable!(),
        };
        match ty {
            elf::R_AARCH64_RELATIVE => {
                let value = base_addr as u64 + (addr as i64 + reloc.addend()) as u64;
                write_u64(addr, value);
            }
            elf::R_AARCH64_GLOB_DAT => {
                let symbol_idx = match reloc.target() {
                    RelocationTarget::Symbol(idx) => idx,
                    _ => unreachable!(),
                };
                let symbol = elf
                    .dynamic_symbol_table()
                    .unwrap()
                    .symbol_by_index(symbol_idx)
                    .unwrap();
                if symbol.is_weak() {
                    continue;
                }
                let name = symbol.name().unwrap();
                if let Some(&value) = symbol_table.global_symbols.get(name) {
                    write_u64(addr, value as u64);
                } else {
                    unimplemented!("GOT symbol lookup: {}", name);
                }
            }
            elf::R_AARCH64_JUMP_SLOT => {
                let symbol_idx = match reloc.target() {
                    RelocationTarget::Symbol(idx) => idx,
                    _ => unreachable!(),
                };
                let symbol = elf
                    .dynamic_symbol_table()
                    .unwrap()
                    .symbol_by_index(symbol_idx)
                    .unwrap();
                if symbol.is_weak() {
                    continue;
                }
                let name = symbol.name().unwrap();
                if let Some(&value) = symbol_table.global_symbols.get(name) {
                    write_u64(addr, value as u64);
                } else {
                    unimplemented!("PLT symbol lookup: {}", name);
                }
            }
            _ => {
                unimplemented!("Relocation: {:#?}", reloc);
            }
        }
    }
}

fn get_dlsym(name: &str) -> *const u8 {
    let name_c_str = CString::new(name).unwrap();
    unsafe {
        let handle = nix::libc::dlopen(std::ptr::null(), nix::libc::RTLD_LAZY);
        nix::libc::dlsym(handle, name_c_str.as_ptr()) as *const u8
    }
}

// Returns true if succeeded
fn try_load_wrapped(name: &str, symbol_table: &mut SymbolTable) -> bool {
    if name.starts_with("libc.so") {
        let names = ["abort", "puts"];
        for name in names {
            symbol_table
                .global_symbols
                .insert(name.to_string(), get_dlsym(name));
        }
        // TODO
        symbol_table
            .global_symbols
            .insert("__libc_start_main".to_string(), std::ptr::null());
        true
    } else {
        false
    }
}

fn collect_init_fini(elf: &ElfFile64, base_ptr: *const u8) -> (Vec<*const u8>, Vec<*const u8>) {
    let base_addr = base_ptr as u64;

    let mut init_array = Vec::new();
    let mut fini_array = Vec::new();

    let mut init_array_size = None;
    let mut fini_array_size = None;
    let mut init_array_ptr = None;
    let mut fini_array_ptr = None;

    let dynamic_table = elf.elf_dynamic_table().unwrap();
    for dynamic in dynamic_table.iter() {
        let addr_val = base_addr + dynamic.val;
        match dynamic.tag {
            elf::DT_INIT => init_array.insert(0, addr_val as *const u8),
            elf::DT_FINI => fini_array.insert(0, addr_val as *const u8),
            elf::DT_INIT_ARRAY => init_array_ptr = Some(addr_val as *const u64),
            elf::DT_FINI_ARRAY => fini_array_ptr = Some(addr_val as *const u64),
            elf::DT_INIT_ARRAYSZ => init_array_size = Some(dynamic.val),
            elf::DT_FINI_ARRAYSZ => fini_array_size = Some(dynamic.val),
            _ => {}
        }
    }

    // Read .init_array
    if let Some(size) = init_array_size
        && let Some(array_ptr) = init_array_ptr
    {
        for idx in 0..(size / 8) {
            let addr = unsafe { array_ptr.add(idx as usize).read() };
            init_array.push(addr as *const u8);
        }
    }

    // Read .fini_array
    if let Some(size) = fini_array_size
        && let Some(array_ptr) = fini_array_ptr
    {
        for idx in 0..(size / 8) {
            let addr = unsafe { array_ptr.add(idx as usize).read() };
            fini_array.push(addr as *const u8);
        }
    }

    (init_array, fini_array)
}

pub fn load_object(name: &str) -> usize {
    let mut object_pool = OBJECT_POOL.lock().unwrap();

    // Parse ELF
    let object_data = fs::read(name).unwrap();
    let elf = ElfFile64::parse(object_data.as_slice()).unwrap();

    // Load object dependencies
    let dynamic_table = elf.elf_dynamic_table().unwrap();
    for dynamic in dynamic_table.iter() {
        if dynamic.tag == elf::DT_NEEDED {
            let name = dynamic_table.string(dynamic).unwrap();
            let name = str::from_utf8(name).unwrap();
            if !try_load_wrapped(name, &mut object_pool.symbol_table) {
                unimplemented!("loading library {}", name);
            }
        }
    }

    // Load object segments
    let fd_handle = File::open(name).unwrap();
    let fd = fd_handle.as_raw_fd();
    let base_addr = generate_suitable_base_addr(&elf);
    for segment in elf.segments() {
        unsafe {
            load_segment(segment, fd, base_addr);
        }
    }

    // Apply relocations
    resolve_relocations(&elf, base_addr as *mut u8, &object_pool.symbol_table);

    // Collect initialization and finalization functions
    let (init_array, fini_array) = collect_init_fini(&elf, base_addr);

    let idx = object_pool.objects.len();
    object_pool.objects.push(EmuObject {
        base_ptr: base_addr,
        init_array,
        fini_array,
    });

    idx
}
