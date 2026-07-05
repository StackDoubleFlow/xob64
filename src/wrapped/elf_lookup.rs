use std::ffi::{CStr, CString};

use nix::libc;
use object::{LittleEndian, elf, read::elf::Dyn};

pub struct LoadedObjectInfo {
    pub path: CString,
    pub base_addr: u64,
    phdr: *const libc::Elf64_Phdr,
    phnum: u16,
}

pub fn loaded_object_info(name: &CStr) -> Option<LoadedObjectInfo> {
    // Normally, we could use dlinfo here, but that isn't supported on android
    struct IterateCtx {
        name: *const i8,
        obj_info: Option<LoadedObjectInfo>,
    }
    unsafe extern "C" fn callback(
        info: *mut libc::dl_phdr_info,
        _size: libc::size_t,
        data: *mut libc::c_void,
    ) -> libc::c_int {
        unsafe {
            let ctx = &mut *(data as *mut IterateCtx);
            let info = &*info;
            let search_name = CStr::from_ptr(ctx.name);
            let path = CStr::from_ptr(info.dlpi_name);
            if path.to_bytes().ends_with(search_name.to_bytes()) {
                ctx.obj_info = Some(LoadedObjectInfo {
                    path: path.to_owned(),
                    base_addr: info.dlpi_addr,
                    phdr: info.dlpi_phdr,
                    phnum: info.dlpi_phnum,
                });
                1
            } else {
                0
            }
        }
    }
    let mut ctx = IterateCtx {
        name: name.as_ptr(),
        obj_info: None,
    };
    unsafe {
        nix::libc::dl_iterate_phdr(Some(callback), &mut ctx as *mut _ as _);
    }
    ctx.obj_info
}

#[repr(C)]
#[derive(Clone, Debug)]
struct GnuHashTableHeader {
    nbuckets: u32,
    symoffset: u32,
    bloom_size: u32,
    bloom_shift: u32,
}

#[derive(Debug)]
struct GnuHashTable {
    header: GnuHashTableHeader,
    bloom: *const u64,
    buckets: *const u32,
    chain: *const u32,
}

impl GnuHashTable {
    fn bloom(&self) -> &[u64] {
        unsafe { std::slice::from_raw_parts(self.bloom, self.header.bloom_size as usize) }
    }

    fn buckets(&self) -> &[u32] {
        unsafe { std::slice::from_raw_parts(self.buckets, self.header.nbuckets as usize) }
    }

    unsafe fn chain_at(&self, idx: usize) -> u32 {
        unsafe { self.chain.add(idx).read() }
    }
}

pub struct SymbolFinder {
    hash_table: GnuHashTable,
    symtab: *const libc::Elf64_Sym,
    strtab: *const i8,
}

impl SymbolFinder {
    pub fn new(obj_info: &LoadedObjectInfo) -> Self {
        let phdrs = unsafe { std::slice::from_raw_parts(obj_info.phdr, obj_info.phnum as usize) };
        let dyn_header = phdrs
            .iter()
            .find(|phdr| phdr.p_type == libc::PT_DYNAMIC)
            .expect("could not find PT_DYNAMIC");

        let mut strtab = None;
        let mut symtab = None;
        let mut gnu_hash = None;

        let mut dyn_entry =
            (obj_info.base_addr + dyn_header.p_vaddr) as *const elf::Dyn64<LittleEndian>;
        while unsafe { (*dyn_entry).d_tag(LittleEndian) } != elf::DT_NULL {
            let d_tag = unsafe { (*dyn_entry).d_tag(LittleEndian) };
            let d_val = unsafe { (*dyn_entry).d_val(LittleEndian) };
            match d_tag {
                elf::DT_STRTAB => strtab = Some(d_val),
                elf::DT_SYMTAB => symtab = Some(d_val),
                elf::DT_GNU_HASH => gnu_hash = Some(d_val),
                _ => {}
            }
            dyn_entry = dyn_entry.wrapping_add(1);
        }
        let strtab = strtab.expect("missing DT_STRTAB");
        let symtab = symtab.expect("missing DT_SYMTAB");
        let gnu_hash = gnu_hash.expect("missing DT_GNU_HASH");
        let hash_table_header = unsafe { (*(gnu_hash as *const GnuHashTableHeader)).clone() };
        let bloom = gnu_hash as usize + size_of::<GnuHashTableHeader>();
        let buckets = bloom + hash_table_header.bloom_size as usize * size_of::<u64>();
        let chain = buckets + hash_table_header.nbuckets as usize * size_of::<u32>();
        let hash_table = GnuHashTable {
            header: hash_table_header,
            bloom: bloom as _,
            buckets: buckets as _,
            chain: chain as _,
        };
        SymbolFinder {
            hash_table,
            strtab: strtab as _,
            symtab: symtab as _,
        }
    }

    pub fn lookup(&self, name: &CStr) -> Option<*const libc::Elf64_Sym> {
        let hash = elf::gnu_hash(name.to_bytes());
        let bloom_idx = (hash / 64) % self.hash_table.header.bloom_size;
        let bloom_word = self.hash_table.bloom()[bloom_idx as usize];
        if !bit_is_set(bloom_word, hash % 64)
            || !bit_is_set(
                bloom_word,
                (hash >> self.hash_table.header.bloom_shift) % 64,
            )
        {
            return None;
        }
        let bucket_idx = hash % self.hash_table.header.nbuckets;
        let mut chain_idx = self.hash_table.buckets()[bucket_idx as usize];
        if chain_idx < self.hash_table.header.symoffset {
            return None;
        }
        chain_idx -= self.hash_table.header.symoffset;
        loop {
            let chain_hash = unsafe { self.hash_table.chain_at(chain_idx as usize) };
            if chain_hash >> 1 == hash >> 1 {
                let sym_idx = chain_idx + self.hash_table.header.symoffset;
                let sym = self.symtab.wrapping_add(sym_idx as usize);
                let sym_ref = unsafe { &*sym };
                let sym_name =
                    unsafe { CStr::from_ptr(self.strtab.wrapping_add(sym_ref.st_name as usize)) };
                if sym_name == name {
                    return Some(sym);
                }
            }
            if (chain_hash & 1) == 1 {
                return None;
            }
            chain_idx += 1;
        }
    }
}

fn bit_is_set(num: u64, idx: u32) -> bool {
    (num & (1 << idx)) != 0
}
