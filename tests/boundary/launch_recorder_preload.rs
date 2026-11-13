//! Linux `LD_PRELOAD` recorder for Skill-discovery process starts.
//!
//! The test harness compiles this file as a `cdylib` and loads it into an
//! isolated copy of itself. Each `posix_spawn`, `posix_spawnp`, `execv`,
//! `execve`, and `execvp` writes one line before the real libc function runs,
//! so a launch that has already exited is still in the log when a delayed
//! reader drains it. The line is one `write` to an `O_APPEND` file.

#![no_std]

use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicPtr, Ordering};

const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;
const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 64;
const O_APPEND: c_int = 1024;
const O_CLOEXEC: c_int = 0x80000;

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn open(path: *const c_char, flags: c_int, mode: u32) -> c_int;
    fn write(fd: c_int, buf: *const u8, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn getenv(name: *const c_char) -> *mut c_char;
    fn abort() -> !;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { abort() }
}

type SpawnFn = unsafe extern "C" fn(
    pid: *mut i32,
    file: *const c_char,
    file_actions: *const c_void,
    attrp: *const c_void,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int;
type ExecFileFn = unsafe extern "C" fn(file: *const c_char, argv: *const *mut c_char) -> c_int;
type ExecEnvFn = unsafe extern "C" fn(
    file: *const c_char,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int;

static REAL_SPAWN: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static REAL_SPAWNP: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static REAL_EXECV: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static REAL_EXECVE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static REAL_EXECVP: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

unsafe fn resolve(slot: &AtomicPtr<c_void>, name: &core::ffi::CStr) -> *mut c_void {
    let existing = slot.load(Ordering::Acquire);
    if !existing.is_null() {
        return existing;
    }
    let found = dlsym(RTLD_NEXT, name.as_ptr());
    if found.is_null() {
        abort();
    }
    slot.store(found, Ordering::Release);
    found
}

unsafe fn record(kind: &[u8], file: *const c_char) {
    let path = getenv(c"RUSTX_LAUNCH_LOG".as_ptr());
    if path.is_null() {
        return;
    }
    let mut line = [0u8; 576];
    let mut length = 0usize;
    for byte in kind {
        if length == line.len() {
            break;
        }
        line[length] = *byte;
        length += 1;
    }
    if length < line.len() {
        line[length] = b'\t';
        length += 1;
    }
    if !file.is_null() {
        let mut index = 0usize;
        while length < line.len() - 1 && *file.add(index) != 0 {
            let byte = *file.add(index) as u8;
            line[length] = if byte == b'\n' || byte == b'\r' {
                b'?'
            } else {
                byte
            };
            length += 1;
            index += 1;
        }
    }
    if length < line.len() {
        line[length] = b'\n';
        length += 1;
    }
    let fd = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0o644);
    if fd < 0 {
        return;
    }
    let _ = write(fd, line.as_ptr(), length);
    let _ = close(fd);
}

#[no_mangle]
pub unsafe extern "C" fn posix_spawn(
    pid: *mut i32,
    file: *const c_char,
    file_actions: *const c_void,
    attrp: *const c_void,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    record(b"posix_spawn", file);
    let real: SpawnFn = core::mem::transmute(resolve(&REAL_SPAWN, c"posix_spawn"));
    real(pid, file, file_actions, attrp, argv, envp)
}

#[no_mangle]
pub unsafe extern "C" fn posix_spawnp(
    pid: *mut i32,
    file: *const c_char,
    file_actions: *const c_void,
    attrp: *const c_void,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    record(b"posix_spawnp", file);
    let real: SpawnFn = core::mem::transmute(resolve(&REAL_SPAWNP, c"posix_spawnp"));
    real(pid, file, file_actions, attrp, argv, envp)
}

#[no_mangle]
pub unsafe extern "C" fn execv(file: *const c_char, argv: *const *mut c_char) -> c_int {
    record(b"execv", file);
    let real: ExecFileFn = core::mem::transmute(resolve(&REAL_EXECV, c"execv"));
    real(file, argv)
}

#[no_mangle]
pub unsafe extern "C" fn execve(
    file: *const c_char,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    record(b"execve", file);
    let real: ExecEnvFn = core::mem::transmute(resolve(&REAL_EXECVE, c"execve"));
    real(file, argv, envp)
}

#[no_mangle]
pub unsafe extern "C" fn execvp(file: *const c_char, argv: *const *mut c_char) -> c_int {
    record(b"execvp", file);
    let real: ExecFileFn = core::mem::transmute(resolve(&REAL_EXECVP, c"execvp"));
    real(file, argv)
}
