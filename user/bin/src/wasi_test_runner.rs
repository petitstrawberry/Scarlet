#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::string::String;
use alloc::vec::Vec;
use std::fs::File;
use std::io::Write;
use std::println;
use std::task::{execve, fork, waitpid};

struct Test {
    name: &'static str,
    args: &'static [&'static str],
}

const RUNNER: &str = "/bin/wasm-runtime";
const TEST_DIR: &str = "/root/wasi-tests";
const RESULT_FILE: &str = "/tmp/wasi_results.txt";

const TESTS_NO_ARGS: &[&str] = &[
    "args_sizes_get-no-arguments",
    "big_random_buf",
    "clock_getres-monotonic",
    "clock_getres-realtime",
    "clock_gettime-monotonic",
    "clock_gettime-realtime",
    "clock_time_get",
    "environ_sizes_get-no-variables",
    "proc_exit-failure",
    "proc_exit-success",
    "random_get-non-zero-length",
    "random_get-zero-length",
    "sched_yield",
];

const TESTS_WITH_TMP: &[&str] = &[
    "close_preopen",
    "dangling_fd",
    "fd_write-to-invalid-fd",
    "fd_write-to-stdout",
    "fopen-with-no-access",
    "lseek",
    "sock_shutdown-invalid_fd",
    "sock_shutdown-not_sock",
    "dangling_symlink",
    "dir_fd_op_failures",
    "directory_seek",
    "environ_get-multiple-variables",
    "environ_sizes_get-multiple-variables",
    "fd_advise",
    "fd_fdstat_set_rights",
    "fd_filestat_set",
    "fd_flags_set",
    "fd_readdir",
    "fdopendir-with-access",
    "file_allocate",
    "file_pread_pwrite",
    "file_seek_tell",
    "fstflags_validate",
    "interesting_paths",
    "isatty",
    "nofollow_errors",
    "overwrite_preopen",
    "path_exists",
    "path_filestat",
    "poll_oneoff_stdio",
    "pread-with-access",
    "pwrite-with-access",
    "pwrite-with-append",
    "stat-dev-ino",
    "symlink_create",
    "symlink_filestat",
    "symlink_loop",
    "truncation_rights",
    "unlink_file_trailing_slashes",
    "file_truncation",
    "file_unbuffered_write",
    "path_open_dirfd_not_dir",
    "path_rename_dir_trailing_slashes",
    "remove_directory_trailing_slashes",
    "remove_nonempty_directory",
    "fopen-with-access",
    "path_open_missing",
];

fn build_test_list() -> Vec<Test> {
    let mut tests = Vec::new();
    for &name in TESTS_NO_ARGS {
        tests.push(Test { name, args: &[] });
    }
    for &name in TESTS_WITH_TMP {
        tests.push(Test {
            name,
            args: &["/tmp"],
        });
    }
    tests
}

fn run_single_test(test: &Test) -> (bool, i32) {
    let wasm_path = alloc::format!("{}/{}.wasm", TEST_DIR, test.name);

    let mut argv = Vec::new();
    argv.push(RUNNER);
    argv.push(&wasm_path);
    for &arg in test.args {
        argv.push(arg);
    }

    let pid = fork();
    if pid == 0 {
        let _ = execve(RUNNER, &argv, &[]);
        return (false, 127);
    }

    let (reaped_pid, status) = waitpid(pid, 0);
    if reaped_pid < 0 {
        return (false, -1);
    }

    let exit_code = status;

    let pass = if test.name == "proc_exit-failure" {
        exit_code != 0
    } else {
        exit_code == 0
    };

    (pass, exit_code)
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let tests = build_test_list();

    let mut results = String::new();
    let mut pass_count: usize = 0;
    let mut fail_count: usize = 0;
    let mut failed_names = Vec::new();

    let total = tests.len();
    println!("Running {} WASI tests...", total);

    for (i, test) in tests.iter().enumerate() {
        let (pass, code) = run_single_test(test);
        let status = if pass { "PASS" } else { "FAIL" };

        if (i + 1) % 10 == 0 || !pass {
            println!("[{}/{}] {} (exit={})", i + 1, total, test.name, code);
        }

        if pass {
            pass_count += 1;
        } else {
            fail_count += 1;
            failed_names.push(test.name);
        }

        results.push_str(status);
        results.push_str(": ");
        results.push_str(test.name);
        results.push_str(" (exit=");
        let mut code_buf = [0u8; 12];
        let code_str = format_int(code, &mut code_buf);
        results.push_str(code_str);
        results.push('\n');
    }

    results.push_str(&alloc::format!(
        "\n=== RESULTS: PASS={} FAIL={} TOTAL={} ===\n",
        pass_count,
        fail_count,
        total
    ));

    if !failed_names.is_empty() {
        results.push_str("FAILED:");
        for name in &failed_names {
            results.push(' ');
            results.push_str(name);
        }
        results.push('\n');
    }

    if let Ok(mut f) = File::create(RESULT_FILE) {
        let _ = f.write_all(results.as_bytes());
    }

    println!(
        "=== RESULTS: PASS={} FAIL={} TOTAL={} ===",
        pass_count, fail_count, total
    );
    println!("Results written to {}", RESULT_FILE);

    0
}

fn format_int(mut val: i32, buf: &mut [u8; 12]) -> &str {
    if val == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let negative = val < 0;
    if negative {
        val = -val;
    }
    let mut pos = 12;
    while val > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    if negative {
        pos -= 1;
        buf[pos] = b'-';
    }
    core::str::from_utf8(&buf[pos..]).unwrap_or("?")
}
