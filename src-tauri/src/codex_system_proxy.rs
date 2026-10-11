//! Windows 当前用户的系统 PAC 配置；不安装服务，不修改 VPN 网卡或路由。

use serde::{Deserialize, Serialize};

/// 只备份与代理相关的选项，关闭时按所有权比较后恢复。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SystemProxy {
    pub flags: u32,
    pub pac_url: Option<String>,
    pub server: Option<String>,
    pub bypass: Option<String>,
}

#[cfg(target_os = "windows")]
pub(crate) fn read() -> Result<SystemProxy, String> {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Networking::WinInet::*;

    // 1. 使用 WinINET 查询当前用户的 LAN 配置，与 Electron/WinHTTP 读取范围一致。
    let mut options = [INTERNET_PER_CONN_OPTIONW::default(); 4];
    for (option, kind) in options.iter_mut().zip([
        INTERNET_PER_CONN_FLAGS,
        INTERNET_PER_CONN_AUTOCONFIG_URL,
        INTERNET_PER_CONN_PROXY_SERVER,
        INTERNET_PER_CONN_PROXY_BYPASS,
    ]) {
        option.dwOption = kind;
    }
    let mut list = INTERNET_PER_CONN_OPTION_LISTW::default();
    list.dwSize = size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32;
    list.dwOptionCount = options.len() as u32;
    list.pOptions = options.as_mut_ptr();
    let mut length = list.dwSize;
    // 指针仅在同步 API 调用期间有效；返回的字符串按 WinINET 契约由 GlobalFree 释放。
    let success = unsafe {
        InternetQueryOptionW(
            ptr::null(),
            INTERNET_OPTION_PER_CONNECTION_OPTION,
            &mut list as *mut _ as *mut _,
            &mut length,
        )
    };
    let failure = std::io::Error::last_os_error();
    let flags = unsafe { options[0].Value.dwValue };
    let mut strings = Vec::new();
    for option in &options[1..] {
        let pointer = unsafe { option.Value.pszValue };
        let value = if pointer.is_null() {
            None
        } else {
            let mut length = 0;
            unsafe {
                while *pointer.add(length) != 0 {
                    length += 1;
                }
                let value = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length));
                GlobalFree(pointer as *mut _);
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
            }
        };
        strings.push(value);
    }
    if success == 0 {
        return Err(format!(
            "读取 Windows 系统代理失败 / Cannot read system proxy: {failure}"
        ));
    }
    Ok(SystemProxy {
        flags,
        pac_url: strings.remove(0),
        server: strings.remove(0),
        bypass: strings.remove(0),
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn write(settings: &SystemProxy) -> Result<(), String> {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::Networking::WinInet::*;

    // 1. 保持静态代理地址和绕过列表；仅切换 flags 与 PAC URL。
    let mut pac: Vec<u16> = settings
        .pac_url
        .as_deref()
        .unwrap_or("")
        .encode_utf16()
        .collect();
    pac.push(0);
    let mut options = [INTERNET_PER_CONN_OPTIONW::default(); 2];
    options[0].dwOption = INTERNET_PER_CONN_FLAGS;
    options[0].Value.dwValue = settings.flags;
    options[1].dwOption = INTERNET_PER_CONN_AUTOCONFIG_URL;
    options[1].Value.pszValue = pac.as_mut_ptr();
    let mut list = INTERNET_PER_CONN_OPTION_LISTW::default();
    list.dwSize = size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32;
    list.dwOptionCount = options.len() as u32;
    list.pOptions = options.as_mut_ptr();
    let success = unsafe {
        InternetSetOptionW(
            ptr::null(),
            INTERNET_OPTION_PER_CONNECTION_OPTION,
            &list as *const _ as *const _,
            list.dwSize,
        )
    };
    if success == 0 {
        return Err(format!(
            "设置 Windows 系统代理失败 / Cannot set system proxy: {}",
            std::io::Error::last_os_error()
        ));
    }

    // 2. 通知使用系统代理的应用刷新配置；不结束或重启 Codex。
    for option in [INTERNET_OPTION_SETTINGS_CHANGED, INTERNET_OPTION_REFRESH] {
        let notified = unsafe { InternetSetOptionW(ptr::null(), option, ptr::null(), 0) };
        if notified == 0 {
            return Err(format!(
                "刷新 Windows 系统代理失败 / Cannot refresh system proxy: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn read() -> Result<SystemProxy, String> {
    Err("Codex 桌面域名代理目前仅支持 Windows / Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn write(_: &SystemProxy) -> Result<(), String> {
    Err("Codex 桌面域名代理目前仅支持 Windows / Windows only".into())
}
