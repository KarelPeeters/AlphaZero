use std::collections::HashMap;
use std::sync::Mutex;

use itertools::Itertools;
use lazy_static::lazy_static;

use cuda_sys::wrapper::handle::Device;
use cuda_sys::wrapper::rtc::core::{CuFunction, CuModule};

lazy_static! {
    static ref KERNEL_CACHE: Mutex<HashMap<KernelKey, CuFunction>> = Mutex::new(HashMap::new());
    static ref HEADERS: HashMap<&'static str, &'static str> = {
        let mut map = HashMap::new();
        map.insert("util.cu", include_str!("util.cu"));
        map
    };
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct KernelKey {
    pub device: Device,
    pub source: String,
    pub func_name: String,
}

pub fn compile_cached_kernel(key: KernelKey) -> CuFunction {
    // keep locked for the duration of compilation
    let mut cache = KERNEL_CACHE.lock().unwrap();

    let func = cache.entry(key).or_insert_with_key(|key| {
        let module = CuModule::from_source(key.device, &key.source, None, &[&key.func_name], &HEADERS);

        if !module.log.is_empty() {
            eprintln!("Kernel source:\n{}\nLog:\n{}\n", key.source, module.log);
        }

        let lowered_names = module.lowered_names;

        let module = module.module.unwrap();
        let lowered_name = lowered_names.get(&key.func_name).unwrap();
        let func = module.get_function(lowered_name).unwrap();

        func
    });

    func.clone()
}

pub fn fill_replacements(src: &str, replacements: &[(&str, String)]) -> String {
    let result = replacements.iter().fold(src.to_owned(), |src, (key, value)| {
        assert!(src.contains(key), "Source does not contain key {}", key);
        src.replace(key, value)
    });

    if result.contains('$') {
        eprintln!("Source after replacements:\n{}", result);
        panic!("Source still contains $");
    }

    result
}

pub fn c_nested_array_string(values: &[Vec<isize>]) -> String {
    format!("{{{}}}", values.iter().map(|a| c_array_string(a)).join(", "))
}

pub fn c_array_string(values: &[isize]) -> String {
    format!("{{{}}}", values.iter().map(|v| v.to_string()).join(", "))
}

pub fn ceil_div(x: u32, y: u32) -> u32 {
    (x + y - 1) / y
}
