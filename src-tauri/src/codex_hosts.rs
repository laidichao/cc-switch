//! 仅为 Codex 公共云 WebSocket 管理固定 hosts 映射，不接受任意文件或域名。

pub(crate) const CLOUD_HOST: &str = "codex-cloud-backend.chatgpt.com";
pub(crate) const CLOUD_ADDRESS: &str = "127.0.0.2";
const MARKER: &str = "# CCS Codex cloud ";

#[cfg(target_os = "windows")]
fn hosts_path() -> Result<std::path::PathBuf, String> {
    use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;
    // 使用系统 API 定位 hosts，提权进程不信任调用方的环境变量或路径参数。
    let mut buffer = [0u16; 32768];
    let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err("无法定位 Windows hosts / Cannot locate Windows hosts".into());
    }
    let directory = String::from_utf16_lossy(&buffer[..length as usize]);
    Ok(std::path::PathBuf::from(directory).join("System32/drivers/etc/hosts"))
}

fn block(owner: &str) -> String {
    format!("\r\n{MARKER}{owner} begin\r\n{CLOUD_ADDRESS} {CLOUD_HOST}\r\n::1 {CLOUD_HOST}\r\n{MARKER}{owner} end\r\n")
}

pub(crate) fn applied(owner: &str) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        // 只读检查本功能的完整映射，以及标记外是否有冲突条目。
        let path = hosts_path()?;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            // 缺少 hosts 表示映射未应用，状态检查不创建系统文件。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!("读取 hosts 失败（{}）: {error}", path.display()));
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let expected = block(owner);
        let outside = text.replace(&expected, "");
        let conflict = outside.lines().any(|line| {
            let content = line.split('#').next().unwrap_or("");
            let words = content.split_whitespace();
            let mut names = words.skip(1);
            names.any(|host| host.eq_ignore_ascii_case(CLOUD_HOST))
        });
        Ok(text.matches(&expected).count() == 1 && !conflict)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = owner;
        Ok(false)
    }
}

/// 在普通 CCS 进程中请求一次固定操作；只有辅助进程获得管理员权限。
pub(crate) fn change(owner: &str, install: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*, UI::Shell::*};
        // 1. 仅传操作与 UUID，不传认证、代理配置或可写路径。
        uuid::Uuid::parse_str(owner).map_err(|_| "Invalid hosts owner")?;
        // 普通进程与辅助进程分别解析固定系统路径，错误只说明目标文件。
        let path = hosts_path()?;
        if !install {
            // 权限拒绝或尚未写 hosts 的回滚不再弹 UAC；残缺标记仍进入恢复分支。
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                // 文件缺失时没有本功能映射可移除，允许继续恢复 PAC 和 Codex 设置。
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => {
                    return Err(format!(
                        "恢复 hosts 前读取失败（{}）: {error}",
                        path.display()
                    ));
                }
            };
            let text = String::from_utf8_lossy(&bytes);
            if !text.contains(&format!("{MARKER}{owner}")) {
                return Ok(());
            }
        }
        let executable = std::env::current_exe()
            .map_err(|error| format!("定位 CCS hosts 辅助程序失败: {error}"))?;
        let mut file: Vec<u16> = executable.as_os_str().encode_wide().collect();
        file.push(0);
        let action = if install { "install" } else { "remove" };
        let parent = std::process::id();
        let arguments = format!("--ccs-codex-hosts {action} {owner} {parent}");
        let mut parameters: Vec<u16> = arguments.encode_utf16().collect();
        parameters.push(0);
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let mut info = SHELLEXECUTEINFOW::default();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = parameters.as_ptr();
        info.nShow = 0;
        // 2. UAC 拒绝直接失败，不尝试未经授权写 hosts。
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            return Err(
                "未获得 hosts 管理权限 / Hosts administrator permission was not granted".into(),
            );
        }
        if info.hProcess.is_null() {
            return Err("无法确认 hosts 操作进程 / Cannot confirm hosts helper".into());
        }
        // 辅助操作只读写固定小文件；等待退出后才发布就绪状态或停止转发。
        let wait = unsafe { WaitForSingleObject(info.hProcess, u32::MAX) };
        let mut code = 1;
        let read = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
        unsafe { CloseHandle(info.hProcess) };
        if wait != 0 || read == 0 {
            return Err("无法确认 hosts 操作结果 / Cannot confirm hosts update".into());
        }
        match code {
            0 => Ok(()),
            10 => Err("hosts 中已有该域名或其他 CCS 标记，未覆盖 / Conflicting hosts entry was not overwritten".into()),
            11 => Err("hosts 编码、大小或标记异常，未修改 / Unsupported hosts format or marker".into()),
            12 => Err(format!("hosts 读写失败（{}），请检查文件目录和权限后重试 / Hosts update failed; check the directory and permissions", path.display())),
            13 => Err("hosts 已写入但 DNS 缓存刷新失败 / Hosts updated but DNS cache refresh failed".into()),
            _ => Err("hosts 辅助操作失败 / Hosts helper failed".into()),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (owner, install);
        Err("Hosts forwarding is only supported on Windows".into())
    }
}

/// 主入口先分派固定 hosts 操作，避免提权后初始化 GUI、数据库或任意用户配置。
pub(crate) fn helper_entry() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.get(1).map(String::as_str) != Some("--ccs-codex-hosts") {
        return;
    }
    if arguments.len() != 5 || uuid::Uuid::parse_str(&arguments[3]).is_err() {
        std::process::exit(1);
    }
    let install = match arguments[2].as_str() {
        "install" => true,
        "remove" => false,
        _ => std::process::exit(1),
    };
    let parent = match arguments[4].parse::<u32>() {
        Ok(value) if value != 0 => value,
        _ => std::process::exit(1),
    };
    #[cfg(target_os = "windows")]
    let process = {
        use windows_sys::Win32::{Foundation::WAIT_TIMEOUT, System::Threading::*};
        // UAC 停留期间 CCS 可能已被强制结束，此时不再安装新的映射。
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent) };
        if install
            && (handle.is_null() || unsafe { WaitForSingleObject(handle, 0) } != WAIT_TIMEOUT)
        {
            std::process::exit(1);
        }
        handle
    };
    #[cfg(not(target_os = "windows"))]
    let _ = parent;
    // 不输出文件正文或配置，父进程仅按退出码给出固定错误信息。
    let mut result = edit_hosts(&arguments[3], install);
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_TIMEOUT},
            System::Threading::WaitForSingleObject,
        };
        if install && unsafe { WaitForSingleObject(process, 0) } != WAIT_TIMEOUT {
            // 写入期间父进程退出则立即撤销该完整标记块，避免晚到的 UAC 写入残留。
            let _ = edit_hosts(&arguments[3], false);
            result = Err(1);
        }
        if !process.is_null() {
            unsafe { CloseHandle(process) };
        }
    }
    std::process::exit(result.err().unwrap_or(0));
}

#[cfg(target_os = "windows")]
fn edit_hosts(owner: &str, install: bool) -> Result<(), i32> {
    use std::{
        fs::OpenOptions,
        io::{Read, Seek, SeekFrom, Write},
        os::windows::fs::OpenOptionsExt,
    };
    // Windows 系统 DNS 缓存入口未由 windows-sys 导出，直接链接已有系统库。
    #[link(name = "dnsapi")]
    unsafe extern "system" {
        fn DnsFlushResolverCache() -> i32;
    }
    // 1. 固定目标并在同一文件句柄上读改写；拒绝并发写入，保留文件 ACL。
    let path = hosts_path().map_err(|_| 12)?;
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    options.share_mode(1);
    // 只有通过 UAC 的安装操作可创建缺失文件；已有内容不截断，恢复不创建文件。
    options.create(install);
    let mut file = match options.open(&path) {
        Ok(file) => file,
        // 普通进程检查后文件仍可能被删除，辅助恢复同样按无映射处理。
        Err(error) if !install && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(12),
    };
    if file.metadata().map_err(|_| 12)?.len() > 4 * 1024 * 1024 {
        return Err(11);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| 12)?;
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Err(11);
    }
    // 2. 按字节保存无关条目和编码，只增删本次完整标记块。
    let expected = block(owner);
    let marker = expected.as_bytes();
    let windows = bytes.windows(marker.len());
    let mut matches = Vec::new();
    for (index, window) in windows.enumerate() {
        if window == marker {
            matches.push(index);
        }
    }
    if matches.len() > 1 {
        return Err(11);
    }
    let mut updated = bytes.clone();
    if let Some(index) = matches.first() {
        updated.drain(*index..*index + marker.len());
    } else {
        let text = String::from_utf8_lossy(&bytes);
        if text.contains(&format!("{MARKER}{owner}")) {
            return Err(11);
        }
    }
    if install {
        let text = String::from_utf8_lossy(&updated);
        let conflict = text.lines().any(|line| {
            let content = line.split('#').next().unwrap_or("");
            let words = content.split_whitespace();
            let mut names = words.skip(1);
            names.any(|host| host.eq_ignore_ascii_case(CLOUD_HOST))
        });
        if conflict || text.contains(MARKER) {
            return Err(10);
        }
        updated.extend_from_slice(marker);
    }
    if updated != bytes {
        // 3. 写入失败尽力回写原始字节；恢复记录由父进程保留以便重试。
        let result = (|| {
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&updated)?;
            file.set_len(updated.len() as u64)?;
            file.sync_all()
        })();
        if result.is_err() {
            let _ = file.seek(SeekFrom::Start(0));
            let _ = file.write_all(&bytes);
            let _ = file.set_len(bytes.len() as u64);
            let _ = file.sync_all();
            return Err(12);
        }
    }
    drop(file);
    // 即使是幂等重试也刷新缓存，避免上次刷新失败仍保留旧的地址。
    if unsafe { DnsFlushResolverCache() } == 0 {
        return Err(13);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn edit_hosts(_owner: &str, _install: bool) -> Result<(), i32> {
    Err(1)
}
