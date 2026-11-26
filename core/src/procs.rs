//! Finding, closing and killing a game's processes, and guessing its main executable.
use crate::model::Game;
use std::path::{Path, PathBuf};

/// Folders a game's processes run from: install dir and the exe's folder.
pub fn game_dirs(g: &Game) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = g.install_dir.iter().filter(|d| !d.trim().is_empty()).map(PathBuf::from).collect();
    if let Some(p) = g.exe.as_deref().map(Path::new).and_then(Path::parent) {
        if !v.iter().any(|d| p.starts_with(d)) && p.is_dir() {
            v.push(p.to_path_buf());
        }
    }
    // Never treat a drive root or the Windows folder as a game folder.
    v.retain(|d| d.components().count() > 2 && !d.to_string_lossy().to_lowercase().contains(r"\windows\"));
    v
}

/// Processes (pid, image path) whose executable lives under one of `dirs`.
pub fn under(dirs: &[PathBuf]) -> Vec<(u32, PathBuf)> {
    let prefixes: Vec<String> =
        dirs.iter().map(|d| format!("{}\\", d.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase())).collect();
    if prefixes.is_empty() {
        return Vec::new();
    }
    let me = std::process::id();
    sys::processes()
        .into_iter()
        .filter(|(pid, p)| {
            let s = p.to_string_lossy().to_lowercase();
            *pid != me && prefixes.iter().any(|pre| s.starts_with(pre))
        })
        .collect()
}

/// Asks the processes' windows to close (like clicking X).
pub fn close(pids: &[u32]) -> usize {
    sys::close_windows(pids)
}

/// Terminates the processes; returns how many were killed.
pub fn kill(pids: &[u32]) -> usize {
    pids.iter().filter(|&&p| sys::terminate(p)).count()
}

/// Likely main executable in a game folder: the largest .exe that isn't a known helper.
pub fn main_exe(dir: &Path) -> Option<PathBuf> {
    const SKIP: [&str; 16] = [
        "crash", "unins", "setup", "launcher", "redist", "helper", "dxsetup", "prereq", "easyanticheat", "battleye",
        "be_service", "report", "cefprocess", "update", "vcredist", "dotnet",
    ];
    fn walk(dir: &Path, depth: u32, best: &mut Option<(u64, PathBuf)>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if depth > 0 && !["_commonredist", "redist", "directx", "support", "easyanticheat", "battleye"].contains(&n.as_str()) {
                    walk(&p, depth - 1, best);
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if SKIP.iter().any(|s| n.contains(s)) {
                    continue;
                }
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                if best.as_ref().is_none_or(|(b, _)| size > *b) {
                    *best = Some((size, p));
                }
            }
        }
    }
    let mut best = None;
    walk(dir, 3, &mut best);
    best.map(|(_, p)| p)
}

/// The game's executable: the configured one, else a guess inside its install folder.
pub fn game_exe(g: &Game) -> Option<PathBuf> {
    let exe = g.exe.as_deref().map(PathBuf::from).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) && p.is_file());
    exe.or_else(|| g.install_dir.as_deref().map(Path::new).filter(|d| d.is_dir()).and_then(main_exe))
}

/// A PE file's bitness and imported DLL names (lowercase).
pub fn pe_imports(path: &Path) -> Option<(bool, Vec<String>)> {
    let b = std::fs::read(path).ok()?;
    let u16_at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let pe = u32_at(0x3c)? as usize;
    if b.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let coff = pe + 4;
    let sections = u16_at(coff + 2)? as usize;
    let opt_size = u16_at(coff + 16)? as usize;
    let opt = coff + 20;
    let is64 = match u16_at(opt)? {
        0x20b => true,
        0x10b => false,
        _ => return None,
    };
    let dirs = opt + if is64 { 112 } else { 96 };
    let import_rva = u32_at(dirs + 8)? as usize;
    let sec_table = opt + opt_size;
    let to_off = |rva: usize| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = sec_table + i * 40;
            let (vsize, va, raw_size, raw) = (u32_at(s + 8)? as usize, u32_at(s + 12)? as usize, u32_at(s + 16)? as usize, u32_at(s + 20)? as usize);
            (rva >= va && rva < va + vsize.max(raw_size)).then(|| rva - va + raw)
        })
    };
    let mut names = Vec::new();
    if import_rva != 0 {
        let mut d = to_off(import_rva)?;
        for _ in 0..512 {
            let name_rva = u32_at(d + 12)? as usize;
            if name_rva == 0 {
                break;
            }
            if let Some(o) = to_off(name_rva) {
                let end = b[o..].iter().position(|&c| c == 0).map_or(b.len(), |e| o + e);
                names.push(String::from_utf8_lossy(&b[o..end]).to_lowercase());
            }
            d += 20;
        }
    }
    Some((is64, names))
}

#[cfg(windows)]
mod sys {
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
        TerminateProcess,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE};

    pub fn processes() -> Vec<(u32, PathBuf)> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                if let Some(p) = image_path(e.th32ProcessID) {
                    out.push((e.th32ProcessID, p));
                }
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        out
    }

    fn image_path(pid: u32) -> Option<PathBuf> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
            CloseHandle(h);
            ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
        }
    }

    pub fn terminate(pid: u32) -> bool {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h.is_null() {
                return false;
            }
            let ok = TerminateProcess(h, 1) != 0;
            CloseHandle(h);
            ok
        }
    }

    pub fn close_windows(pids: &[u32]) -> usize {
        struct Ctx<'a> {
            pids: &'a [u32],
            n: usize,
        }
        unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> windows_sys::core::BOOL {
            let ctx = unsafe { &mut *(lp as *mut Ctx) };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if ctx.pids.contains(&pid) && unsafe { IsWindowVisible(hwnd) } != 0 {
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
                ctx.n += 1;
            }
            1
        }
        let mut ctx = Ctx { pids, n: 0 };
        unsafe { EnumWindows(Some(each), &mut ctx as *mut Ctx as LPARAM) };
        ctx.n
    }
}

#[cfg(not(windows))]
mod sys {
    use std::path::PathBuf;
    pub fn processes() -> Vec<(u32, PathBuf)> {
        Vec::new()
    }
    pub fn terminate(_: u32) -> bool {
        false
    }
    pub fn close_windows(_: &[u32]) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_imports() {
        let exe = std::env::current_exe().unwrap();
        let (is64, names) = pe_imports(&exe).unwrap();
        assert_eq!(is64, cfg!(target_pointer_width = "64"));
        assert!(names.iter().any(|n| n == "kernel32.dll"), "{names:?}");
    }

    #[test]
    fn finds_own_process() {
        // This test binary runs from target/; a parent of it must not match our own pid.
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap().to_path_buf();
        assert!(under(&[dir]).iter().all(|(p, _)| *p != std::process::id()));
    }
}
