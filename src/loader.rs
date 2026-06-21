use std::{
    collections::HashMap,
    fs::{self, File},
    io::Cursor,
    os::fd::AsRawFd,
    sync::{Arc, LazyLock, Mutex},
};

use object::{
    Endianness, LittleEndian, Object, ObjectSegment,
    elf::{self, ProgramHeader64},
    read::elf::{ElfFile64, ElfSegment64},
};

const PAGE_SIZE: LazyLock<usize> =
    LazyLock::new(|| unsafe { nix::libc::sysconf(nix::libc::_SC_PAGE_SIZE) as usize });

#[derive(Default)]
struct ObjectPool {
    objects: Vec<EmuObject>,
}

struct CompiledPage {
    data: Vec<u8>,
}

struct ExecutableRange {
    start: *const u8,
    end: *const u8,
}

struct EmuObject {
    base_ptr: *const u8,
    // We don't actually mark any ARM64 memory as executable, but we instead keep track of their ranges here.
    // When they are attempted to be executed, we dynamically compile them to x86_64.
    exec_ranges: Vec<ExecutableRange>,
    executable_map: HashMap<u64, CompiledPage>,
}

// *const u8 is not normally Send
unsafe impl Send for EmuObject {}

impl EmuObject {
    fn new(base_ptr: *const u8, exec_ranges: Vec<ExecutableRange>) -> Self {
        Self {
            base_ptr,
            exec_ranges,
            executable_map: HashMap::new(),
        }
    }
}

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

    // If the file mapping ends before the segment does, then we need to zero out the rest
    if segment.size() > file_size {
        unsafe {
            let segment_end = mapped_addr as usize + aligned_size;
            let mapping_end = align_to_next_page(segment_end);
            let file_load_end = mapped_addr as usize + page_offset + file_size as usize;
            let zero_len = mapping_end - file_load_end;
            println!(
                "zeroing out from {:x} to {:x}",
                file_load_end,
                file_load_end + zero_len
            );
            nix::libc::memset(file_load_end as *mut nix::libc::c_void, 0, zero_len);
        }
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

pub fn load_object(name: &str) -> usize {
    let mut object_pool = OBJECT_POOL.lock().unwrap();

    let fd_handle = File::open(name).unwrap();
    let fd = fd_handle.as_raw_fd();

    let mut object_data = fs::read(name).unwrap();
    let elf = ElfFile64::parse(object_data.as_slice()).unwrap();

    let mut rel_data = object_data.clone();

    let mut base_addr = generate_suitable_base_addr(&elf);
    for segment in elf.segments() {
        unsafe {
            load_segment(segment, fd, base_addr);
        }
    }

    todo!();
}
