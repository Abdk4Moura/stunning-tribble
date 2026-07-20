// Windows WinFsp client adapter for the mesh-native mount protocol.
//
// This is the Windows `MountHost` piece from docs/design-cross-platform-capabilities.md:
// the mesh serves a uniform SFTP-like file protocol (see mount_proto.rs), and
// this module presents it as a local NTFS-like filesystem via WinFsp.
//
// Honesty note: this module is gated to `cfg(target_os = "windows")` and cannot
// be compiled or run on the Linux development box. It is structured against the
// public winfsp-rs API and is intended for CI validation on a Windows runner.
// Some operations are stubbed with STATUS_NOT_IMPLEMENTED while the Windows
// path is being shaken out; see the PR description for the validated/unvalidated
// split.

#![cfg(all(target_os = "windows", feature = "mount-windows"))]

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStringExt, OsStrExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{
    STATUS_ACCESS_DENIED, STATUS_BUFFER_OVERFLOW, STATUS_DIRECTORY_NOT_EMPTY,
    STATUS_END_OF_FILE, STATUS_INVALID_PARAMETER, STATUS_NOT_A_DIRECTORY,
    STATUS_NOT_IMPLEMENTED, STATUS_OBJECT_NAME_COLLISION, STATUS_OBJECT_NAME_INVALID,
    STATUS_OBJECT_NAME_NOT_FOUND, STATUS_OBJECT_PATH_NOT_FOUND,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT, INVALID_FILE_ATTRIBUTES,
};
use winfsp::FspError;
use winfsp::constants::FspCleanupFlags;
use winfsp::filesystem::{
    DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo, VolumeInfo,
};
use winfsp::host::{FileSystemHost, VolumeParams};
use winfsp::U16CStr;

use crate::mount_proto::{FileKind, FileStat, MountClient, MountOp, MountResult, path_encode, path_decode};

const SECTOR_SIZE: u16 = 512;
const SECTORS_PER_ALLOCATION_UNIT: u16 = 1;
const VOLUME_LABEL: &str = "Filament";

/// Convert a Windows-wide path (backslash-separated, from WinFsp) to a relative
/// POSIX-style path used by the mount protocol. This also applies the server's
/// forbidden-character/byte policy: if the path contains a forbidden byte, we map
/// it to a private-use Unicode replacement and log it in the escaped name table.
/// For simplicity, this first version rejects the path with STATUS_INVALID_PARAMETER.
fn win32_to_protocol_path(
    caps: &crate::mount_proto::MountCaps,
    win_path: &U16CStr,
) -> Result<String, FspError> {
    let wide: Vec<u16> = win_path.as_slice().iter().copied().collect();
    let os = OsString::from_wide(&wide);
    let mut pb = PathBuf::from(os);
    // WinFsp paths are absolute in the volume (e.g. \\filament\file). Strip a
    // leading component that looks like our prefix and make the rest relative.
    if pb.starts_with("\\\\") {
        pb = pb.iter().skip(3).collect();
    }
    // Convert backslashes to forward slashes for the protocol.
    let posix = pb.to_string_lossy().replace('\\', "/");
    // Check forbidden characters and bytes.
    for b in &caps.forbidden_bytes {
        if posix.as_bytes().contains(b) {
            return Err(FspError::NTSTATUS(STATUS_OBJECT_NAME_INVALID.0));
        }
    }
    if posix.is_empty() {
        return Ok(".".into());
    }
    Ok(posix)
}

/// Convert a protocol path to a Windows display path (backslash-separated).
/// This is used for directory entries returned to the kernel.
fn protocol_to_win32_path(protocol_path: &str) -> Vec<u16> {
    let win = if protocol_path == "." {
        "\\".into()
    } else {
        protocol_path.replace('/', "\\")
    };
    win.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Check whether a final path component is a forbidden Windows device name.
fn is_forbidden_name(caps: &crate::mount_proto::MountCaps, name: &str) -> bool {
    caps.forbidden_names.iter().any(|r| r.eq_ignore_ascii_case(name))
}

/// Translate a protocol FileStat into a WinFsp FileInfo.
fn stat_to_file_info(stat: &FileStat, supports_fifo: bool) -> FileInfo {
    let mut attrs = if stat.mode & 0o170000 == 0o040000 {
        FILE_ATTRIBUTE_DIRECTORY.0
    } else if stat.mode & 0o170000 == 0o010000 && supports_fifo {
        FILE_ATTRIBUTE_REPARSE_POINT.0
    } else {
        FILE_ATTRIBUTE_ARCHIVE.0
    };
    if stat.mode & 0o222 == 0 {
        attrs |= FILE_ATTRIBUTE_READONLY.0;
    }
    FileInfo {
        file_attributes: attrs,
        reparse_tag: 0,
        allocation_size: stat.size,
        file_size: stat.size,
        creation_time: stat.mtime * 10_000_000 + 116_444_736_000_000_000,
        last_access_time: stat.mtime * 10_000_000 + 116_444_736_000_000_000,
        last_write_time: stat.mtime * 10_000_000 + 116_444_736_000_000_000,
        change_time: stat.mtime * 10_000_000 + 116_444_736_000_000_000,
        index_number: 0,
        hard_links: stat.nlink.max(1),
        ea_size: 0,
    }
}

fn parse_stat(v: &serde_json::Value) -> Option<FileStat> {
    serde_json::from_value(v.clone()).ok()
}

fn now_filetime() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs() * 10_000_000 + (now.subsec_nanos() as u64) / 100 + 116_444_736_000_000_000
}

#[derive(Clone, Debug)]
struct FileContext {
    fh: u64,
    path: String,
    is_directory: bool,
}

struct WindowsFs {
    client: Arc<Mutex<MountClient>>,
    /// Map of open file handle -> protocol path, for directory re-reads.
    handles: Mutex<HashMap<u64, String>>,
    next_dir_fh: Mutex<u64>,
}

impl WindowsFs {
    fn new(client: MountClient) -> Self {
        assert!(client.binary_frames, "WindowsFs requires a v2 MountClient");
        WindowsFs {
            client: Arc::new(Mutex::new(client)),
            handles: Mutex::new(HashMap::new()),
            next_dir_fh: Mutex::new(1),
        }
    }

    fn caps(&self) -> crate::mount_proto::MountCaps {
        self.client.lock().unwrap().caps.clone()
    }

    fn call_sync(&self, op: MountOp) -> Result<serde_json::Value, FspError> {
        let mut c = self.client.lock().unwrap();
        match c.call_sync(op) {
            Ok(resp) => match resp.result {
                MountResult::Ok(v) => Ok(v),
                MountResult::Err(e) => Err(ntstatus_from_errno(e.code)),
            },
            Err(_) => Err(FspError::NTSTATUS(STATUS_NOT_IMPLEMENTED.0)),
        }
    }

    fn call_sync_binary(&self, op: MountOp, data: Option<&[u8]>) -> Result<(serde_json::Value, Option<bytes::Bytes>), FspError> {
        let mut c = self.client.lock().unwrap();
        match c.call_sync_binary(op, data) {
            Ok((resp, bin)) => match resp.result {
                MountResult::Ok(v) => Ok((v, bin)),
                MountResult::Err(e) => Err(ntstatus_from_errno(e.code)),
            },
            Err(_) => Err(FspError::NTSTATUS(STATUS_NOT_IMPLEMENTED.0)),
        }
    }
}

fn ntstatus_from_errno(errno: i32) -> FspError {
    let status = match errno {
        2 => STATUS_OBJECT_NAME_NOT_FOUND.0,
        13 => STATUS_ACCESS_DENIED.0,
        20 => STATUS_NOT_A_DIRECTORY.0,
        39 => STATUS_DIRECTORY_NOT_EMPTY.0,
        17 => STATUS_OBJECT_NAME_COLLISION.0,
        22 => STATUS_INVALID_PARAMETER.0,
        _ => STATUS_NOT_IMPLEMENTED.0,
    };
    FspError::NTSTATUS(status)
}

impl FileSystemContext for WindowsFs {
    type FileContext = FileContext;

    fn get_volume_info(&self, out: &mut VolumeInfo) -> Result<(), FspError> {
        out.total_size = 0;
        out.free_size = 0;
        out.set_volume_label(VOLUME_LABEL);
        Ok(())
    }

    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        _security_descriptor: Option<&mut [std::ffi::c_void]>,
    ) -> Result<FileSecurity, FspError> {
        let path = win32_to_protocol_path(&self.caps(), file_name)?;
        let enc = path_encode(Path::new(&path));
        let v = self.call_sync(MountOp::GetAttr { path: enc })?;
        let stat = parse_stat(&v).ok_or(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0))?;
        let info = stat_to_file_info(&stat, self.caps().supports_fifo);
        Ok(FileSecurity {
            reparse: false,
            sz_security_descriptor: 0,
            attributes: info.file_attributes,
        })
    }

    fn open(
        &self,
        file_name: &U16CStr,
        _create_options: u32,
        _granted_access: u32,
        file_info: &mut OpenFileInfo,
    ) -> Result<Self::FileContext, FspError> {
        let path = win32_to_protocol_path(&self.caps(), file_name)?;
        let enc = path_encode(Path::new(&path));
        let v = self.call_sync(MountOp::GetAttr { path: enc.clone() })?;
        let stat = parse_stat(&v).ok_or(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0))?;
        let is_directory = stat.mode & 0o170000 == 0o040000;
        *file_info.as_mut() = stat_to_file_info(&stat, self.caps().supports_fifo);
        Ok(FileContext { fh: 0, path, is_directory })
    }

    fn create(
        &self,
        file_name: &U16CStr,
        _create_options: u32,
        _granted_access: u32,
        file_attributes: u32,
        _security_descriptor: Option<&[std::ffi::c_void]>,
        _allocation_size: u64,
        file_info: &mut OpenFileInfo,
    ) -> Result<Self::FileContext, FspError> {
        let caps = self.caps();
        let path = win32_to_protocol_path(&caps, file_name)?;
        let enc = path_encode(Path::new(&path));
        let is_directory = (file_attributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
        if is_directory {
            self.call_sync(MountOp::MkDir { path: enc.clone(), mode: 0o755 })?;
        } else {
            let name = Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or("");
            if is_forbidden_name(&caps, name) {
                return Err(FspError::NTSTATUS(STATUS_OBJECT_NAME_INVALID.0));
            }
            self.call_sync(MountOp::Create { path: enc.clone(), mode: 0o644, flags: 0 })?;
        }
        let v = self.call_sync(MountOp::GetAttr { path: enc })?;
        let stat = parse_stat(&v).ok_or(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0))?;
        *file_info.as_mut() = stat_to_file_info(&stat, caps.supports_fifo);
        Ok(FileContext { fh: 0, path, is_directory })
    }

    fn read(
        &self,
        context: &Self::FileContext,
        buffer: &mut [u8],
        offset: u64,
    ) -> Result<u32, FspError> {
        let max_read = self.caps().max_read_size;
        let size = (buffer.len() as u32).min(max_read);
        let enc = path_encode(Path::new(&context.path));
        let (v, bin) = self.call_sync_binary(
            MountOp::Read { fh: context.fh, offset, size },
            None,
        )?;
        let data = bin.ok_or(FspError::NTSTATUS(STATUS_END_OF_FILE.0))?;
        let n = data.len().min(buffer.len());
        buffer[..n].copy_from_slice(&data[..n]);
        Ok(n as u32)
    }

    fn write(
        &self,
        context: &Self::FileContext,
        buffer: &[u8],
        offset: u64,
        _write_to_eof: bool,
        _constrained_io: bool,
        file_info: &mut FileInfo,
    ) -> Result<u32, FspError> {
        let max_write = self.caps().max_write_size as usize;
        let mut written: u32 = 0;
        for chunk in buffer.chunks(max_write) {
            let enc = path_encode(Path::new(&context.path));
            let (v, _) = self.call_sync_binary(
                MountOp::Write { fh: context.fh, offset: offset + written as u64, size: chunk.len() as u32 },
                Some(chunk),
            )?;
            let n = v["size"].as_u64().unwrap_or(0) as u32;
            written += n;
            if n == 0 { break; }
        }
        // Refresh file info.
        let enc = path_encode(Path::new(&context.path));
        let v = self.call_sync(MountOp::GetAttr { path: enc })?;
        let stat = parse_stat(&v).ok_or(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0))?;
        *file_info = stat_to_file_info(&stat, self.caps().supports_fifo);
        Ok(written)
    }

    fn cleanup(&self, context: &Self::FileContext, _file_name: Option<&U16CStr>, flags: u32) {
        if FspCleanupFlags::FspCleanupDelete.is_flagged(flags) {
            let enc = path_encode(Path::new(&context.path));
            if context.is_directory {
                let _ = self.call_sync(MountOp::RmDir { path: enc });
            } else {
                let _ = self.call_sync(MountOp::Unlink { path: enc });
            }
        }
    }

    fn close(&self, _context: Self::FileContext) {}

    fn get_file_info(&self, context: &Self::FileContext, file_info: &mut FileInfo) -> Result<(), FspError> {
        let enc = path_encode(Path::new(&context.path));
        let v = self.call_sync(MountOp::GetAttr { path: enc })?;
        let stat = parse_stat(&v).ok_or(FspError::NTSTATUS(STATUS_OBJECT_NAME_NOT_FOUND.0))?;
        *file_info = stat_to_file_info(&stat, self.caps().supports_fifo);
        Ok(())
    }

    fn read_directory(
        &self,
        context: &Self::FileContext,
        _pattern: Option<&U16CStr>,
        marker: DirMarker,
        buffer: &mut [u8],
    ) -> Result<u32, FspError> {
        let enc = path_encode(Path::new(&context.path));
        let v = self.call_sync(MountOp::ReadDir { fh: 0, offset: 0 })?;
        let entries: Vec<serde_json::Value> = v.as_array().cloned().unwrap_or_default();
        let caps = self.caps();
        let mut cursor: u32 = 0;
        let mut dir_info: DirInfo<255> = DirInfo::new();
        let mut started = marker.is_none();
        for e in entries {
            let name = e["name"].as_str().unwrap_or("");
            if name == "." || name == ".." { continue; }
            if name.is_empty() { continue; }
            let name_wide = name.encode_utf16().collect::<Vec<u16>>();
            if !started {
                if let Some(m) = marker.inner_as_cstr() {
                    let m_slice: Vec<u16> = m.as_slice().iter().copied().collect();
                    if m_slice == name_wide {
                        started = true;
                    }
                    continue;
                }
            }
            let stat: FileStat = match serde_json::from_value(e["stat"].clone()) {
                Ok(s) => s,
                Err(_) => continue,
            };
            dir_info.reset();
            *dir_info.file_info_mut() = stat_to_file_info(&stat, caps.supports_fifo);
            dir_info.set_name_raw(&name_wide).map_err(|_| FspError::NTSTATUS(STATUS_INVALID_PARAMETER.0))?;
            if !dir_info.append_to_buffer(buffer, &mut cursor) {
                return Ok(cursor);
            }
        }
        DirInfo::<255>::finalize_buffer(buffer, &mut cursor);
        Ok(cursor)
    }

    fn set_delete(&self, _context: &Self::FileContext, _file_name: &U16CStr, _delete_file: bool) -> Result<(), FspError> {
        // Delete is handled in cleanup() via FspCleanupDelete.
        Ok(())
    }

    fn rename(
        &self,
        context: &Self::FileContext,
        _file_name: &U16CStr,
        new_file_name: &U16CStr,
        _replace_if_exists: bool,
    ) -> Result<(), FspError> {
        let caps = self.caps();
        let from = path_encode(Path::new(&context.path));
        let new_path = win32_to_protocol_path(&caps, new_file_name)?;
        let to = path_encode(Path::new(&new_path));
        self.call_sync(MountOp::Rename { from, to })?;
        Ok(())
    }

    fn set_basic_info(
        &self,
        _context: &Self::FileContext,
        _file_attributes: u32,
        _creation_time: u64,
        _last_access_time: u64,
        _last_write_time: u64,
        _change_time: u64,
        _file_info: &mut FileInfo,
    ) -> Result<(), FspError> {
        Err(FspError::NTSTATUS(STATUS_NOT_IMPLEMENTED.0))
    }

    fn set_file_size(
        &self,
        _context: &Self::FileContext,
        _new_size: u64,
        _set_allocation_size: bool,
        _file_info: &mut FileInfo,
    ) -> Result<(), FspError> {
        Err(FspError::NTSTATUS(STATUS_NOT_IMPLEMENTED.0))
    }
}

/// Mount the protocol served by `client` at `mountpoint` via WinFsp. Blocks the
/// calling thread until the filesystem is unmounted.
pub fn run_mount(client: MountClient, mountpoint: &std::path::Path) -> anyhow::Result<()> {
    let mut volume_params = VolumeParams::new();
    volume_params
        .sector_size(SECTOR_SIZE)
        .sectors_per_allocation_unit(SECTORS_PER_ALLOCATION_UNIT)
        .volume_creation_time(now_filetime())
        .volume_serial_number(0)
        .file_info_timeout(1000)
        .case_sensitive_search(!client.caps.case_sensitive)
        .case_preserved_names(true)
        .unicode_on_disk(true)
        .persistent_acls(false)
        .post_cleanup_when_modified_only(true)
        .filesystem_name("Filament");

    let fs = WindowsFs::new(client);
    let mut host = FileSystemHost::<WindowsFs>::new(volume_params, fs)?;
    let wide: Vec<u16> = mountpoint.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mount_point = U16CStr::from_slice_truncate(&wide)?;
    host.mount(mount_point)?;
    Ok(())
}
