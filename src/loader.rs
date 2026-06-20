use std::{
    collections::HashMap,
    fs,
    io::Cursor,
    sync::{Arc, LazyLock, Mutex},
};

use object::{LittleEndian, Object, read::elf::ElfFile64};

#[derive(Default)]
struct ObjectPool {
    objects: Vec<EmuObject>,
}

struct CompiledPage {
    data: Vec<u8>,
}

struct EmuObject {
    object_data: Vec<u8>,
    executable_map: HashMap<u64, CompiledPage>,
}

impl EmuObject {
    fn new(object_data: Vec<u8>) -> Self {
        Self {
            object_data,
            executable_map: HashMap::new(),
        }
    }

    fn addr(&self) -> u64 {
        self.object_data.as_ptr() as u64
    }
}

static OBJECT_POOL: LazyLock<Arc<Mutex<ObjectPool>>> =
    LazyLock::new(|| Arc::new(Mutex::new(ObjectPool::default())));

fn load_object(name: &str) -> usize {
    let mut object_pool = OBJECT_POOL.lock().unwrap();

    let mut object_data = fs::read(name).unwrap();
    let elf = ElfFile64::<LittleEndian>::parse(object_data.as_slice()).unwrap();

    let mut rel_data = object_data.clone();

    todo!();
}
