use std::{
    collections::HashMap,
    ffi::{CStr, CString, OsStr},
    fs::{self, File},
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    sync::{Arc, LazyLock, Mutex},
};

use nix::libc;
use object::{
    Object, ObjectSegment, ObjectSymbol, ObjectSymbolTable, RelocationFlags, RelocationTarget, elf,
    read::elf::{ElfFile64, ElfSegment64, Sym},
};

use crate::{runner, wrapped::WrappedLib};

pub const PAGE_SIZE: LazyLock<usize> =
    LazyLock::new(|| unsafe { libc::sysconf(libc::_SC_PAGE_SIZE) as usize });

#[derive(Default)]
struct ObjectPool {
    objects: Vec<EmuObject>,
    wrapped_libs: Vec<WrappedLib>,
    global_symbols: HashMap<CString, *const u8>,
}

// *const u8 is not normally Send
unsafe impl Send for ObjectPool {}

impl ObjectPool {
    fn get_symbol(&mut self, name: &CStr) -> *const u8 {
        if let Some(&addr) = self.global_symbols.get(name) {
            return addr;
        }
        for lib in &self.wrapped_libs {
            if let Some(addr) = lib.get_symbol(name) {
                self.global_symbols.insert(name.to_owned(), addr);
                return addr;
            }
        }
        panic!("could not locate symbol: {:?}", name);
    }
}

struct EmuObject {
    base_ptr: *const u8,
    fini_array: Vec<*const u8>,
}

// impl SymbolTable {
//     pub fn insert_global(&mut self, name: &CStr, addr: *const ()) {
//         self.global_symbols
//             .insert(name.to_owned(), addr as *const u8);
//     }
// }

static OBJECT_POOL: LazyLock<Arc<Mutex<ObjectPool>>> =
    LazyLock::new(|| Arc::new(Mutex::new(ObjectPool::default())));

fn align_to_next(addr: usize, alignment: usize) -> usize {
    (addr + alignment - 1) & !(alignment - 1)
}

unsafe fn load_segment(segment: ElfSegment64, fd: i32, base_addr: *const u8) {
    let alignment = (segment.align() as usize).max(*PAGE_SIZE);
    let addr = unsafe { base_addr.add(segment.address() as usize) };
    let page_offset = addr as usize % alignment;
    let aligned_addr = unsafe { addr.offset(-(page_offset as isize)) };
    let aligned_size = page_offset + segment.size() as usize;

    let mut prot = Default::default();
    if segment.permissions().writable() {
        prot |= libc::PROT_WRITE;
    }
    if segment.permissions().readable() {
        prot |= libc::PROT_READ;
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

    eprintln!(
        "Attempting mapping at address {:?} with size {:#x}",
        aligned_addr, aligned_size
    );
    // Map segment
    let mapped_addr = unsafe {
        libc::mmap(
            aligned_addr as *mut libc::c_void,
            aligned_size,
            prot,
            libc::MAP_PRIVATE | libc::MAP_FIXED_NOREPLACE,
            fd,
            map_offset,
        )
    } as *const u8;
    if mapped_addr as isize == -1 {
        unsafe {
            libc::perror(c"mmap failed".as_ptr());
        }
        panic!(
            "mapping failed: {:?} ({}), offset: {}",
            aligned_addr, aligned_size, map_offset
        );
    }

    eprintln!("mapping successful: {:?}", mapped_addr);

    let segment_end = mapped_addr as usize + aligned_size;
    let mapping_end = align_to_next(segment_end, *PAGE_SIZE);
    // If the file mapping ends before the segment does, then we need to zero out the rest
    if segment.size() > file_size {
        let file_load_end = mapped_addr as usize + page_offset + file_size as usize;
        eprintln!("zeroing out from {:x} to {:x}", file_load_end, mapping_end);
        let page_break = align_to_next(file_load_end, *PAGE_SIZE);
        unsafe {
            // Write 0 from the end of the file to end of the page
            libc::memset(
                file_load_end as *mut libc::c_void,
                0,
                page_break - file_load_end,
            );
        }
        // If the mapping ends beyond the last page of the file mapping, we need to create a new anonymous mapping for it and memset it to 0
        if page_break < mapping_end {
            unsafe {
                libc::mmap(
                    page_break as *mut libc::c_void,
                    mapping_end - page_break,
                    prot,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED,
                    -1,
                    0,
                );
                libc::memset(page_break as *mut libc::c_void, 0, mapping_end - page_break);
            }
        }
    }

    if segment.permissions().executable() {
        runner::define_exec_range(mapped_addr, mapping_end as *const u8);
    }
}

fn generate_suitable_base_addr(elf: &ElfFile64) -> *const u8 {
    let base_alignment = (elf
        .segments()
        .find(|segment| segment.address() == 0)
        .expect("could not find base segment for alignment")
        .align() as usize)
        .max(*PAGE_SIZE);
    let unaligned_end = elf
        .segments()
        .map(|segment| segment.address() + segment.size())
        .max()
        .unwrap_or(0);
    let end = align_to_next(unaligned_end as usize, *PAGE_SIZE) + base_alignment - *PAGE_SIZE;

    let mapped_addr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            end,
            0,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };

    if mapped_addr as isize == -1 {
        unsafe {
            libc::perror(c"mmap failed".as_ptr());
        }
        panic!("mapping failed");
    }

    unsafe {
        libc::munmap(mapped_addr, end);
    }

    let base_addr = align_to_next(mapped_addr as usize, base_alignment);
    base_addr as *const u8
}

fn resolve_relocations(obj_pool: &mut ObjectPool, elf: &ElfFile64, base_addr: *mut u8) {
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
                let value = base_addr as u64 + reloc.addend() as u64;
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
                let name_bytes = symbol.name_bytes().unwrap();
                let name = CString::new(name_bytes).unwrap();
                write_u64(addr, obj_pool.get_symbol(&name) as u64);
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
                let name_bytes = symbol.name_bytes().unwrap();
                let name = CString::new(name_bytes).unwrap();
                write_u64(addr, obj_pool.get_symbol(&name) as u64);
            }
            _ => {
                unimplemented!("Relocation: {:#?}", reloc);
            }
        }
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

pub fn load_object(name: &CStr, args: &[*const u8]) -> usize {
    let name = OsStr::from_bytes(name.to_bytes());
    let mut object_pool = OBJECT_POOL.lock().unwrap();

    // Parse ELF
    let object_data = fs::read(name).unwrap();
    let elf = ElfFile64::parse(object_data.as_slice()).unwrap();

    // Load object dependencies
    let dynamic_table = elf.elf_dynamic_table().unwrap();
    for dynamic in dynamic_table.iter() {
        if dynamic.tag == elf::DT_NEEDED {
            let name_bytes = dynamic_table.string(dynamic).unwrap();
            if !object_pool
                .wrapped_libs
                .iter()
                .any(|lib| lib.path.as_bytes().ends_with(name_bytes))
            {
                let name = CString::new(name_bytes.to_vec()).unwrap();
                let wrapped_lib = WrappedLib::try_load(&name).unwrap();
                object_pool.wrapped_libs.push(wrapped_lib);
            };
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
    resolve_relocations(&mut object_pool, &elf, base_addr as *mut u8);

    // Collect initialization and finalization functions
    let (init_array, fini_array) = collect_init_fini(&elf, base_addr);

    let idx = object_pool.objects.len();
    object_pool.objects.push(EmuObject {
        base_ptr: base_addr,
        fini_array,
    });

    for symbol in elf.elf_symbol_table().symbols() {
        let shndx = symbol.st_shndx(elf.endianness());
        let bind = symbol.st_bind();
        let other = symbol.st_other();
        if shndx != elf::SHN_UNDEF
            && (bind == elf::STB_GLOBAL || bind == elf::STB_WEAK)
            && other == elf::STV_DEFAULT
        {
            let name_bytes = symbol
                .name(elf.endianness(), elf.elf_symbol_table().strings())
                .unwrap();
            let name = CString::new(name_bytes.to_vec()).unwrap();
            let addr = symbol.st_value(elf.endianness()) as usize;

            object_pool
                .global_symbols
                .insert(name, unsafe { base_addr.add(addr) });
        }
    }

    // Release the lock
    drop(object_pool);

    for &ptr in &init_array {
        runner::call(ptr, args);
    }

    idx
}

pub fn get_symbol(name: &CStr) -> Option<*const u8> {
    let object_pool = OBJECT_POOL.lock().unwrap();
    object_pool.global_symbols.get(name).cloned()
}
