#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use std::{format, println, vec::Vec};

const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

unsafe extern "C" fn host_write_fn(ptr: *const u8, len: usize) {
    let buf = unsafe { core::slice::from_raw_parts(ptr, len) };
    std::io::stdout().write_all(buf).ok();
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<std::string::String> = std::env::args().collect();

    if args.len() < 2 {
        println!("wasm-runtime: missing wasm file operand");
        println!("usage: wasm-runtime <file.wasm> [args...]");
        return 1;
    }

    let wasm_path = &args[1];
    let wasm_args = if args.len() > 2 { &args[2..] } else { &[] };

    match run_wasm(wasm_path, wasm_args) {
        Ok(code) => {
            println!("wasm-runtime: exited with code {}", code);
            code
        }
        Err(e) => {
            println!("wasm-runtime: {}: {}", wasm_path, e);
            1
        }
    }
}

fn run_wasm(wasm_path: &str, _args: &[std::string::String]) -> Result<i32, std::string::String> {
    let mut file = std::fs::File::open(wasm_path).map_err(|_| format!("cannot open file"))?;

    let mut header = [0u8; 8];
    file.read(&mut header)
        .map_err(|_| format!("cannot read file header"))?;

    if header[..4] != WASM_MAGIC {
        return Err(format!("not a valid wasm file"));
    }

    let mut wasm_bytes = Vec::new();
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|_| format!("seek failed"))?;

    let mut buf = [0u8; 4096];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => wasm_bytes.extend_from_slice(&buf[..n]),
            Err(_) => return Err(format!("read failed")),
        }
    }

    execute_wasm(&wasm_bytes)
}

fn execute_wasm(wasm_bytes: &[u8]) -> Result<i32, std::string::String> {
    use std::handle::capability::memory_mapping::{flags, mmap_anonymous, prot};
    use wasm_jit::engine;

    fn scarlet_exec_alloc(size: usize) -> *mut u8 {
        let page_size = 4096;
        let pages = (size + page_size - 1) / page_size;
        match mmap_anonymous(
            0,
            pages * page_size,
            prot::READ | prot::WRITE | prot::EXEC,
            flags::PRIVATE,
        ) {
            Ok(addr) => addr as *mut u8,
            Err(()) => core::ptr::null_mut(),
        }
    }
    engine::set_exec_allocator(scarlet_exec_alloc);

    let module =
        engine::compile_module(wasm_bytes).map_err(|e| format!("compile error: {:?}", e))?;

    let memory_pages = module
        .data_segments
        .iter()
        .map(|s| (s.offset as usize + s.data.len() + 65535) / 65536)
        .max()
        .unwrap_or(1)
        .max(1);
    let mut memory = alloc::vec![0u8; memory_pages * 65536];
    module.init_memory(&mut memory);

    let mut ctx =
        wasm_jit::runtime::VmContext::new(memory.as_mut_ptr(), memory.len(), core::ptr::null(), 0);
    ctx.host_write = Some(host_write_fn);

    let imported_names: alloc::vec::Vec<wasm_jit::runtime::ImportedFuncName> = module
        .imported_funcs
        .iter()
        .map(|f| wasm_jit::runtime::ImportedFuncName {
            module: f.module.as_ptr(),
            module_len: f.module.len(),
            name: f.name.as_ptr(),
            name_len: f.name.len(),
        })
        .collect();
    let imported_names_box = imported_names.into_boxed_slice();
    ctx.imported_names = imported_names_box.as_ptr();
    ctx.imported_count = imported_names_box.len();
    core::mem::forget(imported_names_box);

    unsafe {
        core::arch::asm!("fence.i");
        match engine::invoke_export(&module, &mut ctx, "_start", &[]) {
            Ok(_) => Ok(0),
            Err(trap) => {
                if ctx.exited {
                    Ok(ctx.exit_code as i32)
                } else {
                    Err(format!("trap: {:?}", trap))
                }
            }
        }
    }
}
