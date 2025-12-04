#![allow(nonstandard_style)]
#![allow(unsafe_op_in_unsafe_fn)]
// miri has some special hacks here that make things unused.
#![cfg_attr(miri, allow(unused))]

#[cfg(test)]
mod tests;

#[cfg(all(target_os = "linux", target_env = "gnu"))]
use libc::c_char;
#[cfg(any(
    all(target_os = "linux", not(target_env = "musl")),
    target_os = "android",
    target_os = "fuchsia",
    target_os = "hurd",
    target_os = "illumos",
    target_vendor = "apple",
))]
use libc::dirfd;
#[cfg(any(target_os = "fuchsia", target_os = "illumos", target_vendor = "apple"))]
use libc::fstatat as fstatat64;
#[cfg(any(all(target_os = "linux", not(target_env = "musl")), target_os = "hurd"))]
use libc::fstatat64;
#[cfg(any(
    target_os = "aix",
    target_os = "android",
    target_os = "freebsd",
    target_os = "fuchsia",
    target_os = "illumos",
    target_os = "nto",
    target_os = "redox",
    target_os = "solaris",
    target_os = "vita",
    target_os = "wasi",
    all(target_os = "linux", target_env = "musl"),
))]
use libc::readdir as readdir64;
#[cfg(not(any(
    target_os = "aix",
    target_os = "android",
    target_os = "freebsd",
    target_os = "fuchsia",
    target_os = "hurd",
    target_os = "illumos",
    target_os = "l4re",
    target_os = "linux",
    target_os = "nto",
    target_os = "redox",
    target_os = "solaris",
    target_os = "vita",
    target_os = "wasi",
)))]
use libc::readdir_r as readdir64_r;
#[cfg(any(all(target_os = "linux", not(target_env = "musl")), target_os = "hurd"))]
use libc::readdir64;
#[cfg(target_os = "l4re")]
use libc::readdir64_r;
use libc::{c_int, mode_t};
#[cfg(target_os = "android")]
use libc::{
    dirent as dirent64, fstat as fstat64, fstatat as fstatat64, ftruncate64, lseek64,
    lstat as lstat64, off64_t, open as open64, stat as stat64,
};
#[cfg(not(any(
    all(target_os = "linux", not(target_env = "musl")),
    target_os = "l4re",
    target_os = "android",
    target_os = "hurd",
)))]
use libc::{
    dirent as dirent64, fstat as fstat64, ftruncate as ftruncate64, lseek as lseek64,
    lstat as lstat64, off_t as off64_t, open as open64, stat as stat64,
};
#[cfg(any(
    all(target_os = "linux", not(target_env = "musl")),
    target_os = "l4re",
    target_os = "hurd"
))]
use libc::{dirent64, fstat64, ftruncate64, lseek64, lstat64, off64_t, open64, stat64};

use crate::ffi::{CStr, OsStr, OsString};
use crate::fmt::{self, Write as _};
use crate::fs::TryLockError;
use crate::io::{self, BorrowedCursor, Error, IoSlice, IoSliceMut, SeekFrom};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd};
#[cfg(target_family = "unix")]
use crate::os::unix::prelude::*;
#[cfg(target_os = "wasi")]
use crate::os::wasi::prelude::*;
use crate::path::{Path, PathBuf};
use crate::pin::Pin;
use crate::sync::Arc;
use crate::sys::common::small_c_string::run_path_with_cstr;
use crate::sys::fd::FileDesc;
pub use crate::sys::fs::common::exists;
use crate::sys::time::SystemTime;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
use crate::sys::weak::syscall;
#[cfg(target_os = "android")]
use crate::sys::weak::weak;
use crate::sys::{AsInner, AsInnerMut, FromInner, IntoInner, cvt, cvt_r};
use crate::{mem, ptr};

pub struct File(FileDesc);

// FIXME: This should be available on Linux with all `target_env`.
// But currently only glibc exposes `statx` fn and structs.
// We don't want to import unverified raw C structs here directly.
// https://github.com/rust-lang/rust/pull/67774
macro_rules! cfg_has_statx {
    ({ $($then_tt:tt)* } else { $($else_tt:tt)* }) => {
        cfg_select! {
            all(target_os = "linux", any(target_env = "", target_env = "gnu")) => {
                $($then_tt)*
            }
            _ => {
                $($else_tt)*
            }
        }
    };
    ($($block_inner:tt)*) => {
        #[cfg(all(target_os = "linux", any(target_env = "", target_env = "gnu")))]
        {
            $($block_inner)*
        }
    };
}

cfg_has_statx! {{
    #[derive(Clone)]
    pub struct FileAttr {
        stat: stat64,
        statx_extra_fields: Option<StatxExtraFields>,
    }

    #[derive(Clone)]
    struct StatxExtraFields {
        // This is needed to check if btime is supported by the filesystem.
        stx_mask: u32,
        stx_btime: libc::statx_timestamp,
        // With statx, we can overcome 32-bit `time_t` too.
        #[cfg(target_pointer_width = "32")]
        stx_atime: libc::statx_timestamp,
        #[cfg(target_pointer_width = "32")]
        stx_ctime: libc::statx_timestamp,
        #[cfg(target_pointer_width = "32")]
        stx_mtime: libc::statx_timestamp,

    }

    // We prefer `statx` on Linux if available, which contains file creation time,
    // as well as 64-bit timestamps of all kinds.
    // Default `stat64` contains no creation time and may have 32-bit `time_t`.
    unsafe fn try_statx(
        fd: c_int,
        path: *const c_char,
        flags: i32,
        mask: u32,
    ) -> Option<io::Result<FileAttr>> {
        use crate::sync::atomic::{Atomic, AtomicU8, Ordering};

        // Linux kernel prior to 4.11 or glibc prior to glibc 2.28 don't support `statx`.
        // We check for it on first failure and remember availability to avoid having to
        // do it again.
        #[repr(u8)]
        enum STATX_STATE{ Unknown = 0, Present, Unavailable }
        static STATX_SAVED_STATE: Atomic<u8> = AtomicU8::new(STATX_STATE::Unknown as u8);

        syscall!(
            fn statx(
                fd: c_int,
                pathname: *const c_char,
                flags: c_int,
                mask: libc::c_uint,
                statxbuf: *mut libc::statx,
            ) -> c_int;
        );

        let statx_availability = STATX_SAVED_STATE.load(Ordering::Relaxed);
        if statx_availability == STATX_STATE::Unavailable as u8 {
            return None;
        }

        let mut buf: libc::statx = mem::zeroed();
        if let Err(err) = cvt(statx(fd, path, flags, mask, &mut buf)) {
            if STATX_SAVED_STATE.load(Ordering::Relaxed) == STATX_STATE::Present as u8 {
                return Some(Err(err));
            }

            // We're not yet entirely sure whether `statx` is usable on this kernel
            // or not. Syscalls can return errors from things other than the kernel
            // per se, e.g. `EPERM` can be returned if seccomp is used to block the
            // syscall, or `ENOSYS` might be returned from a faulty FUSE driver.
            //
            // Availability is checked by performing a call which expects `EFAULT`
            // if the syscall is usable.
            //
            // See: https://github.com/rust-lang/rust/issues/65662
            //
            // FIXME what about transient conditions like `ENOMEM`?
            let err2 = cvt(statx(0, ptr::null(), 0, libc::STATX_BASIC_STATS | libc::STATX_BTIME, ptr::null_mut()))
                .err()
                .and_then(|e| e.raw_os_error());
            if err2 == Some(libc::EFAULT) {
                STATX_SAVED_STATE.store(STATX_STATE::Present as u8, Ordering::Relaxed);
                return Some(Err(err));
            } else {
                STATX_SAVED_STATE.store(STATX_STATE::Unavailable as u8, Ordering::Relaxed);
                return None;
            }
        }
        if statx_availability == STATX_STATE::Unknown as u8 {
            STATX_SAVED_STATE.store(STATX_STATE::Present as u8, Ordering::Relaxed);
        }

        // We cannot fill `stat64` exhaustively because of private padding fields.
        let mut stat: stat64 = mem::zeroed();
        // `c_ulong` on gnu-mips, `dev_t` otherwise
        stat.st_dev = libc::makedev(buf.stx_dev_major, buf.stx_dev_minor) as _;
        stat.st_ino = buf.stx_ino as libc::ino64_t;
        stat.st_nlink = buf.stx_nlink as libc::nlink_t;
        stat.st_mode = buf.stx_mode as libc::mode_t;
        stat.st_uid = buf.stx_uid as libc::uid_t;
        stat.st_gid = buf.stx_gid as libc::gid_t;
        stat.st_rdev = libc::makedev(buf.stx_rdev_major, buf.stx_rdev_minor) as _;
        stat.st_size = buf.stx_size as off64_t;
        stat.st_blksize = buf.stx_blksize as libc::blksize_t;
        stat.st_blocks = buf.stx_blocks as libc::blkcnt64_t;
        stat.st_atime = buf.stx_atime.tv_sec as libc::time_t;
        // `i64` on gnu-x86_64-x32, `c_ulong` otherwise.
        stat.st_atime_nsec = buf.stx_atime.tv_nsec as _;
        stat.st_mtime = buf.stx_mtime.tv_sec as libc::time_t;
        stat.st_mtime_nsec = buf.stx_mtime.tv_nsec as _;
        stat.st_ctime = buf.stx_ctime.tv_sec as libc::time_t;
        stat.st_ctime_nsec = buf.stx_ctime.tv_nsec as _;

        let extra = StatxExtraFields {
            stx_mask: buf.stx_mask,
            stx_btime: buf.stx_btime,
            // Store full times to avoid 32-bit `time_t` truncation.
            #[cfg(target_pointer_width = "32")]
            stx_atime: buf.stx_atime,
            #[cfg(target_pointer_width = "32")]
            stx_ctime: buf.stx_ctime,
            #[cfg(target_pointer_width = "32")]
            stx_mtime: buf.stx_mtime,
        };

        Some(Ok(FileAttr { stat, statx_extra_fields: Some(extra) }))
    }

} else {
    #[derive(Clone)]
    pub struct FileAttr {
        stat: stat64,
    }
}}

// FIXME: would be very helpful to define named cfg(...) arguments instead of repeating them
//        ad nauseam!
macro_rules! cfg_has_getdents {
    ( $it:item ) => {
        #[cfg(any(
            target_os = "linux",
            target_os = "hurd",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        $it
    };
    ( $ex:expr ) => {
        #[cfg(any(
            target_os = "linux",
            target_os = "hurd",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        $ex
    };
}

macro_rules! cfg_select_has_getdents {
    ( => { $($then_tt:tt)* } $($else_tt:tt)* ) => {
        cfg_select! {
            any(
                target_os = "linux",
                target_os = "hurd",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd",

            ) => {
                $($then_tt)*
            }
            $($else_tt)*
        }
    };
    ( => $ex:expr ) => {
        cfg_select! {
            any(
                target_os = "linux",
                target_os = "hurd",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd",

            ) => $ex
        }
    };
}

// Implementation using the available `getdents()` method.
cfg_has_getdents! {
mod getdents_impl {
    use super::buffer_state::ChunkedStreamResult;
    use core::slice::memchr;
    use crate::os::fd;
    use crate::{ffi, io, iter, mem, num, ops, ptr, slice};

    // TODO: pull this in from the libc crate itself when it's in:
    //       https://github.com/rust-lang/libc/pull/4522
    cfg_select! {
        all(target_os = "linux", target_env = "musl") => {
            // FIXME: switch to using posix_getdents() when available! currently not in the musl
            //        that ships with rust, although musl released this in early 2025.
            use super::dirent64 as dent_struct;

            unsafe extern "C" {
                fn getdents64(fd: libc::c_int, buf: *mut libc::c_void, nbytes: usize) -> isize;
            }

            use getdents64 as getdents_fn;
        }
        // i.e. all glibc platforms:
        all(
            any(target_os = "linux", target_os = "hurd"),
            any(target_env = "", target_env = "gnu"),
        ) => {
            use super::dirent64 as dent_struct;

            unsafe extern "C" {
                fn getdents64(fd: libc::c_int, buf: *mut libc::c_void, nbytes: usize) -> isize;
            }

            use getdents64 as getdents_fn;
        }
        target_os = "dragonfly" => {
            use super::dirent64 as dent_struct;

            unsafe extern "C" {
                fn getdents(fd: libc::c_int, buf: *mut libc::c_char, nbytes: usize) -> libc::c_int;
            }

            use getdents as getdents_fn;
        }
        target_os = "freebsd" => {
            use super::dirent64 as dent_struct;

            unsafe extern "C" {
                fn getdents(fd: libc::c_int, buf: *mut libc::c_char, nbytes: usize) -> isize;
            }

            use getdents as getdents_fn;
        }
        target_os = "netbsd" => {
            use super::dirent64 as dent_struct;

            unsafe extern "C" {
                fn getdents(fd: libc::c_int, buf: *mut libc::c_char, nbytes: usize) -> libc::c_int;
            }

            use getdents as getdents_fn;
        }
        target_os = "openbsd" => {
            use super::dirent64 as dent_struct;

            unsafe extern "C" {
                fn getdents(fd: libc::c_int, buf: *mut libc::c_void, nbytes: usize) -> libc::c_int;
            }

            use getdents as getdents_fn;
        }
        _ => {
            compile_error!("getdents is not supported on this platform!");
        }
    }

    // See https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_getdents.html for
    // semantics.
    #[inline]
    unsafe fn do_getdents(
        fd: fd::RawFd,
        base: *mut mem::MaybeUninit<u8>,
        nbytes: usize,
    ) -> io::Result<Option<num::NonZeroUsize>> {
        match unsafe { getdents_fn(fd, base.cast(), nbytes) } {
            // EINVAL occurs when the provided buffer size is not large enough for the next entry.
            -1 => Err(io::Error::last_os_error()),
            0 => Ok(None),
            rc => {
                debug_assert!(rc > 0);
                Ok(Some(unsafe { num::NonZeroUsize::new_unchecked(rc as usize) }))
            }
        }
    }

    #[inline]
    const fn getdents_eager<'buf>(
        fd: &mut fd::RawFd,
        buf: &'buf mut [mem::MaybeUninit<u8>],
    ) -> io::Result<Option<&'buf EagerEntries>> {
        let n = match unsafe { do_getdents(*fd, buf.as_mut_ptr(), buf.len()) } {
            Err(e) => return Err(e),
            Ok(None) => return Ok(None),
            Ok(Some(n)) => n,
        };
        let buf: &[u8] = unsafe { slice::from_raw_parts(buf.as_ptr().cast(), n.get()) };
        Ok(Some(EagerEntries::new(buf)))
    }

    #[inline]
    pub(super) fn getdents_single<'buf, 'fd>(
        buf: &'buf mut [mem::MaybeUninit<u8>],
        fd: &'fd mut fd::RawFd,
    ) -> ChunkedStreamResult<&'buf EagerEntries, io::Error> {
        match getdents_eager(fd, buf).transpose() {
            None => ChunkedStreamResult::NoFurtherEntries,
            Some(Err(e)) => ChunkedStreamResult::InterruptedByErr(e),
            Some(Ok(entries)) => ChunkedStreamResult::Entries(entries),
        }
    }

    #[derive(Debug)]
    #[repr(transparent)]
    pub(super) struct EagerEntries([u8]);

    impl EagerEntries {
        #[inline(always)]
        const fn new(bytes: &[u8]) -> &Self {
            unsafe { &*(bytes as *const [u8] as *const Self) }
        }

        #[inline(always)]
        const fn new_mut(bytes: &mut [u8]) -> &mut Self {
            unsafe { &mut *(bytes as *mut [u8] as *mut Self) }
        }

        #[inline]
        fn shift_index(&mut self, reclen: usize) {
            (&self.0)
                .split_off(..reclen)
                .expect("record length from getdents was more than remaining length");
        }

        pub(super) fn get_next<'buf, 's>(&'s mut self) -> Option<&'buf EagerDirent>
        where
            's: 'buf,
        {
            // The end of the final record will always be the end of the buffer.
            // This is a result of limiting buffer length to the result of the getdents() call.
            if self.0.is_empty() {
                return None;
            }

            // Index to the beginning of the current entry in the buffer.
            let cur_dirent_ptr: *const dent_struct = self.0.as_ptr().cast();
            // Read the length of the current record.
            let p_reclen = unsafe { ptr::addr_of!((*cur_dirent_ptr).d_reclen) };
            let d_reclen = unsafe { p_reclen.read() } as usize;
            // Create an unsized reference to the current record.
            let ret: &'buf [u8] = unsafe { slice::from_raw_parts(self.0.as_ptr(), d_reclen) };
            // Prepare for the next iteration by shifting the index to the start of the next record.
            self.shift_index(d_reclen);
            // Return an unsized wrapper around the record contained in the specified buffer region.
            Some(EagerDirent::new(ret))
        }

        #[inline(always)]
        pub(super) const fn as_ptr_range(&self) -> ops::Range<*const u8> {
            self.0.as_ptr_range()
        }

        #[unstable(feature = "slice_from_ptr_range", issue = "89792")]
        #[inline(always)]
        pub(super) const unsafe fn from_ptr_range<'a>(range: ops::Range<*const u8>) -> &'a Self {
            Self::new(unsafe { slice::from_ptr_range(range) })
        }

        #[unstable(feature = "slice_from_ptr_range", issue = "89792")]
        #[inline(always)]
        pub(super) const unsafe fn from_ptr_range_self_mut<'a>(
            range: ops::Range<*const u8>,
        ) -> &'a mut Self {
            let range: ops::Range<*mut u8> = mem::transmute(range);
            Self::new_mut(unsafe { slice::from_mut_ptr_range(range) })
        }
    }

    #[derive(Debug)]
    #[repr(transparent)]
    pub(super) struct EagerDirent([u8]);

    impl EagerDirent {
        #[inline(always)]
        const fn new(bytes: &[u8]) -> &Self {
            unsafe { &*(bytes as *const [u8] as *const Self) }
        }

        #[inline(always)]
        const fn new_mut(bytes: &mut [u8]) -> &mut Self {
            unsafe { &mut *(bytes as *mut [u8] as *mut Self) }
        }

        #[inline(always)]
        const fn as_dirent_ptr(&self) -> *const dent_struct {
            self.0.as_ptr().cast()
        }

        #[inline]
        const fn d_ino(&self) -> libc::ino64_t {
            let cur_dirent_ptr = self.as_dirent_ptr();
            let p_ino = unsafe { ptr::addr_of!((*cur_dirent_ptr).d_ino) };
            unsafe { p_ino.read() }
        }

        #[inline(always)]
        pub(super) const fn inode(&self) -> libc::ino64_t {
            self.d_ino()
        }

        #[inline]
        const fn d_type(&self) -> libc::c_uchar {
            let cur_dirent_ptr = self.as_dirent_ptr();
            let p_type = unsafe { ptr::addr_of!((*cur_dirent_ptr).d_type) };
            unsafe { p_type.read() }
        }

        #[inline(always)]
        pub(super) const fn eager_type(&self) -> super::EagerFileType {
            super::EagerFileType::from_type(self.d_type())
        }

        #[inline]
        #[cfg_attr(debug_assertions, track_caller)]
        const unsafe fn get_p_name(cur_dirent_ptr: *const dent_struct) -> *const libc::c_char {
            unsafe { ptr::addr_of!((*cur_dirent_ptr).d_name) }.cast()
        }

        #[inline]
        const fn p_name(&self) -> *const libc::c_char {
            let cur_dirent_ptr = self.as_dirent_ptr();
            unsafe { Self::get_p_name(cur_dirent_ptr) }
        }

        /// Ensure we have a non-empty string, which does *not* begin with a null.
        ///
        /// This is the behavior we expect from POSIX, but is *not* safety-critical.
        #[inline]
        #[track_caller]
        const fn is_valid_name_region(region: &[u8]) -> bool {
            // name field cannot be an empty slice
            !region.is_empty() &&
                // name field cannot be an empty null-terminated string
                (unsafe { region.as_ptr().read() } != b'\0')
        }

        const CUR_DIR_LEN: usize = 2;
        const CUR_DIR: [u8; Self::CUR_DIR_LEN] = [b'.', b'\0'];
        const PARENT_DIR_LEN: usize = 3;
        const PARENT_DIR: [u8; Self::PARENT_DIR_LEN] = [b'.', b'.', b'\0'];

        /// Whether this directory entry points to the `'.'` (current) or `'..'` (parent) directory.
        ///
        /// If so, [`fs::read_dir()`](crate::fs::read_dir) must avoid generating it as an entry.
        /// While this could be done at a higher level, after getting
        #[inline]
        pub(super) const fn is_generated_cur_or_parent(&self) -> bool {
            // Regardless of whether we know the precise entry name length on the current platform,
            // we can still ensure we never look at uninitialized memory, *without* calling strlen
            // (i.e. in constant time and space), by limiting to the length of the current record.
            let region = self.overbroad_name_region();
            // NB: *not* a safety check, since we also check the 0 and 1 cases below with runtime
            //     panics.
            debug_assert!(Self::is_valid_name_region(region));
            match region.len() {
                0 => unreachable!("region cannot be empty"),
                1 => unreachable!("region cannot be single byte"),
                // Determine if it matches the '.' entry.
                Self::CUR_DIR_LEN => {
                    debug_assert_eq!(Self::CUR_DIR_LEN, 2);
                    debug_assert_eq!(Self::CUR_DIR_LEN, Self::CUR_DIR
                                     .len());
                    let name_buf: &[u8; Self::CUR_DIR.len()] = {
                        let array_chunk: *const [u8; Self::CUR_DIR.len()] = region.as_ptr().cast();
                        unsafe { &*array_chunk }
                    };
                    matches!(name_buf, &Self::CUR_DIR)
                },
                // Determine if it matches the '..' entry.
                Self::PARENT_DIR_LEN => {
                    debug_assert_eq!(Self::PARENT_DIR_LEN, 3);
                    debug_assert_eq!(Self::PARENT_DIR_LEN, Self::PARENT_DIR.len());
                    let name_buf: &[u8; Self::PARENT_DIR.len()] = {
                        let array_chunk: *const [u8; Self::PARENT_DIR.len()] = region.as_ptr().cast();
                        unsafe { &*array_chunk }
                    };
                    matches!(name_buf, &Self::PARENT_DIR)
                },
                // TODO: add logging for any of these cases?
                n => {
                    debug_assert!(n > 3);
                    false
                },
            }
        }

        /// This is the value originally extracted from [`dent_struct::d_reclen`].
        #[inline(always)]
        const fn d_reclen(&self) -> usize { self.0.len() }

        // FIXME: some platforms provide the exact name length in the dirent struct (in particular
        //        hurd and most BSDs). If provided, both the generation of the .name() CStr as well
        //        as the .is_generated_cur_or_parent() method can avoid this "max" process entirely,
        //        and can instead generate .name() with CStr::from_bytes_with_nul_unchecked()!
        //
        //        It would be ideal to implement this with a platform-specific trait method,
        //        although that would also lose the ability to implement as a `const fn`, since
        //        trait methods cannot be const. However, since this is always interpreting the
        //        result of an OS syscall (the runtime state of the vfs abstraction), losing
        //        constness is no loss (and constness may in fact be wrong to use here at all).
        #[inline]
        const fn overbroad_name_region<'buf, 's>(&'s self) -> &'buf [u8]
        where
            's: 'buf,
        {
            let cur_dirent_ptr = self.as_dirent_ptr();

            // NB: The `name` field is always at the end of the directory entry, by design:
            // (a) this is true for `posix_getdents()` (not yet supported)
            // (b) this is true for all of the `getdents_fn()` impls supported above.
            //
            // As historical context, the initial linux "getdents" syscall (after 2.6.4) inserted
            // `d_type` at the end of the record. This behavior is not exposed by any of the libc
            // externs we call into above--getdents64
            //
            // `getdents_fn()` aligns records within the given buffer to `dent_struct`,
            // so there may be any amount of trailing garbage (it may or may not be nulls).
            // However, it *is* required that the `name` field have a null-termination byte.
            // As a result, we can limit the range of bytes that we have to scan for nulls (and
            // avoid the potential to ever read out of bounds (!!!)) by only scanning within the
            // allocated record field.
            let p_name = unsafe { Self::get_p_name(cur_dirent_ptr) };
            debug_assert!(unsafe { p_name.byte_offset_from(cur_dirent_ptr) } > 0);
            let name_offset = unsafe { p_name.byte_offset_from_unsigned(cur_dirent_ptr) };
            debug_assert!(name_offset <= self.d_reclen());
            let ret: usize = unsafe { self.d_reclen().unchecked_sub(name_offset) };

            // We also know:
            // (a) the `name` field will never be zero-sized,
            // (b) the `name` pointer will never be null.
            // This is because POSIX equates directory entry names with null-terminated strings, and
            // disallows empty (i.e. null) name strings.
            // As a result, we can also infer that the name itself
            // (and therefore the upper bound we calculate in this method)
            // will be strictly > 0.
            debug_assert!(name_offset < self.d_reclen());
            let max_namelen = unsafe { num::NonZeroUsize::new_unchecked(ret) };

            // Now we can calculate the region containing the name string:
            unsafe { slice::from_raw_parts(p_name.cast(), max_namelen.get()) }
        }

        // Narrow the name string to the exact region it covers by searching for the first null.
        #[inline]
        const fn name_buf<'buf, 's>(&'s self) -> &'buf [u8]
        where
            's: 'buf,
        {
            let region = self.overbroad_name_region();
            // NB: This is *not* a safety check, as this will be covered by the memchr call below.
            debug_assert!(Self::is_valid_name_region(region));
            let Some(nul_pos) = memchr::memchr(b'\0', region) else {
                let len = region.len();
                // NB: we do *not* relegate this to debug builds only--in case the OS gives us
                //     garbage, we must panic here.
                unreachable!("should always be a null byte in name (len {len}): {region:.10?}")
            };
            unsafe { slice::from_raw_parts(region.as_ptr(), nul_pos + 1) }
        }

        /// Calculate the runtime length of the null-terminated string located
        #[inline(always)]
        pub(super) const fn name<'buf, 's>(&'s self) -> &'buf ffi::CStr
        where
            's: 'buf,
        {
            let buf = self.name_buf();
            // We should be given a buffer with exactly one null byte at the end.
            debug_assert!(!buf.is_empty());
            debug_assert_eq!(buf[buf.len() - 1], b'\0');
            debug_assert!(memchr::memchr(b'\0', buf).is_none());
            unsafe { ffi::CStr::from_bytes_with_nul_unchecked(buf) }
        }

        #[inline(always)]
        pub(super) const fn as_ptr_range(&self) -> ops::Range<*const u8> {
            self.0.as_ptr_range()
        }

        #[unstable(feature = "slice_from_ptr_range", issue = "89792")]
        #[inline(always)]
        pub(super) const unsafe fn from_ptr_range<'a>(range: ops::Range<*const u8>) -> &'a Self {
            Self::new(unsafe { slice::from_ptr_range(range) })
        }

        #[unstable(feature = "slice_from_ptr_range", issue = "89792")]
        #[inline(always)]
        pub(super) const unsafe fn from_ptr_range_self_mut<'a>(
            range: ops::Range<*const u8>,
        ) -> &'a mut Self {
            let range: ops::Range<*mut u8> = mem::transmute(range);
            Self::new_mut(unsafe { slice::from_mut_ptr_range(range) })
        }
    }
}}

#[cfg(not(any(
    // No type:
    target_os = "solaris",
    target_os = "illumos",
    target_os = "aix",
    target_os = "nto",
    target_os = "haiku",
    target_os = "vxworks",
    // Neither ino nor type:
    target_os = "vita",
    target_os = "nuttx",
)))]
#[repr(u8)]
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum EagerFileType {
    Unknown = libc::DT_UNKNOWN,
    Fifo = libc::DT_FIFO,
    Symlink = libc::DT_LNK,
    File = libc::DT_REG,
    Socket = libc::DT_SOCK,
    Directory = libc::DT_DIR,
    Block = libc::DT_BLK,
    Character = libc::DT_CHR,
    Other(libc::c_uchar),
}

#[cfg(not(any(
    // No type:
    target_os = "solaris",
    target_os = "illumos",
    target_os = "aix",
    target_os = "nto",
    target_os = "haiku",
    target_os = "vxworks",
    // Neither ino nor type:
    target_os = "vita",
    target_os = "nuttx",
)))]
impl EagerFileType {
    #[inline(always)]
    const fn from_type(d_type: libc::c_uchar) -> Self {
        match d_type {
            libc::DT_FIFO => Self::Fifo,
            libc::DT_LNK => Self::Symlink,
            libc::DT_REG => Self::File,
            libc::DT_SOCK => Self::Socket,
            libc::DT_DIR => Self::Directory,
            libc::DT_BLK => Self::Block,
            libc::DT_CHR => Self::Character,
            libc::DT_UNKNOWN => Self::Unknown,
            _ => Self::Other(d_type),
        }
    }

    #[inline(always)]
    const fn as_file_type(self) -> Option<FileType> {
        match self {
            Self::Fifo => Some(FileType { mode: libc::S_IFIFO }),
            Self::Symlink => Some(FileType { mode: libc::S_IFLNK }),
            Self::File => Some(FileType { mode: libc::S_IFREG }),
            Self::Socket => Some(FileType { mode: libc::S_IFSOCK }),
            Self::Directory => Some(FileType { mode: libc::S_IFDIR }),
            Self::Block => Some(FileType { mode: libc::S_IFBLK }),
            Self::Character => Some(FileType { mode: libc::S_IFCHR }),
            Self::Unknown => None,
            _ => None,
        }
    }
}

#[unstable(feature = "mapped_lock_guards", issue = "117108")]
mod dir_fd {
    use crate::marker::{PhantomData, PhantomPinned};
    use crate::os::fd;
    use crate::os::unix::io::IntoRawFd;
    use crate::path::{Path, PathBuf};
    use crate::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard, MappedRwLockReadGuard, MappedRwLockWriteGuard};
    use crate::{io, mem, ops};

    // NB: the getdents syscall internally performs a mutating operation upon
    //     the resource represented by the `libc::DIR *` pointer.
    #[derive(Debug)]
    #[repr(transparent)]
    struct DirFd {
        fd: fd::RawFd,
    }

    impl DirFd {
        #[inline(always)]
        fn from_dir(dir: super::Dir) -> Self {
            let fd = unsafe { libc::dirfd(dir.0) };
            Self::from_fd(fd)
        }

        #[inline(always)]
        fn from_fd(fd: fd::RawFd) -> Self {
            super::debug_assert_fd_is_open(fd);
            Self { fd }
        }

        #[inline(always)]
        const fn as_fd(&self) -> &fd::RawFd {
            &self.fd
        }

        #[inline(always)]
        const fn as_mut_fd(&mut self) -> &mut fd::RawFd {
            &mut self.fd
        }
    }

    impl ops::Drop for DirFd {
        fn drop(&mut self) {
            super::debug_assert_fd_is_open(self.fd);
            let res = unsafe { libc::close(self.fd) };
            assert!(
                res == 0 || io::Error::last_os_error().is_interrupted(),
                "unexpected error during close() on directory fd: {:?}",
                io::Error::last_os_error()
            );
        }
    }

    #[derive(Debug)]
    pub(super) struct DirWithPath {
        dir: RwLock<DirFd>,
        path: PathBuf,
    }

    pub(super) trait FileDescriptorHandle {
        type FdRead<'h>: ops::Deref<Target = fd::RawFd> where Self: 'h;
        type FdWrite<'h>: ops::DerefMut<Target = fd::RawFd> where Self: 'h;

        fn as_fd<'h>(&'h self) -> Self::FdRead<'h>;
        fn as_mut_fd<'h>(&'h self) -> Self::FdWrite<'h>;
    }

    impl FileDescriptorHandle for DirWithPath {
        type FdRead<'h> = MappedRwLockReadGuard<'h, fd::RawFd> where Self: 'h;
        type FdWrite<'h> = MappedRwLockWriteGuard<'h, fd::RawFd> where Self: 'h;

        #[inline]
        fn as_fd<'h>(&'h self) -> Self::FdRead<'h> {
            RwLockReadGuard::map(self.dir.read().unwrap(), |dir| dir.as_fd())
        }
        #[inline]
        fn as_mut_fd<'h>(&'h self) -> Self::FdWrite<'h> {
            RwLockWriteGuard::map(self.dir.write().unwrap(), |dir| dir.as_mut_fd())
        }
    }

    #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
    impl const AsRef<Path> for DirWithPath {
        #[inline(always)]
        fn as_ref(&self) -> &Path {
            self.path.as_path()
        }
    }

    impl DirWithPath {
        pub(super) fn from_dir_and_path(dir: super::Dir, path: impl Into<PathBuf>) -> Self {
            let dir = RwLock::new(DirFd::from_dir(dir));
            let path = path.into();
            Self { dir, path }
        }

        pub(super) fn from_owned_dir_fd_and_path(
            dir_fd: fd::OwnedFd,
            path: impl Into<PathBuf>,
        ) -> Self {
            let dir = RwLock::new(DirFd::from_fd(dir_fd.into_raw_fd()));
            let path = path.into();
            Self { dir, path }
        }
    }
}

mod buffer_state {
    use super::getdents_impl;
    use core::marker::Destruct;
    use crate::convert::Infallible;
    use crate::os::fd;
    use crate::rc::Rc;
    use crate::sync::Arc;
    use crate::{io, mem, ops};

    pub(super) trait CloneMutRef: ops::Deref<Target = Self::R> + Clone {
        type R;
        fn make_unique_mut(&mut self) -> &mut Self::R;
        fn make_unique_self(self) -> Self::R;
        fn wrap(r: Self::R) -> Self;
    }

    #[repr(transparent)]
    #[derive(Debug)]
    pub(super) struct IterState<Buf>(Buf);

    impl<Buf> IterState<Buf> {
        #[inline(always)]
        pub(super) const fn with_buf(buf: Buf) -> Self {
            Self(buf)
        }

        #[inline(always)]
        pub(super) const fn into_inner(self) -> Buf {
            let Self(buf) = self;
            buf
        }
    }

    #[rustc_const_unstable(feature = "const_clone", issue = "142757")]
    /* #[rustc_const_unstable(feature = "const_destruct", issue = "133214")] */
    impl<Buf> const Clone for IterState<Buf>
    where
        Buf: [const] Clone + [const] Destruct,
    {
        #[inline(always)]
        fn clone(&self) -> Self {
            Self::with_buf(self.0.clone())
        }

        #[inline(always)]
        fn clone_from(&mut self, source: &Self) {
            self.0.clone_from(&source.0);
        }
    }

    #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
    impl<Buf> const ops::Deref for IterState<Buf>
    where
        Buf: [const] ops::Deref,
    {
        type Target = Buf::Target;

        #[inline(always)]
        fn deref(&self) -> &Self::Target {
            self.0.deref()
        }
    }

    #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
    impl<Buf, T> const AsRef<[T]> for IterState<Buf>
    where
        Buf: [const] AsRef<[T]>,
    {
        #[inline(always)]
        fn as_ref(&self) -> &[T] {
            self.0.as_ref()
        }
    }

    #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
    impl<Buf> const ops::DerefMut for IterState<Buf>
    where
        Buf: [const] ops::DerefMut,
    {
        #[inline(always)]
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.0.deref_mut()
        }
    }

    #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
    impl<Buf, T> const AsMut<[T]> for IterState<Buf>
    where
        Buf: [const] AsMut<[T]>,
    {
        #[inline(always)]
        fn as_mut(&mut self) -> &mut [T] {
            self.0.as_mut()
        }
    }

    impl<Data> CloneMutRef for Rc<Data>
    where
        Data: Clone,
    {
        type R = Data;

        #[inline]
        fn make_unique_mut(&mut self) -> &mut Self::R {
            Rc::make_mut(&mut self)
        }

        #[inline]
        fn make_unique_self(self) -> Self::R {
            Rc::unwrap_or_clone(self)
        }

        #[inline]
        fn wrap(r: Self::R) -> Self {
            Rc::new(r)
        }
    }

    impl<Data> CloneMutRef for Arc<Data>
    where
        Data: Clone,
    {
        type R = Data;

        #[inline]
        fn make_unique_mut(&mut self) -> &mut Self::R {
            Arc::make_mut(&mut self)
        }

        #[inline]
        fn make_unique_self(self) -> Self::R {
            Arc::unwrap_or_clone(self)
        }

        #[inline]
        fn wrap(r: Self::R) -> Self {
            Arc::new(r)
        }
    }

    #[derive(Debug)]
    pub(super) enum ChunkedStreamResult<T, E> {
        // This is expected to return a non-empty collection of values.
        Entries(T),
        // When all values have been exhausted, iteration ends without error.
        NoFurtherEntries,
        // If an error is produced, no new entries are readable, and the stream is generally
        // expected to be in an indeterminate state.
        InterruptedByErr(E),
    }

    #[unstable(feature = "try_trait_v2", issue = "84277")]
    #[rustc_const_unstable(feature = "const_try", issue = "74935")]
    impl<T, E> const ops::FromResidual<Result<Option<T>, E>> for ChunkedStreamResult<T, E> {
        #[inline]
        #[track_caller]
        fn from_residual(residual: Result<Option<T>, E>) -> Self {
            match residual {
                Ok(Some(t)) => Self::Entries(t),
                Ok(None) => Self::NoFurtherEntries,
                Err(e) => Self::InterruptedByErr(e),
            }
        }
    }

    #[unstable(feature = "try_trait_v2", issue = "84277")]
    #[rustc_const_unstable(feature = "const_try", issue = "74935")]
    impl<T> const ops::FromResidual<Option<T>> for ChunkedStreamResult<T, Infallible> {
        #[inline]
        #[track_caller]
        fn from_residual(residual: Option<T>) -> Self {
            match residual {
                Some(t) => Self::Entries(t),
                None => Self::NoFurtherEntries,
            }
        }
    }

    #[unstable(feature = "try_trait_v2", issue = "84277")]
    #[rustc_const_unstable(feature = "const_try", issue = "74935")]
    impl<T, E, F> const ops::FromResidual<ChunkedStreamResult<Infallible, E>>
        for ChunkedStreamResult<T, F>
        where F: [const] From<E>,
    {
        #[inline]
        #[track_caller]
        fn from_residual(residual: ChunkedStreamResult<Infallible, E>) -> Self {
            match residual {
                ChunkedStreamResult::NoFurtherEntries => Self::NoFurtherEntries,
                ChunkedStreamResult::InterruptedByErr(e) => Self::InterruptedByErr(From::from(e)),
            }
        }
    }

    #[unstable(feature = "try_trait_v2_residual", issue = "91285")]
    #[rustc_const_unstable(feature = "const_try_residual", issue = "91285")]
    impl<T, E> const ops::Residual<T> for ChunkedStreamResult<Infallible, E> {
        type TryType = ChunkedStreamResult<T, E>;
    }

    #[unstable(feature = "try_trait_v2", issue = "84277")]
    #[rustc_const_unstable(feature = "const_try", issue = "74935")]
    impl<T, E> const ops::Try for ChunkedStreamResult<T, E> {
        type Output = T;
        type Residual = ChunkedStreamResult<Infallible, E>;

        #[inline]
        fn from_output(output: Self::Output) -> Self {
            Self::Entries(output)
        }

        #[inline]
        fn branch(self) -> ops::ControlFlow<Self::Residual, Self::Output> {
            match self {
                Self::Entries(v) => ops::ControlFlow::Continue(v),
                Self::NoFurtherEntries => ops::ControlFlow::Break(ChunkedStreamResult::NoFurtherEntries),
                Self::InterruptedByErr(e) => ops::ControlFlow::Break(ChunkedStreamResult::InterruptedByErr(e)),
            }
        }
    }
}

cfg_has_getdents! {
#[unstable(feature = "assert_matches", issue = "82775")]
mod getdents_iter {
    use super::buffer_state::{ChunkedStreamResult, CloneMutRef, IterState};
    use super::getdents_impl::{self, EagerDirent, EagerEntries};
    use super::{dir_fd, run_path_with_cstr};
    use core::assert_matches::debug_assert_matches;
    use crate::borrow::Cow;
    use crate::marker::{Destruct, PhantomData, PhantomPinned};
    use crate::os::fd;
    use crate::path::{Path, PathBuf};
    use crate::pin::{self, Pin};
    use crate::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
    use crate::{cell, fmt, io, mem, ops, ptr};

    #[derive(Debug)]
    pub(super) struct Entry<DirMutRef, MutRef> {
        state: IterState<MutRef>,
        eager_dirent: ops::Range<*const u8>,
        pub(super) parent: DirMutRef,
    }

    impl<DirMutRef, MutRef> Entry<DirMutRef, MutRef> {
        #[inline(always)]
        pub(super) const fn as_dirent(&self) -> &EagerDirent {
            unsafe { EagerDirent::from_ptr_range(self.eager_dirent.clone()) }
        }

        #[inline(always)]
        pub(super) const fn as_dirent_mut(&mut self) -> &mut EagerDirent {
            unsafe { EagerDirent::from_ptr_range_self_mut(self.eager_dirent.clone()) }
        }
    }

    impl<DirMutRef, MutRef> Clone for Entry<DirMutRef, MutRef>
    where
        DirMutRef: Clone + Destruct,
        MutRef: Clone + Destruct,
    {
        #[inline]
        fn clone(&self) -> Self {
            Self {
                state: self.state.clone(),
                eager_dirent: self.eager_dirent.clone(),
                parent: self.parent.clone(),
            }
        }

        #[inline]
        fn clone_from(&mut self, source: &Self) {
            self.state.clone_from(&source.state);
            self.eager_dirent.clone_from(&source.eager_dirent);
            self.parent.clone_from(&source.parent);
        }
    }

    impl<DirMutRef, Buf, MutRef> Entry<DirMutRef, Pin<MutRef>>
    where
        MutRef: CloneMutRef<R = Buf>,
        Buf: AsMut<[mem::MaybeUninit<u8>]>,
    {
        #[unstable(feature = "try_trait_v2", issue = "84277")]
        fn next_entry<'buf, 's>(
            entries: &'s mut IterEntries<Pin<MutRef>>,
            parent: DirMutRef,
        ) -> impl ops::Try<Output = Self>
        where
            Buf: 'buf,
            's: 'buf,
        {
            let eager_dirent: ops::Range<*const u8> =
                entries.as_entries_mut().get_next()?.as_ptr_range();
            let state = entries.state.clone();
            Some(Self { state, eager_dirent, parent })
        }
    }

    #[derive(Debug)]
    struct IterEntries<MutRef> {
        state: IterState<MutRef>,
        entries: ops::Range<*const u8>,
    }

    impl<MutRef> IterEntries<Pin<MutRef>> {
        #[inline(always)]
        const fn as_entries(&self) -> &EagerEntries {
            unsafe { EagerEntries::from_ptr_range(self.entries.clone()) }
        }

        #[inline(always)]
        const fn as_entries_mut(&mut self) -> &mut EagerEntries {
            unsafe { EagerEntries::from_ptr_range_self_mut(self.entries.clone()) }
        }

        #[inline(always)]
        const fn retrieve_state(self) -> MutRef where MutRef: ops::Deref {
            let inner = self.state.into_inner();
            unsafe { Pin::into_inner_unchecked(inner) }
        }
    }

    #[unstable(feature = "unsafe_pinned", issue = "125735")]
    impl<Buf, MutRef> IterEntries<Pin<MutRef>>
    where
        MutRef: CloneMutRef<R = Buf>,
        Buf: AsMut<[mem::MaybeUninit<u8>]> + Clone,
    {
        #[unstable(feature = "try_trait_v2", issue = "84277")]
        fn next_getdents_entries<'fd>(
            buf: MutRef,
            fd: &'fd mut fd::RawFd,
        ) -> ChunkedStreamResult<Self, io::Error> {
            let mut buf = pin::UnsafePinned::new(buf);
            let mut buf_pin = unsafe { Pin::new_unchecked(&mut buf) };
            let entries: ops::Range<*const u8> = unsafe {
                // Ensure we have unique ownership of the now-pinned allocation.
                let buf = unsafe { &mut *(buf_pin.as_mut().get_mut_pinned()) }.make_unique_mut();
                // Now we can point to it freely.
                getdents_impl::getdents_single(buf.as_mut(), fd)?.as_ptr_range()
            };
            let buf = unsafe { Pin::new_unchecked(buf.into_inner()) };
            let state = IterState::with_buf(buf);
            ChunkedStreamResult::Entries(Self { state, entries })
        }
    }

    enum IterStateMachine<MutRef> {
        Ready(mem::ManuallyDrop<MutRef>),
        InProgress(mem::ManuallyDrop<IterEntries<Pin<MutRef>>>),
        Done,
    }

    impl<MutRef> fmt::Debug for IterStateMachine<MutRef> {
        #[inline]
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Ready(_) => write!(f, "Ready(...)"),
                Self::InProgress(_) => write!(f, "InProgress(...)"),
                Self::Done => write!(f, "Done"),
            }
        }
    }

    pub(super) struct Iter<DirMutRef, MutRef> {
        // NB: As with the readdir() implementation, we need a reference to the initial path string
        //     provided to readdir() without introducing a lifetime parameter.
        //     However, as we also now respect the mutability and distinct lifetimes of Dir
        //     instances, the Arc acknowledges that the iterator exists separately from the
        //     directory itself.
        dir: DirMutRef,
        // NB: In order to support DirEntry instances living past the lifetime of the getdents chunk
        //     they reference, we call Arc::make_mut() for each new getdents call. This lazily
        //     clones the buffer so that if all DirEntry instances are dropped, no further
        //     allocation occurs, but if all DirEntry instances are retained and *not* dropped, we
        //     perform far fewer allocations by sharing the backing mallocation across
        //     DirEntry instances.
        state_machine: IterStateMachine<MutRef>,
    }

    impl<DirMutRef, MutRef> Iter<DirMutRef, MutRef>
    {
        pub(super) const fn from_dir_with_path_and_buf(
            dir: DirMutRef,
            buf: MutRef,
        ) -> Self {
            Self { dir, state_machine: IterStateMachine::Ready(mem::ManuallyDrop::new(buf)) }
        }
    }

    #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
    impl<Dir, DirMutRef, MutRef> const AsRef<Path> for Iter<DirMutRef, MutRef>
    where Dir: [const] AsRef<Path>, DirMutRef: [const] ops::Deref<Target=Dir> {
        #[inline(always)]
        fn as_ref(&self) -> &Path { self.dir.as_ref() }
    }

    impl<Dir, DirMutRef, MutRef> fmt::Debug for Iter<DirMutRef, MutRef>
    where Dir: AsRef<Path>, DirMutRef: ops::Deref<Target=Dir> {
        #[inline]
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let p: &Path = self.as_ref();
            fmt::Debug::fmt(p, f)
        }
    }

    impl<Dir, DirMutRef, MutRef> dir_fd::FileDescriptorHandle for Iter<DirMutRef, MutRef>
    where for<'h> Dir: dir_fd::FileDescriptorHandle + 'h,
          DirMutRef: ops::Deref<Target=Dir> {
        type FdRead<'h> = <Dir as dir_fd::FileDescriptorHandle>::FdRead<'h> where Self: 'h;
        type FdWrite<'h> = <Dir as dir_fd::FileDescriptorHandle>::FdWrite<'h> where Self: 'h;

        #[inline(always)]
        fn as_fd<'h>(&'h self) -> Self::FdRead<'h> {
            self.dir.as_fd()
        }
        #[inline(always)]
        fn as_mut_fd<'h>(&'h self) -> Self::FdWrite<'h> {
            self.dir.as_mut_fd()
        }
    }

    impl<Dir, DirMutRef, Buf, MutRef> Iterator for Iter<DirMutRef, MutRef>
    where
        for<'h> Dir: dir_fd::FileDescriptorHandle + 'h,
        DirMutRef: ops::Deref<Target = Dir> + Clone,
        Buf: AsMut<[mem::MaybeUninit<u8>]> + Clone,
        MutRef: CloneMutRef<R = Buf>,
    {
        type Item = io::Result<Entry<DirMutRef, Pin<MutRef>>>;

        /* #[unstable(feature = "try_trait_v2", issue = "84277")] */
        fn next(&mut self) -> Option<Self::Item> {
            use core::ops::Try;

            let res = 'done_or_err: loop {
                match &mut self.state_machine {
                    IterStateMachine::Done => break 'done_or_err None,
                    IterStateMachine::Ready(buf) => match IterEntries::next_getdents_entries(
                        unsafe { mem::ManuallyDrop::take(buf) },
                        &mut self.dir.as_mut_fd(),
                    )
                    .branch()
                    {
                        ops::ControlFlow::Break(e) => match e {
                            // FIXME: add a log message for no entries?
                            ChunkedStreamResult::NoFurtherEntries => break 'done_or_err None,
                            ChunkedStreamResult::InterruptedByErr(e) => break 'done_or_err Some(e),
                        },
                        ops::ControlFlow::Continue(entries) => {
                            self.state_machine = IterStateMachine::InProgress(
                                mem::ManuallyDrop::new(entries),
                            );
                        }
                    }, // InProgress => fall through!
                }
                // If we're here, we are "in progress"!
                debug_assert_matches!(&self.state_machine, IterStateMachine::InProgress(_));

                // This too is a loop so that it can filter out "." and ".." entries.
                'entry_result: loop {
                    match &mut self.state_machine {
                        IterStateMachine::InProgress(entries) => {
                            // We need to do unsafe things to clone all the handles needed for an
                            // entry, because we share mutable state in the getdents buffer across
                            // multiple objects in complex ways.
                            let entries = cell::UnsafeCell::from_mut(entries);
                            let p_entries: *const IterEntries<Pin<MutRef>> = entries.get().cast();
                            // We got one!!!!!
                            if let Some(entry) = entries.get_mut().as_entries_mut().get_next()
                            {
                                // Ensure . and .. are filtered out:
                                if entry.is_generated_cur_or_parent() {
                                    continue 'entry_result;
                                }
                                // Otherwise, clone all the handles to satisfy fs::DirEntry's
                                // requirements!
                                let eager_dirent = entry.as_ptr_range();
                                let parent = self.dir.clone();
                                let state = unsafe { &*p_entries }.state.clone();

                                return Some(Ok(Entry { state, eager_dirent, parent }));
                            } else {
                                // Out of entries!!! Time to reload!!!
                                let buf = unsafe { mem::ManuallyDrop::take(entries.get_mut()) }
                                    .retrieve_state();
                                self.state_machine =
                                    IterStateMachine::Ready(mem::ManuallyDrop::new(buf));
                                continue 'done_or_err;
                            }
                        }
                    }
                }

                unreachable!(
                    "should never get here! in-progress state should return early or continue!"
                )
            };

            match res {
                // We have an i/o error! Maybe there's still more entries!
                Some(e) => Some(Err(e)),
                None => {
                    // We have no more in the current buffer, and no more buffers to receive.
                    self.state_machine = IterStateMachine::Done;
                    None
                }
            }
        }
    }

    impl<Dir, DirMutRef, Buf, MutRef> crate::iter::FusedIterator for Iter<DirMutRef, MutRef>
    where
        for<'h> Dir: dir_fd::FileDescriptorHandle + 'h,
        DirMutRef: ops::Deref<Target = Dir> + Clone,
        Buf: AsMut<[mem::MaybeUninit<u8>]> + Clone,
        MutRef: CloneMutRef<R = Buf>,
    {
    }
}}

cfg_select_has_getdents! {
    => {
        #[repr(transparent)]
        struct ReadDirWithBuf<DirMutRef, MutRef> {
            iter: getdents_iter::Iter<DirMutRef, MutRef>,
        }

        impl<DirMutRef, MutRef> ReadDirWithBuf<DirMutRef, MutRef> {
            #[inline(always)]
            const fn with_buf(dir: DirMutRef, buf: MutRef) -> Self {
                let iter = getdents_iter::Iter::from_dir_with_path_and_buf(dir, buf);
                Self { iter }
            }
        }

        impl<Dir, DirMutRef, MutRef> fmt::Debug for ReadDirWithBuf<DirMutRef, MutRef>
        where Dir: AsRef<Path>, DirMutRef: core::ops::Deref<Target=Dir> {
            #[inline(always)]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // This will only be called from std::fs::ReadDir, which will add a "ReadDir()" frame.
                // Thus the result will be e g 'ReadDir("/home")'
                fmt::Debug::fmt(&self.iter, f)
            }
        }

        #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
        impl<DirMutRef, MutRef> const AsRef<getdents_iter::Iter<DirMutRef, MutRef>>
            for ReadDirWithBuf<DirMutRef, MutRef>
        {
            #[inline(always)]
            fn as_ref(&self) -> &getdents_iter::Iter<DirMutRef, MutRef> { &self.iter }
        }

        #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
        impl<DirMutRef, MutRef> const AsMut<getdents_iter::Iter<DirMutRef, MutRef>>
            for ReadDirWithBuf<DirMutRef, MutRef>
        {
            #[inline(always)]
            fn as_mut(&mut self) -> &mut getdents_iter::Iter<DirMutRef, MutRef> { &mut self.iter }
        }

        /// Chosen arbitrarily.
        ///
        /// Rust users with performance- or allocation-sensitive use cases would require expanding
        /// the stdlib API, so it's not worth spending too much time optimizing this default
        /// right now.
        pub(crate) const GETDENTS_BUF_DEFAULT_SIZE: usize = 8192;
        /// This can be allocated arbitrarily, but the size is known at compile time.
        ///
        /// This is a good default for a generic stdlib impl, but users who care about performance
        /// *will* need a way to provide their own buffer.
        pub(crate) type ReadDirBuffer = [mem::MaybeUninit<u8>; GETDENTS_BUF_DEFAULT_SIZE];
        /// How to allocate memory regions for the OS to write into with `getdents()`.
        ///
        /// *NB: This must implement [`buffer_state::CloneMutRef`].*
        ///
        /// Users interested in performance *will* require the ability to employ e.g. memory pooling
        /// and other techniques.
        pub(crate) type ReadDirRef<T> = Arc<T>;
        /// How to allocate memory regions for path strings.
        ///
        /// *NB: This must implement [`buffer_state::CloneMutRef`].*
        ///
        /// Users interested in performance *will* require the ability to employ e.g. memory pooling
        /// and other techniques.
        pub(crate) type DirPathRef<T> = Arc<T>;
        /// Object containing the originally-provided path string and directory file descriptor.
        ///
        /// *NB: This must implement [`dir_fd::FileDescriptorHandle`] and `AsRef<Path>`.*
        pub(crate) type ReadDirDirPath = dir_fd::DirWithPath;

        #[inline(always)]
        pub(crate) fn get_read_dir_buffer_handle() -> ReadDirRef<ReadDirBuffer> {
            let buf = [mem::MaybeUninit::uninit(); GETDENTS_BUF_DEFAULT_SIZE];
            Arc::new(buf)
        }

        #[inline(always)]
        pub(crate) fn create_dir_path_handle(p: ReadDirDirPath) -> DirPathRef<ReadDirDirPath> {
            Arc::new(p)
        }

        #[repr(transparent)]
        pub struct ReadDir(ReadDirWithBuf<DirPathRef<dir_fd::DirWithPath>,
                                          ReadDirRef<ReadDirBuffer>>);

        impl fmt::Debug for ReadDir {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&self.0, f)
            }
        }

        #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
        impl const core::ops::Deref for ReadDir {
            type Target = ReadDirWithBuf<DirPathRef<dir_fd::DirWithPath>,
                                          ReadDirRef<ReadDirBuffer>>;

            #[inline(always)]
            fn deref(&self) -> &Self::Target { &self.0 }
        }

        #[rustc_const_unstable(feature = "const_convert", issue = "143773")]
        impl const core::ops::DerefMut for ReadDir {
            #[inline(always)]
            fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
        }
    }
    _ => {
        // all DirEntry's will have a reference to this struct
        struct InnerReadDir {
            dirp: Dir,
            root: PathBuf,
        }

        pub struct ReadDir {
            inner: Arc<InnerReadDir>,
            end_of_stream: bool,
        }

        impl fmt::Debug for ReadDir {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // This will only be called from std::fs::ReadDir, which will add a "ReadDir()" frame.
                // Thus the result will be e g 'ReadDir("/home")'
                fmt::Debug::fmt(&*self.inner.root, f)
            }
        }
    }
}

cfg_select_has_getdents! {
    => {
        impl ReadDir {
            const fn new(inner: ReadDirWithBuf<DirPathRef<dir_fd::DirWithPath>,
                                               ReadDirRef<ReadDirBuffer>>) -> Self {
                Self(inner)
            }
        }
    }
    _ => {
        impl ReadDir {
            fn new(inner: InnerReadDir) -> Self {
                Self { inner: Arc::new(inner), end_of_stream: false }
            }
        }
    }
}

struct Dir(*mut libc::DIR);

unsafe impl Send for Dir {}
unsafe impl Sync for Dir {}

cfg_select_has_getdents! {
    => {
        #[repr(transparent)]
        pub struct DirEntry {
            inner: getdents_iter::Entry<DirPathRef<dir_fd::DirWithPath>, Pin<ReadDirRef<ReadDirBuffer>>>,
        }
    }
    any(
        target_os = "aix",
        target_os = "android",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "hurd",
        target_os = "illumos",
        target_os = "linux",
        target_os = "nto",
        target_os = "redox",
        target_os = "solaris",
        target_os = "vita",
        target_os = "wasi",
    ) => {
        pub struct DirEntry {
            dir: Arc<InnerReadDir>,
            entry: dirent64_min,
            // We need to store an owned copy of the entry name on platforms that use
            // readdir() (not readdir_r()), because a) struct dirent may use a flexible
            // array to store the name, b) it lives only until the next readdir() call.
            name: crate::ffi::CString,
        }

        // Define a minimal subset of fields we need from `dirent64`, especially since
        // we're not using the immediate `d_name` on these targets. Keeping this as an
        // `entry` field in `DirEntry` helps reduce the `cfg` boilerplate elsewhere.
        struct dirent64_min {
            d_ino: u64,
            #[cfg(not(any(
                target_os = "solaris",
                target_os = "illumos",
                target_os = "aix",
                target_os = "nto",
                target_os = "vita",
            )))]
            d_type: u8,
        }
    }
    _ => {
        pub struct DirEntry {
            dir: Arc<InnerReadDir>,
            // The full entry includes a fixed-length `d_name`.
            entry: dirent64,
        }
    }
}

#[derive(Clone)]
pub struct OpenOptions {
    // generic
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    // system-specific
    custom_flags: i32,
    mode: mode_t,
}

#[derive(Clone, PartialEq, Eq)]
pub struct FilePermissions {
    mode: mode_t,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
    #[cfg(target_vendor = "apple")]
    created: Option<SystemTime>,
}

#[derive(Copy, Clone, Eq)]
pub struct FileType {
    mode: mode_t,
}

impl PartialEq for FileType {
    fn eq(&self, other: &Self) -> bool {
        self.masked() == other.masked()
    }
}

impl core::hash::Hash for FileType {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.masked().hash(state);
    }
}

pub struct DirBuilder {
    mode: mode_t,
}

#[derive(Copy, Clone)]
struct Mode(mode_t);

cfg_has_statx! {{
    impl FileAttr {
        fn from_stat64(stat: stat64) -> Self {
            Self { stat, statx_extra_fields: None }
        }

        #[cfg(target_pointer_width = "32")]
        pub fn stx_mtime(&self) -> Option<&libc::statx_timestamp> {
            if let Some(ext) = &self.statx_extra_fields {
                if (ext.stx_mask & libc::STATX_MTIME) != 0 {
                    return Some(&ext.stx_mtime);
                }
            }
            None
        }

        #[cfg(target_pointer_width = "32")]
        pub fn stx_atime(&self) -> Option<&libc::statx_timestamp> {
            if let Some(ext) = &self.statx_extra_fields {
                if (ext.stx_mask & libc::STATX_ATIME) != 0 {
                    return Some(&ext.stx_atime);
                }
            }
            None
        }

        #[cfg(target_pointer_width = "32")]
        pub fn stx_ctime(&self) -> Option<&libc::statx_timestamp> {
            if let Some(ext) = &self.statx_extra_fields {
                if (ext.stx_mask & libc::STATX_CTIME) != 0 {
                    return Some(&ext.stx_ctime);
                }
            }
            None
        }
    }
} else {
    impl FileAttr {
        fn from_stat64(stat: stat64) -> Self {
            Self { stat }
        }
    }
}}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.stat.st_size as u64
    }
    pub fn perm(&self) -> FilePermissions {
        FilePermissions { mode: (self.stat.st_mode as mode_t) }
    }

    pub fn file_type(&self) -> FileType {
        FileType { mode: self.stat.st_mode as mode_t }
    }
}

#[cfg(target_os = "netbsd")]
impl FileAttr {
    pub fn modified(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_mtime as i64, self.stat.st_mtimensec as i64)
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_atime as i64, self.stat.st_atimensec as i64)
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_birthtime as i64, self.stat.st_birthtimensec as i64)
    }
}

#[cfg(target_os = "aix")]
impl FileAttr {
    pub fn modified(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_mtime.tv_sec as i64, self.stat.st_mtime.tv_nsec as i64)
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_atime.tv_sec as i64, self.stat.st_atime.tv_nsec as i64)
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_ctime.tv_sec as i64, self.stat.st_ctime.tv_nsec as i64)
    }
}

#[cfg(not(any(target_os = "netbsd", target_os = "nto", target_os = "aix", target_os = "wasi")))]
impl FileAttr {
    #[cfg(not(any(
        target_os = "vxworks",
        target_os = "espidf",
        target_os = "horizon",
        target_os = "vita",
        target_os = "hurd",
        target_os = "rtems",
        target_os = "nuttx",
    )))]
    pub fn modified(&self) -> io::Result<SystemTime> {
        #[cfg(target_pointer_width = "32")]
        cfg_has_statx! {
            if let Some(mtime) = self.stx_mtime() {
                return SystemTime::new(mtime.tv_sec, mtime.tv_nsec as i64);
            }
        }

        SystemTime::new(self.stat.st_mtime as i64, self.stat.st_mtime_nsec as i64)
    }

    #[cfg(any(
        target_os = "vxworks",
        target_os = "espidf",
        target_os = "vita",
        target_os = "rtems",
    ))]
    pub fn modified(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_mtime as i64, 0)
    }

    #[cfg(any(target_os = "horizon", target_os = "hurd", target_os = "nuttx"))]
    pub fn modified(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_mtim.tv_sec as i64, self.stat.st_mtim.tv_nsec as i64)
    }

    #[cfg(not(any(
        target_os = "vxworks",
        target_os = "espidf",
        target_os = "horizon",
        target_os = "vita",
        target_os = "hurd",
        target_os = "rtems",
        target_os = "nuttx",
    )))]
    pub fn accessed(&self) -> io::Result<SystemTime> {
        #[cfg(target_pointer_width = "32")]
        cfg_has_statx! {
            if let Some(atime) = self.stx_atime() {
                return SystemTime::new(atime.tv_sec, atime.tv_nsec as i64);
            }
        }

        SystemTime::new(self.stat.st_atime as i64, self.stat.st_atime_nsec as i64)
    }

    #[cfg(any(
        target_os = "vxworks",
        target_os = "espidf",
        target_os = "vita",
        target_os = "rtems"
    ))]
    pub fn accessed(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_atime as i64, 0)
    }

    #[cfg(any(target_os = "horizon", target_os = "hurd", target_os = "nuttx"))]
    pub fn accessed(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_atim.tv_sec as i64, self.stat.st_atim.tv_nsec as i64)
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_vendor = "apple",
        target_os = "cygwin",
    ))]
    pub fn created(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_birthtime as i64, self.stat.st_birthtime_nsec as i64)
    }

    #[cfg(not(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "vita",
        target_vendor = "apple",
        target_os = "cygwin",
    )))]
    pub fn created(&self) -> io::Result<SystemTime> {
        cfg_has_statx! {
            if let Some(ext) = &self.statx_extra_fields {
                return if (ext.stx_mask & libc::STATX_BTIME) != 0 {
                    SystemTime::new(ext.stx_btime.tv_sec, ext.stx_btime.tv_nsec as i64)
                } else {
                    Err(io::const_error!(
                        io::ErrorKind::Unsupported,
                        "creation time is not available for the filesystem",
                    ))
                };
            }
        }

        Err(io::const_error!(
            io::ErrorKind::Unsupported,
            "creation time is not available on this platform currently",
        ))
    }

    #[cfg(target_os = "vita")]
    pub fn created(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_ctime as i64, 0)
    }
}

#[cfg(any(target_os = "nto", target_os = "wasi"))]
impl FileAttr {
    pub fn modified(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_mtim.tv_sec, self.stat.st_mtim.tv_nsec.into())
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_atim.tv_sec, self.stat.st_atim.tv_nsec.into())
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        SystemTime::new(self.stat.st_ctim.tv_sec, self.stat.st_ctim.tv_nsec.into())
    }
}

impl AsInner<stat64> for FileAttr {
    #[inline]
    fn as_inner(&self) -> &stat64 {
        &self.stat
    }
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        // check if any class (owner, group, others) has write permission
        self.mode & 0o222 == 0
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            // remove write permission for all classes; equivalent to `chmod a-w <file>`
            self.mode &= !0o222;
        } else {
            // add write permission for all classes; equivalent to `chmod a+w <file>`
            self.mode |= 0o222;
        }
    }
    #[cfg(not(target_os = "wasi"))]
    pub fn mode(&self) -> u32 {
        self.mode as u32
    }
}

impl FileTimes {
    pub fn set_accessed(&mut self, t: SystemTime) {
        self.accessed = Some(t);
    }

    pub fn set_modified(&mut self, t: SystemTime) {
        self.modified = Some(t);
    }

    #[cfg(target_vendor = "apple")]
    pub fn set_created(&mut self, t: SystemTime) {
        self.created = Some(t);
    }
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.is(libc::S_IFDIR)
    }
    pub fn is_file(&self) -> bool {
        self.is(libc::S_IFREG)
    }
    pub fn is_symlink(&self) -> bool {
        self.is(libc::S_IFLNK)
    }

    pub fn is(&self, mode: mode_t) -> bool {
        self.masked() == mode
    }

    fn masked(&self) -> mode_t {
        self.mode & libc::S_IFMT
    }
}

impl fmt::Debug for FileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let FileType { mode } = self;
        f.debug_struct("FileType").field("mode", &Mode(*mode)).finish()
    }
}

impl FromInner<u32> for FilePermissions {
    fn from_inner(mode: u32) -> FilePermissions {
        FilePermissions { mode: mode as mode_t }
    }
}

impl fmt::Debug for FilePermissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let FilePermissions { mode } = self;
        f.debug_struct("FilePermissions").field("mode", &Mode(*mode)).finish()
    }
}

cfg_select_has_getdents! {
    => {
        impl Iterator for ReadDir {
            type Item = io::Result<DirEntry>;

            fn next(&mut self) -> Option<Self::Item> {
                let iter: &mut getdents_iter::Iter<_, _> = self.as_mut();
                match iter.next() {
                    None => None,
                    Some(Ok(entry)) => Some(Ok(DirEntry { inner: entry })),
                    Some(Err(e)) => Some(Err(e)),
                }
            }
        }

        impl crate::iter::FusedIterator for ReadDir {}
    }
    any(
        target_os = "aix",
        target_os = "android",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "hurd",
        target_os = "illumos",
        target_os = "linux",
        target_os = "nto",
        target_os = "redox",
        target_os = "solaris",
        target_os = "vita",
        target_os = "wasi",
    ) => {
        impl Iterator for ReadDir {
            type Item = io::Result<DirEntry>;

            fn next(&mut self) -> Option<Self::Item> {
                use crate::sys::os::{errno, set_errno};

                if self.end_of_stream {
                    return None;
                }

                unsafe {
                    loop {
                        // As of POSIX.1-2017, readdir() is not required to be thread safe; only
                        // readdir_r() is. However, readdir_r() cannot correctly handle platforms
                        // with unlimited or variable NAME_MAX. Many modern platforms guarantee
                        // thread safety for readdir() as long an individual DIR* is not accessed
                        // concurrently, which is sufficient for Rust.
                        set_errno(0);
                        let entry_ptr: *const dirent64 = readdir64(self.inner.dirp.0);
                        if entry_ptr.is_null() {
                            // We either encountered an error, or reached the end. Either way,
                            // the next call to next() should return None.
                            self.end_of_stream = true;

                            // To distinguish between errors and end-of-directory, we had to clear
                            // errno beforehand to check for an error now.
                            return match errno() {
                                0 => None,
                                e => Some(Err(Error::from_raw_os_error(e))),
                            };
                        }

                        // The dirent64 struct is a weird imaginary thing that isn't ever supposed
                        // to be worked with by value. Its trailing d_name field is declared
                        // variously as [c_char; 256] or [c_char; 1] on different systems but
                        // either way that size is meaningless; only the offset of d_name is
                        // meaningful. The dirent64 pointers that libc returns from readdir64 are
                        // allowed to point to allocations smaller _or_ LARGER than implied by the
                        // definition of the struct.
                        //
                        // As such, we need to be even more careful with dirent64 than if its
                        // contents were "simply" partially initialized data.
                        //
                        // Like for uninitialized contents, converting entry_ptr to `&dirent64`
                        // would not be legal. However, we can use `&raw const (*entry_ptr).d_name`
                        // to refer the fields individually, because that operation is equivalent
                        // to `byte_offset` and thus does not require the full extent of `*entry_ptr`
                        // to be in bounds of the same allocation, only the offset of the field
                        // being referenced.

                        // d_name is guaranteed to be null-terminated.
                        let name = CStr::from_ptr((&raw const (*entry_ptr).d_name).cast());
                        let name_bytes = name.to_bytes();
                        if name_bytes == b"." || name_bytes == b".." {
                            continue;
                        }

                        // When loading from a field, we can skip the `&raw const`; `(*entry_ptr).d_ino` as
                        // a value expression will do the right thing: `byte_offset` to the field and then
                        // only access those bytes.
                        #[cfg(not(target_os = "vita"))]
                        let entry = dirent64_min {
                            #[cfg(target_os = "freebsd")]
                            d_ino: (*entry_ptr).d_fileno,
                            #[cfg(not(target_os = "freebsd"))]
                            d_ino: (*entry_ptr).d_ino as u64,
                            #[cfg(not(any(
                                target_os = "solaris",
                                target_os = "illumos",
                                target_os = "aix",
                                target_os = "nto",
                            )))]
                            d_type: (*entry_ptr).d_type as u8,
                        };

                        #[cfg(target_os = "vita")]
                        let entry = dirent64_min { d_ino: 0u64 };

                        return Some(Ok(DirEntry {
                            entry,
                            name: name.to_owned(),
                            dir: Arc::clone(&self.inner),
                        }));
                    }
                }
            }
        }
    }
    _ => {
        impl Iterator for ReadDir {
            type Item = io::Result<DirEntry>;

            fn next(&mut self) -> Option<Self::Item> {
                if self.end_of_stream {
                    return None;
                }

                unsafe {
                    let mut ret = DirEntry { entry: mem::zeroed(), dir: Arc::clone(&self.inner) };
                    let mut entry_ptr = ptr::null_mut();
                    loop {
                        let err = readdir64_r(self.inner.dirp.0, &mut ret.entry, &mut entry_ptr);
                        if err != 0 {
                            if entry_ptr.is_null() {
                                // We encountered an error (which will be returned in this iteration), but
                                // we also reached the end of the directory stream. The `end_of_stream`
                                // flag is enabled to make sure that we return `None` in the next iteration
                                // (instead of looping forever)
                                self.end_of_stream = true;
                            }
                            return Some(Err(Error::from_raw_os_error(err)));
                        }
                        if entry_ptr.is_null() {
                            return None;
                        }
                        if ret.name_bytes() != b"." && ret.name_bytes() != b".." {
                            return Some(Ok(ret));
                        }
                    }
                }
            }
        }
    }
}

/// Aborts the process if a file desceriptor is not open, if debug asserts are enabled
///
/// Many IO syscalls can't be fully trusted about EBADF error codes because those
/// might get bubbled up from a remote FUSE server rather than the file descriptor
/// in the current process being invalid.
///
/// So we check file flags instead which live on the file descriptor and not the underlying file.
/// The downside is that it costs an extra syscall, so we only do it for debug.
#[inline]
pub(crate) fn debug_assert_fd_is_open(fd: RawFd) {
    use crate::sys::os::errno;

    // this is similar to assert_unsafe_precondition!() but it doesn't require const
    if core::ub_checks::check_library_ub() {
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 && errno() == libc::EBADF {
            rtabort!("IO Safety violation: owned file descriptor already closed");
        }
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        // dirfd isn't supported everywhere
        #[cfg(not(any(
            miri,
            target_os = "redox",
            target_os = "nto",
            target_os = "vita",
            target_os = "hurd",
            target_os = "espidf",
            target_os = "horizon",
            target_os = "vxworks",
            target_os = "rtems",
            target_os = "nuttx",
        )))]
        {
            let fd = unsafe { libc::dirfd(self.0) };
            debug_assert_fd_is_open(fd);
        }
        let r = unsafe { libc::closedir(self.0) };
        assert!(
            r == 0 || crate::io::Error::last_os_error().is_interrupted(),
            "unexpected error during closedir: {:?}",
            crate::io::Error::last_os_error()
        );
    }
}

cfg_select_has_getdents! {
    => {
        impl DirEntry {
            pub fn path(&self) -> PathBuf {
                let p: &ReadDirDirPath = self.inner.parent.as_ref();
                let p: &Path = p.as_ref();
                p.join(self.file_name_os_str())
            }
            #[inline]
            pub fn file_name(&self) -> OsString {
                self.file_name_os_str().to_os_string()
            }
            #[inline(always)]
            fn name_cstr(&self) -> &CStr {
                /* FIXME: use the region-bounded method we developed earlier for this! */
                unsafe { mem::transmute(self.inner.as_dirent().name()) }
            }
            #[inline(always)]
            fn name_bytes(&self) -> &[u8] {
                self.name_cstr().to_bytes()
            }
            #[inline]
            pub fn file_name_os_str(&self) -> &OsStr {
                OsStr::from_bytes(self.name_bytes())
            }

            #[inline(always)]
            pub fn ino(&self) -> u64 {
                self.inner.as_dirent().inode() as u64
            }

            cfg_select! {
                any(
                    // No type:
                    target_os = "solaris",
                    target_os = "illumos",
                    target_os = "aix",
                    target_os = "nto",
                    target_os = "haiku",
                    target_os = "vxworks",
                    // Neither ino nor type:
                    target_os = "vita",
                    target_os = "nuttx",
                ) => {
                    #[inline]
                    pub fn file_type(&self) -> io::Result<FileType> {
                        self.metadata().map(|m| m.file_type())
                    }
                }
                _ => {
                    #[inline(always)]
                    const fn eager_type(&self) -> EagerFileType {
                        self.inner.as_dirent().eager_type()
                    }
                    #[inline]
                    pub fn file_type(&self) -> io::Result<FileType> {
                        match self.eager_type().as_file_type() {
                            Some(ty) => Ok(ty),
                            None => self.metadata().map(|m| m.file_type()),
                        }
                    }
                }
            }

            cfg_select! {
                all(
                    any(
                        target_os = "linux",
                        target_os = "android",
                        target_os = "fuchsia",
                        target_os = "hurd",
                        target_os = "illumos",
                        target_vendor = "apple",
                    ),
                    not(miri) // no dirfd on Miri
                ) => {
                    pub fn metadata(&self) -> io::Result<FileAttr> {
                        use dir_fd::FileDescriptorHandle;
                        let fd = *self.inner.parent.as_fd();
                        let name = self.name_cstr().as_ptr();

                        cfg_has_statx! {
                            if let Some(ret) = unsafe { try_statx(
                                fd,
                                name,
                                libc::AT_SYMLINK_NOFOLLOW | libc::AT_STATX_SYNC_AS_STAT,
                                libc::STATX_BASIC_STATS | libc::STATX_BTIME,
                            ) } {
                                return ret;
                            }
                        }

                        let mut stat: stat64 = unsafe { mem::zeroed() };
                        cvt(unsafe { fstatat64(fd, name, &mut stat, libc::AT_SYMLINK_NOFOLLOW) })?;
                        Ok(FileAttr::from_stat64(stat))
                    }
                }
                _ => {
                    pub fn metadata(&self) -> io::Result<FileAttr> {
                        run_path_with_cstr(&self.path(), &lstat)
                    }
                }
            }

        }
    }
    _ => {
        impl DirEntry {
            pub fn path(&self) -> PathBuf {
                self.dir.root.join(self.file_name_os_str())
            }

            pub fn file_name(&self) -> OsString {
                self.file_name_os_str().to_os_string()
            }

            cfg_select! {
                all(
                    any(
                        target_os = "linux",
                        target_os = "android",
                        target_os = "fuchsia",
                        target_os = "hurd",
                        target_os = "illumos",
                        target_vendor = "apple",
                    ),
                    not(miri) // no dirfd on Miri
                ) => {
                    pub fn metadata(&self) -> io::Result<FileAttr> {
                        let fd = cvt(unsafe { dirfd(self.dir.dirp.0) })?;
                        let name = self.name_cstr().as_ptr();

                        cfg_has_statx! {
                            if let Some(ret) = unsafe { try_statx(
                                fd,
                                name,
                                libc::AT_SYMLINK_NOFOLLOW | libc::AT_STATX_SYNC_AS_STAT,
                                libc::STATX_BASIC_STATS | libc::STATX_BTIME,
                            ) } {
                                return ret;
                            }
                        }

                        let mut stat: stat64 = unsafe { mem::zeroed() };
                        cvt(unsafe { fstatat64(fd, name, &mut stat, libc::AT_SYMLINK_NOFOLLOW) })?;
                        Ok(FileAttr::from_stat64(stat))
                    }
                }
                _ => {
                    pub fn metadata(&self) -> io::Result<FileAttr> {
                        run_path_with_cstr(&self.path(), &lstat)
                    }
                }
            }

            cfg_select! {
                any(
                    target_os = "solaris",
                    target_os = "illumos",
                    target_os = "haiku",
                    target_os = "vxworks",
                    target_os = "aix",
                    target_os = "nto",
                    target_os = "vita",
                ) => {
                    pub fn file_type(&self) -> io::Result<FileType> {
                        self.metadata().map(|m| m.file_type())
                    }
                }
                _ => {
                    pub fn file_type(&self) -> io::Result<FileType> {
                        match self.entry.d_type {
                            libc::DT_CHR => Ok(FileType { mode: libc::S_IFCHR }),
                            libc::DT_FIFO => Ok(FileType { mode: libc::S_IFIFO }),
                            libc::DT_LNK => Ok(FileType { mode: libc::S_IFLNK }),
                            libc::DT_REG => Ok(FileType { mode: libc::S_IFREG }),
                            libc::DT_SOCK => Ok(FileType { mode: libc::S_IFSOCK }),
                            libc::DT_DIR => Ok(FileType { mode: libc::S_IFDIR }),
                            libc::DT_BLK => Ok(FileType { mode: libc::S_IFBLK }),
                            _ => self.metadata().map(|m| m.file_type()),
                        }
                    }
                }
            }

            cfg_select! {
                any(
                    target_os = "aix",
                    target_os = "android",
                    target_os = "cygwin",
                    target_os = "emscripten",
                    target_os = "espidf",
                    target_os = "freebsd",
                    target_os = "fuchsia",
                    target_os = "haiku",
                    target_os = "horizon",
                    target_os = "hurd",
                    target_os = "illumos",
                    target_os = "l4re",
                    target_os = "linux",
                    target_os = "nto",
                    target_os = "redox",
                    target_os = "rtems",
                    target_os = "solaris",
                    target_os = "vita",
                    target_os = "vxworks",
                    target_os = "wasi",
                    target_vendor = "apple",
                ) => {
                    pub fn ino(&self) -> u64 {
                        self.entry.d_ino as u64
                    }
                }
                any(target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly") => {
                    pub fn ino(&self) -> u64 {
                        self.entry.d_fileno as u64
                    }
                }
                target_os = "nuttx" => {
                    pub fn ino(&self) -> u64 {
                        // Leave this 0 for now, as NuttX does not provide an inode number
                        // in its directory entries.
                        0
                    }
                }
            }

            cfg_select! {
                any(
                    target_os = "netbsd",
                    target_os = "openbsd",
                    target_os = "dragonfly",
                    target_vendor = "apple",
                ) => {
                    fn name_bytes(&self) -> &[u8] {
                        use crate::slice;
                        unsafe {
                            slice::from_raw_parts(
                                self.entry.d_name.as_ptr() as *const u8,
                                self.entry.d_namlen as usize,
                            )
                        }
                    }
                }
                _ => {
                    fn name_bytes(&self) -> &[u8] {
                        self.name_cstr().to_bytes()
                    }
                }
            }

            cfg_select! {
                any(
                    target_os = "android",
                    target_os = "freebsd",
                    target_os = "linux",
                    target_os = "solaris",
                    target_os = "illumos",
                    target_os = "fuchsia",
                    target_os = "redox",
                    target_os = "aix",
                    target_os = "nto",
                    target_os = "vita",
                    target_os = "hurd",
                    target_os = "wasi",
                ) => {
                    fn name_cstr(&self) -> &CStr {
                        &self.name
                    }
                }
                _ => {
                    fn name_cstr(&self) -> &CStr {
                        unsafe { CStr::from_ptr(self.entry.d_name.as_ptr()) }
                    }
                }
            }

            pub fn file_name_os_str(&self) -> &OsStr {
                OsStr::from_bytes(self.name_bytes())
            }
        }
    }
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {
            // generic
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            // system-specific
            custom_flags: 0,
            mode: 0o666,
        }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }
    pub fn write(&mut self, write: bool) {
        self.write = write;
    }
    pub fn append(&mut self, append: bool) {
        self.append = append;
    }
    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }
    pub fn create(&mut self, create: bool) {
        self.create = create;
    }
    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }

    pub fn custom_flags(&mut self, flags: i32) {
        self.custom_flags = flags;
    }
    #[cfg(not(target_os = "wasi"))]
    pub fn mode(&mut self, mode: u32) {
        self.mode = mode as mode_t;
    }

    fn get_access_mode(&self) -> io::Result<c_int> {
        match (self.read, self.write, self.append) {
            (true, false, false) => Ok(libc::O_RDONLY),
            (false, true, false) => Ok(libc::O_WRONLY),
            (true, true, false) => Ok(libc::O_RDWR),
            (false, _, true) => Ok(libc::O_WRONLY | libc::O_APPEND),
            (true, _, true) => Ok(libc::O_RDWR | libc::O_APPEND),
            (false, false, false) => {
                // If no access mode is set, check if any creation flags are set
                // to provide a more descriptive error message
                if self.create || self.create_new || self.truncate {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "creating or truncating a file requires write or append access",
                    ))
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "must specify at least one of read, write, or append access",
                    ))
                }
            }
        }
    }

    fn get_creation_mode(&self) -> io::Result<c_int> {
        match (self.write, self.append) {
            (true, false) => {}
            (false, false) => {
                if self.truncate || self.create || self.create_new {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "creating or truncating a file requires write or append access",
                    ));
                }
            }
            (_, true) => {
                if self.truncate && !self.create_new {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "creating or truncating a file requires write or append access",
                    ));
                }
            }
        }

        Ok(match (self.create, self.truncate, self.create_new) {
            (false, false, false) => 0,
            (true, false, false) => libc::O_CREAT,
            (false, true, false) => libc::O_TRUNC,
            (true, true, false) => libc::O_CREAT | libc::O_TRUNC,
            (_, _, true) => libc::O_CREAT | libc::O_EXCL,
        })
    }
}

impl fmt::Debug for OpenOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let OpenOptions { read, write, append, truncate, create, create_new, custom_flags, mode } =
            self;
        f.debug_struct("OpenOptions")
            .field("read", read)
            .field("write", write)
            .field("append", append)
            .field("truncate", truncate)
            .field("create", create)
            .field("create_new", create_new)
            .field("custom_flags", custom_flags)
            .field("mode", &Mode(*mode))
            .finish()
    }
}

impl File {
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        run_path_with_cstr(path, &|path| File::open_c(path, opts))
    }

    pub fn open_c(path: &CStr, opts: &OpenOptions) -> io::Result<File> {
        let flags = libc::O_CLOEXEC
            | opts.get_access_mode()?
            | opts.get_creation_mode()?
            | (opts.custom_flags as c_int & !libc::O_ACCMODE);
        // The third argument of `open64` is documented to have type `mode_t`. On
        // some platforms (like macOS, where `open64` is actually `open`), `mode_t` is `u16`.
        // However, since this is a variadic function, C integer promotion rules mean that on
        // the ABI level, this still gets passed as `c_int` (aka `u32` on Unix platforms).
        let fd = cvt_r(|| unsafe { open64(path.as_ptr(), flags, opts.mode as c_int) })?;
        Ok(File(unsafe { FileDesc::from_raw_fd(fd) }))
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        let fd = self.as_raw_fd();

        cfg_has_statx! {
            if let Some(ret) = unsafe { try_statx(
                fd,
                c"".as_ptr() as *const c_char,
                libc::AT_EMPTY_PATH | libc::AT_STATX_SYNC_AS_STAT,
                libc::STATX_BASIC_STATS | libc::STATX_BTIME,
            ) } {
                return ret;
            }
        }

        let mut stat: stat64 = unsafe { mem::zeroed() };
        cvt(unsafe { fstat64(fd, &mut stat) })?;
        Ok(FileAttr::from_stat64(stat))
    }

    pub fn fsync(&self) -> io::Result<()> {
        cvt_r(|| unsafe { os_fsync(self.as_raw_fd()) })?;
        return Ok(());

        #[cfg(target_vendor = "apple")]
        unsafe fn os_fsync(fd: c_int) -> c_int {
            libc::fcntl(fd, libc::F_FULLFSYNC)
        }
        #[cfg(not(target_vendor = "apple"))]
        unsafe fn os_fsync(fd: c_int) -> c_int {
            libc::fsync(fd)
        }
    }

    pub fn datasync(&self) -> io::Result<()> {
        cvt_r(|| unsafe { os_datasync(self.as_raw_fd()) })?;
        return Ok(());

        #[cfg(target_vendor = "apple")]
        unsafe fn os_datasync(fd: c_int) -> c_int {
            libc::fcntl(fd, libc::F_FULLFSYNC)
        }
        #[cfg(any(
            target_os = "freebsd",
            target_os = "fuchsia",
            target_os = "linux",
            target_os = "cygwin",
            target_os = "android",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "nto",
            target_os = "hurd",
        ))]
        unsafe fn os_datasync(fd: c_int) -> c_int {
            libc::fdatasync(fd)
        }
        #[cfg(not(any(
            target_os = "android",
            target_os = "fuchsia",
            target_os = "freebsd",
            target_os = "linux",
            target_os = "cygwin",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "nto",
            target_os = "hurd",
            target_vendor = "apple",
        )))]
        unsafe fn os_datasync(fd: c_int) -> c_int {
            libc::fsync(fd)
        }
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    ))]
    pub fn lock(&self) -> io::Result<()> {
        cvt(unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_EX) })?;
        return Ok(());
    }

    #[cfg(target_os = "solaris")]
    pub fn lock(&self) -> io::Result<()> {
        let mut flock: libc::flock = unsafe { mem::zeroed() };
        flock.l_type = libc::F_WRLCK as libc::c_short;
        flock.l_whence = libc::SEEK_SET as libc::c_short;
        cvt(unsafe { libc::fcntl(self.as_raw_fd(), libc::F_SETLKW, &flock) })?;
        Ok(())
    }

    #[cfg(not(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "solaris",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    )))]
    pub fn lock(&self) -> io::Result<()> {
        Err(io::const_error!(io::ErrorKind::Unsupported, "lock() not supported"))
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    ))]
    pub fn lock_shared(&self) -> io::Result<()> {
        cvt(unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_SH) })?;
        return Ok(());
    }

    #[cfg(target_os = "solaris")]
    pub fn lock_shared(&self) -> io::Result<()> {
        let mut flock: libc::flock = unsafe { mem::zeroed() };
        flock.l_type = libc::F_RDLCK as libc::c_short;
        flock.l_whence = libc::SEEK_SET as libc::c_short;
        cvt(unsafe { libc::fcntl(self.as_raw_fd(), libc::F_SETLKW, &flock) })?;
        Ok(())
    }

    #[cfg(not(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "solaris",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    )))]
    pub fn lock_shared(&self) -> io::Result<()> {
        Err(io::const_error!(io::ErrorKind::Unsupported, "lock_shared() not supported"))
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    ))]
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        let result = cvt(unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) });
        if let Err(err) = result {
            if err.kind() == io::ErrorKind::WouldBlock {
                Err(TryLockError::WouldBlock)
            } else {
                Err(TryLockError::Error(err))
            }
        } else {
            Ok(())
        }
    }

    #[cfg(target_os = "solaris")]
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        let mut flock: libc::flock = unsafe { mem::zeroed() };
        flock.l_type = libc::F_WRLCK as libc::c_short;
        flock.l_whence = libc::SEEK_SET as libc::c_short;
        let result = cvt(unsafe { libc::fcntl(self.as_raw_fd(), libc::F_SETLK, &flock) });
        if let Err(err) = result {
            if err.kind() == io::ErrorKind::WouldBlock {
                Err(TryLockError::WouldBlock)
            } else {
                Err(TryLockError::Error(err))
            }
        } else {
            Ok(())
        }
    }

    #[cfg(not(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "solaris",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    )))]
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(io::const_error!(
            io::ErrorKind::Unsupported,
            "try_lock() not supported"
        )))
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    ))]
    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        let result = cvt(unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) });
        if let Err(err) = result {
            if err.kind() == io::ErrorKind::WouldBlock {
                Err(TryLockError::WouldBlock)
            } else {
                Err(TryLockError::Error(err))
            }
        } else {
            Ok(())
        }
    }

    #[cfg(target_os = "solaris")]
    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        let mut flock: libc::flock = unsafe { mem::zeroed() };
        flock.l_type = libc::F_RDLCK as libc::c_short;
        flock.l_whence = libc::SEEK_SET as libc::c_short;
        let result = cvt(unsafe { libc::fcntl(self.as_raw_fd(), libc::F_SETLK, &flock) });
        if let Err(err) = result {
            if err.kind() == io::ErrorKind::WouldBlock {
                Err(TryLockError::WouldBlock)
            } else {
                Err(TryLockError::Error(err))
            }
        } else {
            Ok(())
        }
    }

    #[cfg(not(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "solaris",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    )))]
    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(io::const_error!(
            io::ErrorKind::Unsupported,
            "try_lock_shared() not supported"
        )))
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    ))]
    pub fn unlock(&self) -> io::Result<()> {
        cvt(unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_UN) })?;
        return Ok(());
    }

    #[cfg(target_os = "solaris")]
    pub fn unlock(&self) -> io::Result<()> {
        let mut flock: libc::flock = unsafe { mem::zeroed() };
        flock.l_type = libc::F_UNLCK as libc::c_short;
        flock.l_whence = libc::SEEK_SET as libc::c_short;
        cvt(unsafe { libc::fcntl(self.as_raw_fd(), libc::F_SETLKW, &flock) })?;
        Ok(())
    }

    #[cfg(not(any(
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "cygwin",
        target_os = "solaris",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    )))]
    pub fn unlock(&self) -> io::Result<()> {
        Err(io::const_error!(io::ErrorKind::Unsupported, "unlock() not supported"))
    }

    pub fn truncate(&self, size: u64) -> io::Result<()> {
        let size: off64_t =
            size.try_into().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        cvt_r(|| unsafe { ftruncate64(self.as_raw_fd(), size) }).map(drop)
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.0.read_vectored(bufs)
    }

    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        self.0.is_read_vectored()
    }

    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.0.read_at(buf, offset)
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        self.0.read_buf(cursor)
    }

    pub fn read_buf_at(&self, cursor: BorrowedCursor<'_>, offset: u64) -> io::Result<()> {
        self.0.read_buf_at(cursor, offset)
    }

    pub fn read_vectored_at(&self, bufs: &mut [IoSliceMut<'_>], offset: u64) -> io::Result<usize> {
        self.0.read_vectored_at(bufs, offset)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }

    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    pub fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.0.write_at(buf, offset)
    }

    pub fn write_vectored_at(&self, bufs: &[IoSlice<'_>], offset: u64) -> io::Result<usize> {
        self.0.write_vectored_at(bufs, offset)
    }

    #[inline]
    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (whence, pos) = match pos {
            // Casting to `i64` is fine, too large values will end up as
            // negative which will cause an error in `lseek64`.
            SeekFrom::Start(off) => (libc::SEEK_SET, off as i64),
            SeekFrom::End(off) => (libc::SEEK_END, off),
            SeekFrom::Current(off) => (libc::SEEK_CUR, off),
        };
        let n = cvt(unsafe { lseek64(self.as_raw_fd(), pos as off64_t, whence) })?;
        Ok(n as u64)
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        match self.file_attr().map(|attr| attr.size()) {
            // Fall back to default implementation if the returned size is 0,
            // we might be in a proc mount.
            Ok(0) => None,
            result => Some(result),
        }
    }

    pub fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub fn duplicate(&self) -> io::Result<File> {
        self.0.duplicate().map(File)
    }

    pub fn set_permissions(&self, perm: FilePermissions) -> io::Result<()> {
        cvt_r(|| unsafe { libc::fchmod(self.as_raw_fd(), perm.mode) })?;
        Ok(())
    }

    pub fn set_times(&self, times: FileTimes) -> io::Result<()> {
        cfg_select! {
            any(target_os = "redox", target_os = "espidf", target_os = "horizon", target_os = "nuttx") => {
                // Redox doesn't appear to support `UTIME_OMIT`.
                // ESP-IDF and HorizonOS do not support `futimens` at all and the behavior for those OS is therefore
                // the same as for Redox.
                let _ = times;
                Err(io::const_error!(
                    io::ErrorKind::Unsupported,
                    "setting file times not supported",
                ))
            }
            target_vendor = "apple" => {
                let ta = TimesAttrlist::from_times(&times)?;
                cvt(unsafe { libc::fsetattrlist(
                    self.as_raw_fd(),
                    ta.attrlist(),
                    ta.times_buf(),
                    ta.times_buf_size(),
                    0
                ) })?;
                Ok(())
            }
            target_os = "android" => {
                let times = [file_time_to_timespec(times.accessed)?, file_time_to_timespec(times.modified)?];
                // futimens requires Android API level 19
                cvt(unsafe {
                    weak!(
                        fn futimens(fd: c_int, times: *const libc::timespec) -> c_int;
                    );
                    match futimens.get() {
                        Some(futimens) => futimens(self.as_raw_fd(), times.as_ptr()),
                        None => return Err(io::const_error!(
                            io::ErrorKind::Unsupported,
                            "setting file times requires Android API level >= 19",
                        )),
                    }
                })?;
                Ok(())
            }
            _ => {
                #[cfg(all(target_os = "linux", target_env = "gnu", target_pointer_width = "32", not(target_arch = "riscv32")))]
                {
                    use crate::sys::{time::__timespec64, weak::weak};

                    // Added in glibc 2.34
                    weak!(
                        fn __futimens64(fd: c_int, times: *const __timespec64) -> c_int;
                    );

                    if let Some(futimens64) = __futimens64.get() {
                        let to_timespec = |time: Option<SystemTime>| time.map(|time| time.t.to_timespec64())
                            .unwrap_or(__timespec64::new(0, libc::UTIME_OMIT as _));
                        let times = [to_timespec(times.accessed), to_timespec(times.modified)];
                        cvt(unsafe { futimens64(self.as_raw_fd(), times.as_ptr()) })?;
                        return Ok(());
                    }
                }
                let times = [file_time_to_timespec(times.accessed)?, file_time_to_timespec(times.modified)?];
                cvt(unsafe { libc::futimens(self.as_raw_fd(), times.as_ptr()) })?;
                Ok(())
            }
        }
    }
}

#[cfg(not(any(
    target_os = "redox",
    target_os = "espidf",
    target_os = "horizon",
    target_os = "nuttx",
)))]
fn file_time_to_timespec(time: Option<SystemTime>) -> io::Result<libc::timespec> {
    match time {
        Some(time) if let Some(ts) = time.t.to_timespec() => Ok(ts),
        Some(time) if time > crate::sys::time::UNIX_EPOCH => Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "timestamp is too large to set as a file time",
        )),
        Some(_) => Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "timestamp is too small to set as a file time",
        )),
        None => Ok(libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT as _ }),
    }
}

#[cfg(target_vendor = "apple")]
struct TimesAttrlist {
    buf: [mem::MaybeUninit<libc::timespec>; 3],
    attrlist: libc::attrlist,
    num_times: usize,
}

#[cfg(target_vendor = "apple")]
impl TimesAttrlist {
    fn from_times(times: &FileTimes) -> io::Result<Self> {
        let mut this = Self {
            buf: [mem::MaybeUninit::<libc::timespec>::uninit(); 3],
            attrlist: unsafe { mem::zeroed() },
            num_times: 0,
        };
        this.attrlist.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
        if times.created.is_some() {
            this.buf[this.num_times].write(file_time_to_timespec(times.created)?);
            this.num_times += 1;
            this.attrlist.commonattr |= libc::ATTR_CMN_CRTIME;
        }
        if times.modified.is_some() {
            this.buf[this.num_times].write(file_time_to_timespec(times.modified)?);
            this.num_times += 1;
            this.attrlist.commonattr |= libc::ATTR_CMN_MODTIME;
        }
        if times.accessed.is_some() {
            this.buf[this.num_times].write(file_time_to_timespec(times.accessed)?);
            this.num_times += 1;
            this.attrlist.commonattr |= libc::ATTR_CMN_ACCTIME;
        }
        Ok(this)
    }

    fn attrlist(&self) -> *mut libc::c_void {
        (&raw const self.attrlist).cast::<libc::c_void>().cast_mut()
    }

    fn times_buf(&self) -> *mut libc::c_void {
        self.buf.as_ptr().cast::<libc::c_void>().cast_mut()
    }

    fn times_buf_size(&self) -> usize {
        self.num_times * size_of::<libc::timespec>()
    }
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder { mode: 0o777 }
    }

    pub fn mkdir(&self, p: &Path) -> io::Result<()> {
        run_path_with_cstr(p, &|p| cvt(unsafe { libc::mkdir(p.as_ptr(), self.mode) }).map(|_| ()))
    }

    #[cfg(not(target_os = "wasi"))]
    pub fn set_mode(&mut self, mode: u32) {
        self.mode = mode as mode_t;
    }
}

impl fmt::Debug for DirBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let DirBuilder { mode } = self;
        f.debug_struct("DirBuilder").field("mode", &Mode(*mode)).finish()
    }
}

impl AsInner<FileDesc> for File {
    #[inline]
    fn as_inner(&self) -> &FileDesc {
        &self.0
    }
}

impl AsInnerMut<FileDesc> for File {
    #[inline]
    fn as_inner_mut(&mut self) -> &mut FileDesc {
        &mut self.0
    }
}

impl IntoInner<FileDesc> for File {
    fn into_inner(self) -> FileDesc {
        self.0
    }
}

impl FromInner<FileDesc> for File {
    fn from_inner(file_desc: FileDesc) -> Self {
        Self(file_desc)
    }
}

impl AsFd for File {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for File {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl IntoRawFd for File {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl FromRawFd for File {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        Self(FromRawFd::from_raw_fd(raw_fd))
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(any(target_os = "linux", target_os = "illumos", target_os = "solaris"))]
        fn get_path(fd: c_int) -> Option<PathBuf> {
            let mut p = PathBuf::from("/proc/self/fd");
            p.push(&fd.to_string());
            run_path_with_cstr(&p, &readlink).ok()
        }

        #[cfg(any(target_vendor = "apple", target_os = "netbsd"))]
        fn get_path(fd: c_int) -> Option<PathBuf> {
            // FIXME: The use of PATH_MAX is generally not encouraged, but it
            // is inevitable in this case because Apple targets and NetBSD define `fcntl`
            // with `F_GETPATH` in terms of `MAXPATHLEN`, and there are no
            // alternatives. If a better method is invented, it should be used
            // instead.
            let mut buf = vec![0; libc::PATH_MAX as usize];
            let n = unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_ptr()) };
            if n == -1 {
                cfg_select! {
                    target_os = "netbsd" => {
                        // fallback to procfs as last resort
                        let mut p = PathBuf::from("/proc/self/fd");
                        p.push(&fd.to_string());
                        return run_path_with_cstr(&p, &readlink).ok()
                    }
                    _ => {
                        return None;
                    }
                }
            }
            let l = buf.iter().position(|&c| c == 0).unwrap();
            buf.truncate(l as usize);
            buf.shrink_to_fit();
            Some(PathBuf::from(OsString::from_vec(buf)))
        }

        #[cfg(target_os = "freebsd")]
        fn get_path(fd: c_int) -> Option<PathBuf> {
            let info = Box::<libc::kinfo_file>::new_zeroed();
            let mut info = unsafe { info.assume_init() };
            info.kf_structsize = size_of::<libc::kinfo_file>() as libc::c_int;
            let n = unsafe { libc::fcntl(fd, libc::F_KINFO, &mut *info) };
            if n == -1 {
                return None;
            }
            let buf = unsafe { CStr::from_ptr(info.kf_path.as_mut_ptr()).to_bytes().to_vec() };
            Some(PathBuf::from(OsString::from_vec(buf)))
        }

        #[cfg(target_os = "vxworks")]
        fn get_path(fd: c_int) -> Option<PathBuf> {
            let mut buf = vec![0; libc::PATH_MAX as usize];
            let n = unsafe { libc::ioctl(fd, libc::FIOGETNAME, buf.as_ptr()) };
            if n == -1 {
                return None;
            }
            let l = buf.iter().position(|&c| c == 0).unwrap();
            buf.truncate(l as usize);
            Some(PathBuf::from(OsString::from_vec(buf)))
        }

        #[cfg(not(any(
            target_os = "linux",
            target_os = "vxworks",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "illumos",
            target_os = "solaris",
            target_vendor = "apple",
        )))]
        fn get_path(_fd: c_int) -> Option<PathBuf> {
            // FIXME(#24570): implement this for other Unix platforms
            None
        }

        fn get_mode(fd: c_int) -> Option<(bool, bool)> {
            let mode = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if mode == -1 {
                return None;
            }
            match mode & libc::O_ACCMODE {
                libc::O_RDONLY => Some((true, false)),
                libc::O_RDWR => Some((true, true)),
                libc::O_WRONLY => Some((false, true)),
                _ => None,
            }
        }

        let fd = self.as_raw_fd();
        let mut b = f.debug_struct("File");
        b.field("fd", &fd);
        if let Some(path) = get_path(fd) {
            b.field("path", &path);
        }
        if let Some((read, write)) = get_mode(fd) {
            b.field("read", &read).field("write", &write);
        }
        b.finish()
    }
}

// Format in octal, followed by the mode format used in `ls -l`.
//
// References:
//   https://pubs.opengroup.org/onlinepubs/009696899/utilities/ls.html
//   https://www.gnu.org/software/libc/manual/html_node/Testing-File-Type.html
//   https://www.gnu.org/software/libc/manual/html_node/Permission-Bits.html
//
// Example:
//   0o100664 (-rw-rw-r--)
impl fmt::Debug for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(mode) = *self;
        write!(f, "0o{mode:06o}")?;

        let entry_type = match mode & libc::S_IFMT {
            libc::S_IFDIR => 'd',
            libc::S_IFBLK => 'b',
            libc::S_IFCHR => 'c',
            libc::S_IFLNK => 'l',
            libc::S_IFIFO => 'p',
            libc::S_IFREG => '-',
            _ => return Ok(()),
        };

        f.write_str(" (")?;
        f.write_char(entry_type)?;

        // Owner permissions
        f.write_char(if mode & libc::S_IRUSR != 0 { 'r' } else { '-' })?;
        f.write_char(if mode & libc::S_IWUSR != 0 { 'w' } else { '-' })?;
        let owner_executable = mode & libc::S_IXUSR != 0;
        let setuid = mode as c_int & libc::S_ISUID as c_int != 0;
        f.write_char(match (owner_executable, setuid) {
            (true, true) => 's',  // executable and setuid
            (false, true) => 'S', // setuid
            (true, false) => 'x', // executable
            (false, false) => '-',
        })?;

        // Group permissions
        f.write_char(if mode & libc::S_IRGRP != 0 { 'r' } else { '-' })?;
        f.write_char(if mode & libc::S_IWGRP != 0 { 'w' } else { '-' })?;
        let group_executable = mode & libc::S_IXGRP != 0;
        let setgid = mode as c_int & libc::S_ISGID as c_int != 0;
        f.write_char(match (group_executable, setgid) {
            (true, true) => 's',  // executable and setgid
            (false, true) => 'S', // setgid
            (true, false) => 'x', // executable
            (false, false) => '-',
        })?;

        // Other permissions
        f.write_char(if mode & libc::S_IROTH != 0 { 'r' } else { '-' })?;
        f.write_char(if mode & libc::S_IWOTH != 0 { 'w' } else { '-' })?;
        let other_executable = mode & libc::S_IXOTH != 0;
        let sticky = mode as c_int & libc::S_ISVTX as c_int != 0;
        f.write_char(match (entry_type, other_executable, sticky) {
            ('d', true, true) => 't',  // searchable and restricted deletion
            ('d', false, true) => 'T', // restricted deletion
            (_, true, _) => 'x',       // executable
            (_, false, _) => '-',
        })?;

        f.write_char(')')
    }
}

cfg_select_has_getdents! {
    => {
        pub fn readdir(path: &Path) -> io::Result<ReadDir> {
            let ptr = run_path_with_cstr(path, &|p| unsafe { Ok(libc::opendir(p.as_ptr())) })?;
            if ptr.is_null() {
                Err(Error::last_os_error())
            } else {
                let dir = dir_fd::DirWithPath::from_dir_and_path(Dir(ptr), path);
                let dir = create_dir_path_handle(dir);
                let buf = get_read_dir_buffer_handle();
                let inner = ReadDirWithBuf::with_buf(dir, buf);
                Ok(ReadDir::new(inner))
            }
        }
    }
    _ => {
        pub fn readdir(path: &Path) -> io::Result<ReadDir> {
            let ptr = run_path_with_cstr(path, &|p| unsafe { Ok(libc::opendir(p.as_ptr())) })?;
            if ptr.is_null() {
                Err(Error::last_os_error())
            } else {
                let root = path.to_path_buf();
                let inner = InnerReadDir { dirp: Dir(ptr), root };
                Ok(ReadDir::new(inner))
            }
        }

    }
}

pub fn unlink(p: &CStr) -> io::Result<()> {
    cvt(unsafe { libc::unlink(p.as_ptr()) }).map(|_| ())
}

pub fn rename(old: &CStr, new: &CStr) -> io::Result<()> {
    cvt(unsafe { libc::rename(old.as_ptr(), new.as_ptr()) }).map(|_| ())
}

pub fn set_perm(p: &CStr, perm: FilePermissions) -> io::Result<()> {
    cvt_r(|| unsafe { libc::chmod(p.as_ptr(), perm.mode) }).map(|_| ())
}

pub fn rmdir(p: &CStr) -> io::Result<()> {
    cvt(unsafe { libc::rmdir(p.as_ptr()) }).map(|_| ())
}

pub fn readlink(c_path: &CStr) -> io::Result<PathBuf> {
    let p = c_path.as_ptr();

    let mut buf = Vec::with_capacity(256);

    loop {
        let buf_read =
            cvt(unsafe { libc::readlink(p, buf.as_mut_ptr() as *mut _, buf.capacity()) })? as usize;

        unsafe {
            buf.set_len(buf_read);
        }

        if buf_read != buf.capacity() {
            buf.shrink_to_fit();

            return Ok(PathBuf::from(OsString::from_vec(buf)));
        }

        // Trigger the internal buffer resizing logic of `Vec` by requiring
        // more space than the current capacity. The length is guaranteed to be
        // the same as the capacity due to the if statement above.
        buf.reserve(1);
    }
}

pub fn symlink(original: &CStr, link: &CStr) -> io::Result<()> {
    cvt(unsafe { libc::symlink(original.as_ptr(), link.as_ptr()) }).map(|_| ())
}

pub fn link(original: &CStr, link: &CStr) -> io::Result<()> {
    cfg_select! {
        any(
            // VxWorks, Redox and ESP-IDF lack `linkat`, so use `link` instead.
            // POSIX leaves it implementation-defined whether `link` follows
            // symlinks, so rely on the `symlink_hard_link` test in
            // library/std/src/fs/tests.rs to check the behavior.
            target_os = "vxworks",
            target_os = "redox",
            target_os = "espidf",
            // Android has `linkat` on newer versions, but we happen to know
            // `link` always has the correct behavior, so it's here as well.
            target_os = "android",
            // wasi-sdk-29-and-prior have a buggy `linkat` so use `link` instead
            // until wasi-sdk is updated (see WebAssembly/wasi-libc#690)
            target_os = "wasi",
            // Other misc platforms
            target_os = "horizon",
            target_os = "vita",
            target_env = "nto70",
        ) => {
            cvt(unsafe { libc::link(original.as_ptr(), link.as_ptr()) })?;
        }
        _ => {
            // Where we can, use `linkat` instead of `link`; see the comment above
            // this one for details on why.
            cvt(unsafe { libc::linkat(libc::AT_FDCWD, original.as_ptr(), libc::AT_FDCWD, link.as_ptr(), 0) })?;
        }
    }
    Ok(())
}

pub fn stat(p: &CStr) -> io::Result<FileAttr> {
    cfg_has_statx! {
        if let Some(ret) = unsafe { try_statx(
            libc::AT_FDCWD,
            p.as_ptr(),
            libc::AT_STATX_SYNC_AS_STAT,
            libc::STATX_BASIC_STATS | libc::STATX_BTIME,
        ) } {
            return ret;
        }
    }

    let mut stat: stat64 = unsafe { mem::zeroed() };
    cvt(unsafe { stat64(p.as_ptr(), &mut stat) })?;
    Ok(FileAttr::from_stat64(stat))
}

pub fn lstat(p: &CStr) -> io::Result<FileAttr> {
    cfg_has_statx! {
        if let Some(ret) = unsafe { try_statx(
            libc::AT_FDCWD,
            p.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW | libc::AT_STATX_SYNC_AS_STAT,
            libc::STATX_BASIC_STATS | libc::STATX_BTIME,
        ) } {
            return ret;
        }
    }

    let mut stat: stat64 = unsafe { mem::zeroed() };
    cvt(unsafe { lstat64(p.as_ptr(), &mut stat) })?;
    Ok(FileAttr::from_stat64(stat))
}

pub fn canonicalize(path: &CStr) -> io::Result<PathBuf> {
    let r = unsafe { libc::realpath(path.as_ptr(), ptr::null_mut()) };
    if r.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_vec(unsafe {
        let buf = CStr::from_ptr(r).to_bytes().to_vec();
        libc::free(r as *mut _);
        buf
    })))
}

fn open_from(from: &Path) -> io::Result<(crate::fs::File, crate::fs::Metadata)> {
    use crate::fs::File;
    use crate::sys::fs::common::NOT_FILE_ERROR;

    let reader = File::open(from)?;
    let metadata = reader.metadata()?;
    if !metadata.is_file() {
        return Err(NOT_FILE_ERROR);
    }
    Ok((reader, metadata))
}

fn set_times_impl(p: &CStr, times: FileTimes, follow_symlinks: bool) -> io::Result<()> {
    cfg_select! {
       any(target_os = "redox", target_os = "espidf", target_os = "horizon", target_os = "nuttx") => {
            let _ = (p, times, follow_symlinks);
            Err(io::const_error!(
                io::ErrorKind::Unsupported,
                "setting file times not supported",
            ))
       }
       target_vendor = "apple" => {
            // Apple platforms use setattrlist which supports setting times on symlinks
            let ta = TimesAttrlist::from_times(&times)?;
            let options = if follow_symlinks {
                0
            } else {
                libc::FSOPT_NOFOLLOW
            };

            cvt(unsafe { libc::setattrlist(
                p.as_ptr(),
                ta.attrlist(),
                ta.times_buf(),
                ta.times_buf_size(),
                options as u32
            ) })?;
            Ok(())
       }
       target_os = "android" => {
            let times = [file_time_to_timespec(times.accessed)?, file_time_to_timespec(times.modified)?];
            let flags = if follow_symlinks { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
            // utimensat requires Android API level 19
            cvt(unsafe {
                weak!(
                    fn utimensat(dirfd: c_int, path: *const libc::c_char, times: *const libc::timespec, flags: c_int) -> c_int;
                );
                match utimensat.get() {
                    Some(utimensat) => utimensat(libc::AT_FDCWD, p.as_ptr(), times.as_ptr(), flags),
                    None => return Err(io::const_error!(
                        io::ErrorKind::Unsupported,
                        "setting file times requires Android API level >= 19",
                    )),
                }
            })?;
            Ok(())
       }
       _ => {
            let flags = if follow_symlinks { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
            #[cfg(all(target_os = "linux", target_env = "gnu", target_pointer_width = "32", not(target_arch = "riscv32")))]
            {
                use crate::sys::{time::__timespec64, weak::weak};

                // Added in glibc 2.34
                weak!(
                    fn __utimensat64(dirfd: c_int, path: *const c_char, times: *const __timespec64, flags: c_int) -> c_int;
                );

                if let Some(utimensat64) = __utimensat64.get() {
                    let to_timespec = |time: Option<SystemTime>| time.map(|time| time.t.to_timespec64())
                        .unwrap_or(__timespec64::new(0, libc::UTIME_OMIT as _));
                    let times = [to_timespec(times.accessed), to_timespec(times.modified)];
                    cvt(unsafe { utimensat64(libc::AT_FDCWD, p.as_ptr(), times.as_ptr(), flags) })?;
                    return Ok(());
                }
            }
            let times = [file_time_to_timespec(times.accessed)?, file_time_to_timespec(times.modified)?];
            cvt(unsafe { libc::utimensat(libc::AT_FDCWD, p.as_ptr(), times.as_ptr(), flags) })?;
            Ok(())
         }
    }
}

#[inline(always)]
pub fn set_times(p: &CStr, times: FileTimes) -> io::Result<()> {
    set_times_impl(p, times, true)
}

#[inline(always)]
pub fn set_times_nofollow(p: &CStr, times: FileTimes) -> io::Result<()> {
    set_times_impl(p, times, false)
}

#[cfg(any(target_os = "espidf", target_os = "wasi"))]
fn open_to_and_set_permissions(
    to: &Path,
    _reader_metadata: &crate::fs::Metadata,
) -> io::Result<(crate::fs::File, crate::fs::Metadata)> {
    use crate::fs::OpenOptions;
    let writer = OpenOptions::new().open(to)?;
    let writer_metadata = writer.metadata()?;
    Ok((writer, writer_metadata))
}

#[cfg(not(any(target_os = "espidf", target_os = "wasi")))]
fn open_to_and_set_permissions(
    to: &Path,
    reader_metadata: &crate::fs::Metadata,
) -> io::Result<(crate::fs::File, crate::fs::Metadata)> {
    use crate::fs::OpenOptions;
    use crate::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let perm = reader_metadata.permissions();
    let writer = OpenOptions::new()
        // create the file with the correct mode right away
        .mode(perm.mode())
        .write(true)
        .create(true)
        .truncate(true)
        .open(to)?;
    let writer_metadata = writer.metadata()?;
    // fchmod is broken on vita
    #[cfg(not(target_os = "vita"))]
    if writer_metadata.is_file() {
        // Set the correct file permissions, in case the file already existed.
        // Don't set the permissions on already existing non-files like
        // pipes/FIFOs or device nodes.
        writer.set_permissions(perm)?;
    }
    Ok((writer, writer_metadata))
}

mod cfm {
    use crate::fs::{File, Metadata};
    use crate::io::{BorrowedCursor, IoSlice, IoSliceMut, Read, Result, Write};

    #[allow(dead_code)]
    pub struct CachedFileMetadata(pub File, pub Metadata);

    impl Read for CachedFileMetadata {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            self.0.read(buf)
        }
        fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
            self.0.read_vectored(bufs)
        }
        fn read_buf(&mut self, cursor: BorrowedCursor<'_>) -> Result<()> {
            self.0.read_buf(cursor)
        }
        #[inline]
        fn is_read_vectored(&self) -> bool {
            self.0.is_read_vectored()
        }
        fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
            self.0.read_to_end(buf)
        }
        fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
            self.0.read_to_string(buf)
        }
    }
    impl Write for CachedFileMetadata {
        fn write(&mut self, buf: &[u8]) -> Result<usize> {
            self.0.write(buf)
        }
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
            self.0.write_vectored(bufs)
        }
        #[inline]
        fn is_write_vectored(&self) -> bool {
            self.0.is_write_vectored()
        }
        #[inline]
        fn flush(&mut self) -> Result<()> {
            self.0.flush()
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(in crate::sys) use cfm::CachedFileMetadata;

#[cfg(not(target_vendor = "apple"))]
pub fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    let (reader, reader_metadata) = open_from(from)?;
    let (writer, writer_metadata) = open_to_and_set_permissions(to, &reader_metadata)?;

    io::copy(
        &mut cfm::CachedFileMetadata(reader, reader_metadata),
        &mut cfm::CachedFileMetadata(writer, writer_metadata),
    )
}

#[cfg(target_vendor = "apple")]
pub fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    const COPYFILE_ALL: libc::copyfile_flags_t = libc::COPYFILE_METADATA | libc::COPYFILE_DATA;

    struct FreeOnDrop(libc::copyfile_state_t);
    impl Drop for FreeOnDrop {
        fn drop(&mut self) {
            // The code below ensures that `FreeOnDrop` is never a null pointer
            unsafe {
                // `copyfile_state_free` returns -1 if the `to` or `from` files
                // cannot be closed. However, this is not considered an error.
                libc::copyfile_state_free(self.0);
            }
        }
    }

    let (reader, reader_metadata) = open_from(from)?;

    let clonefile_result = run_path_with_cstr(to, &|to| {
        cvt(unsafe { libc::fclonefileat(reader.as_raw_fd(), libc::AT_FDCWD, to.as_ptr(), 0) })
    });
    match clonefile_result {
        Ok(_) => return Ok(reader_metadata.len()),
        Err(e) => match e.raw_os_error() {
            // `fclonefileat` will fail on non-APFS volumes, if the
            // destination already exists, or if the source and destination
            // are on different devices. In all these cases `fcopyfile`
            // should succeed.
            Some(libc::ENOTSUP) | Some(libc::EEXIST) | Some(libc::EXDEV) => (),
            _ => return Err(e),
        },
    }

    // Fall back to using `fcopyfile` if `fclonefileat` does not succeed.
    let (writer, writer_metadata) = open_to_and_set_permissions(to, &reader_metadata)?;

    // We ensure that `FreeOnDrop` never contains a null pointer so it is
    // always safe to call `copyfile_state_free`
    let state = unsafe {
        let state = libc::copyfile_state_alloc();
        if state.is_null() {
            return Err(crate::io::Error::last_os_error());
        }
        FreeOnDrop(state)
    };

    let flags = if writer_metadata.is_file() { COPYFILE_ALL } else { libc::COPYFILE_DATA };

    cvt(unsafe { libc::fcopyfile(reader.as_raw_fd(), writer.as_raw_fd(), state.0, flags) })?;

    let mut bytes_copied: libc::off_t = 0;
    cvt(unsafe {
        libc::copyfile_state_get(
            state.0,
            libc::COPYFILE_STATE_COPIED as u32,
            (&raw mut bytes_copied) as *mut libc::c_void,
        )
    })?;
    Ok(bytes_copied as u64)
}

#[cfg(not(target_os = "wasi"))]
pub fn chown(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    run_path_with_cstr(path, &|path| {
        cvt(unsafe { libc::chown(path.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) })
            .map(|_| ())
    })
}

#[cfg(not(target_os = "wasi"))]
pub fn fchown(fd: c_int, uid: u32, gid: u32) -> io::Result<()> {
    cvt(unsafe { libc::fchown(fd, uid as libc::uid_t, gid as libc::gid_t) })?;
    Ok(())
}

#[cfg(not(any(target_os = "vxworks", target_os = "wasi")))]
pub fn lchown(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    run_path_with_cstr(path, &|path| {
        cvt(unsafe { libc::lchown(path.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) })
            .map(|_| ())
    })
}

#[cfg(target_os = "vxworks")]
pub fn lchown(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    let (_, _, _) = (path, uid, gid);
    Err(io::const_error!(io::ErrorKind::Unsupported, "lchown not supported by vxworks"))
}

#[cfg(not(any(target_os = "fuchsia", target_os = "vxworks", target_os = "wasi")))]
pub fn chroot(dir: &Path) -> io::Result<()> {
    run_path_with_cstr(dir, &|dir| cvt(unsafe { libc::chroot(dir.as_ptr()) }).map(|_| ()))
}

#[cfg(target_os = "vxworks")]
pub fn chroot(dir: &Path) -> io::Result<()> {
    let _ = dir;
    Err(io::const_error!(io::ErrorKind::Unsupported, "chroot not supported by vxworks"))
}

#[cfg(not(target_os = "wasi"))]
pub fn mkfifo(path: &Path, mode: u32) -> io::Result<()> {
    run_path_with_cstr(path, &|path| {
        cvt(unsafe { libc::mkfifo(path.as_ptr(), mode.try_into().unwrap()) }).map(|_| ())
    })
}

pub use remove_dir_impl::remove_dir_all;

// Fallback for REDOX, ESP-ID, Horizon, Vita, Vxworks and Miri
#[cfg(any(
    target_os = "redox",
    target_os = "espidf",
    target_os = "horizon",
    target_os = "vita",
    target_os = "nto",
    target_os = "vxworks",
    miri
))]
mod remove_dir_impl {
    pub use crate::sys::fs::common::remove_dir_all;
}

// Modern implementation using openat(), unlinkat() and fdopendir()
#[cfg(not(any(
    target_os = "redox",
    target_os = "espidf",
    target_os = "horizon",
    target_os = "vita",
    target_os = "nto",
    target_os = "vxworks",
    miri
)))]
mod remove_dir_impl {
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    use libc::{fdopendir, openat, unlinkat};
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    use libc::{fdopendir, openat64 as openat, unlinkat};

    use super::{AsRawFd, Dir, DirEntry, FromRawFd, IntoRawFd, OwnedFd, RawFd, ReadDir, lstat};
    cfg_select_has_getdents! {
        => {
            use super::ReadDirWithBuf;
            use super::dir_fd::FileDescriptorHandle;
        }
        _ => {
            use super::InnerReadDir;
        }
    }
    use crate::ffi::CStr;
    use crate::io;
    use crate::path::{Path, PathBuf};
    use crate::sys::common::small_c_string::run_path_with_cstr;
    use crate::sys::{cvt, cvt_r};
    use crate::sys_common::ignore_notfound;

    pub fn openat_nofollow_dironly(parent_fd: Option<RawFd>, p: &CStr) -> io::Result<OwnedFd> {
        let fd = cvt_r(|| unsafe {
            openat(
                parent_fd.unwrap_or(libc::AT_FDCWD),
                p.as_ptr(),
                libc::O_CLOEXEC | libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        })?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    cfg_select_has_getdents! {
        => {
            fn fdreaddir(dir_fd: OwnedFd) -> io::Result<(ReadDir, RawFd)> {
                // a valid root is not needed because we do not call any functions involving the
                // full path of the `DirEntry`s.
                let dir = super::dir_fd::DirWithPath::from_owned_dir_fd_and_path(dir_fd, PathBuf::new());
                let dir = super::create_dir_path_handle(dir);
                let buf = super::get_read_dir_buffer_handle();
                let inner = ReadDirWithBuf::with_buf(dir, buf);
                /* FIXME: THIS TOTALLY BREAKS ALL OUR CAREFUL PLANNING WITH THE MUTEX AND TRAIT! */
                let fd = *inner.as_fd();
                let ret = ReadDir::new(inner);
                Ok((ret, fd))
            }
        }
        _ => {
            fn fdreaddir(dir_fd: OwnedFd) -> io::Result<(ReadDir, RawFd)> {
                let ptr = unsafe { fdopendir(dir_fd.as_raw_fd()) };
                if ptr.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let dirp = Dir(ptr);
                // file descriptor is automatically closed by libc::closedir() now, so give up ownership
                let new_parent_fd = dir_fd.into_raw_fd();
                // a valid root is not needed because we do not call any functions involving the full path
                // of the `DirEntry`s.
                let dummy_root = PathBuf::new();
                let inner = InnerReadDir { dirp, root: dummy_root };
                Ok((ReadDir::new(inner), new_parent_fd))
            }
        }
    }

    cfg_select_has_getdents! {
        => {
            fn is_dir(ent: &DirEntry) -> Option<bool> {
                match ent.inner.eager_dirent.eager_type() {
                    super::EagerFileType::Directory => Some(true),
                    super::EagerFileType::Unknown => None,
                    _ => Some(false),
                }
            }
        }
        any(
            target_os = "solaris",
            target_os = "illumos",
            target_os = "haiku",
            target_os = "vxworks",
            target_os = "aix",
        ) => {
            fn is_dir(_ent: &DirEntry) -> Option<bool> {
                None
            }
        }
        _ => {
            fn is_dir(ent: &DirEntry) -> Option<bool> {
                match ent.entry.d_type {
                    libc::DT_UNKNOWN => None,
                    libc::DT_DIR => Some(true),
                    _ => Some(false),
                }
            }
        }
    }

    fn is_enoent(result: &io::Result<()>) -> bool {
        if let Err(err) = result
            && matches!(err.raw_os_error(), Some(libc::ENOENT))
        {
            true
        } else {
            false
        }
    }

    fn remove_dir_all_recursive(parent_fd: Option<RawFd>, path: &CStr) -> io::Result<()> {
        // try opening as directory
        let fd = match openat_nofollow_dironly(parent_fd, &path) {
            Err(err) if matches!(err.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) => {
                // not a directory - don't traverse further
                // (for symlinks, older Linux kernels may return ELOOP instead of ENOTDIR)
                return match parent_fd {
                    // unlink...
                    Some(parent_fd) => {
                        cvt(unsafe { unlinkat(parent_fd, path.as_ptr(), 0) }).map(drop)
                    }
                    // ...unless this was supposed to be the deletion root directory
                    None => Err(err),
                };
            }
            result => result?,
        };

        // open the directory passing ownership of the fd
        let (dir, fd) = fdreaddir(fd)?;

        // For WASI all directory entries for this directory are read first
        // before any removal is done. This works around the fact that the
        // WASIp1 API for reading directories is not well-designed for handling
        // mutations between invocations of reading a directory. By reading all
        // the entries at once this ensures that, at least without concurrent
        // modifications, it should be possible to delete everything.
        #[cfg(target_os = "wasi")]
        let dir = dir.collect::<Vec<_>>();

        for child in dir {
            let child = child?;
            let child_name = child.name_cstr();
            // we need an inner try block, because if one of these
            // directories has already been deleted, then we need to
            // continue the loop, not return ok.
            let result: io::Result<()> = try {
                match is_dir(&child) {
                    Some(true) => {
                        remove_dir_all_recursive(Some(fd), child_name)?;
                    }
                    Some(false) => {
                        cvt(unsafe { unlinkat(fd, child_name.as_ptr(), 0) })?;
                    }
                    None => {
                        // POSIX specifies that calling unlink()/unlinkat(..., 0) on a directory can succeed
                        // if the process has the appropriate privileges. This however can causing orphaned
                        // directories requiring an fsck e.g. on Solaris and Illumos. So we try recursing
                        // into it first instead of trying to unlink() it.
                        remove_dir_all_recursive(Some(fd), child_name)?;
                    }
                }
            };
            if result.is_err() && !is_enoent(&result) {
                return result;
            }
        }

        // unlink the directory after removing its contents
        ignore_notfound(cvt(unsafe {
            unlinkat(parent_fd.unwrap_or(libc::AT_FDCWD), path.as_ptr(), libc::AT_REMOVEDIR)
        }))?;
        Ok(())
    }

    fn remove_dir_all_modern(p: &CStr) -> io::Result<()> {
        // We cannot just call remove_dir_all_recursive() here because that would not delete a passed
        // symlink. No need to worry about races, because remove_dir_all_recursive() does not recurse
        // into symlinks.
        let attr = lstat(p)?;
        if attr.file_type().is_symlink() {
            super::unlink(p)
        } else {
            remove_dir_all_recursive(None, &p)
        }
    }

    pub fn remove_dir_all(p: &Path) -> io::Result<()> {
        run_path_with_cstr(p, &remove_dir_all_modern)
    }
}
