//! Windows DPAPI 密钥加密（清单 I-03 策略 A：Windows-only）。
//!
//! 平台策略：运行与发布仅支持 Windows 10/11 x64。本模块全部 Windows FFI
//! 均在 `#[cfg(target_os = "windows")]` 之下；非 Windows 编译由 lib.rs 顶层
//! `compile_error!` 给出明确的平台不支持提示（而非链接阶段找不到 crypt32 的
//! 晦涩错误），且本模块提供返回明确错误的桩实现保持模块解析干净。
//!
//! 安全纪律：
//! - unsafe 仅封装在本模块最小函数内，公开 API 不暴露裸指针；
//! - FFI 长度由 `usize` 经 `u32::try_from` 转换，超长输入返回错误；
//! - 成功返回后检查输出指针非空，所有成功分支恰好 `LocalFree` 一次；
//! - 任何路径不记录加密前/解密后的 Key 明文。

/// 用 Windows DPAPI 加密字节数据
pub fn encrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "windows")]
    {
        win::protect(data, true)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = data;
        Err(UNSUPPORTED_MSG.to_string())
    }
}

/// 用 Windows DPAPI 解密字节数据
pub fn decrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "windows")]
    {
        win::protect(data, false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = data;
        Err(UNSUPPORTED_MSG.to_string())
    }
}

/// 加密字符串 → Base64
pub fn encrypt_string(plaintext: &str) -> Result<String, String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    let encrypted = encrypt(plaintext.as_bytes())?;
    Ok(crate::base64::base64_encode(&encrypted))
}

/// Base64 → 解密字符串
pub fn decrypt_string(encoded: &str) -> Result<String, String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    let encrypted = crate::base64::base64_decode(encoded)?;
    let decrypted = decrypt(&encrypted)?;
    String::from_utf8(decrypted).map_err(|e| format!("UTF-8 decode error: {}", e))
}

#[cfg(not(target_os = "windows"))]
const UNSUPPORTED_MSG: &str =
    "DPAPI 仅在 Windows 平台可用：本项目仅支持 Windows（平台策略 A），不支持的平台无法加密/解密 API Key";

// ============================================================
// Windows 实现（I-03：FFI 全部收敛于此 cfg 块内）
// ============================================================

#[cfg(target_os = "windows")]
mod win {
    use std::ffi::c_void;

    // Windows DPAPI structures
    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            data_in: *const DataBlob,
            description: *const u16,
            optional_entropy: *const DataBlob,
            reserved: *const c_void,
            prompt_struct: *const c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;

        fn CryptUnprotectData(
            data_in: *const DataBlob,
            description: *mut *mut u16,
            optional_entropy: *const DataBlob,
            reserved: *const c_void,
            prompt_struct: *const c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;

        fn LocalFree(hmem: *mut c_void) -> *mut c_void;
    }

    /// DPAPI 加密（protect=true）/ 解密（protect=false）统一入口。
    ///
    /// I-03 加固：
    /// - 长度先经 `u32::try_from` 转换，超过 `u32::MAX` 的输入返回错误而非截断；
    /// - 成功返回后校验 `pb_data` 非空；
    /// - 所有成功分支恰好 `LocalFree` 一次（收敛在 `take_output`）。
    pub fn protect(data: &[u8], protect: bool) -> Result<Vec<u8>, String> {
        let cb_data = to_u32_len(data.len())?;
        let data_in = DataBlob {
            cb_data,
            pb_data: data.as_ptr() as *mut u8,
        };
        let mut data_out = DataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };

        let success = if protect {
            unsafe {
                CryptProtectData(
                    &data_in,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut data_out,
                )
            }
        } else {
            unsafe {
                CryptUnprotectData(
                    &data_in,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut data_out,
                )
            }
        };

        if success == 0 {
            return Err(if protect {
                "CryptProtectData failed".to_string()
            } else {
                "CryptUnprotectData failed".to_string()
            });
        }

        take_output(data_out)
    }

    /// 输入长度 usize → u32（FFI 要求），超长返回错误（可单测的边界部分）。
    fn to_u32_len(len: usize) -> Result<u32, String> {
        u32::try_from(len)
            .map_err(|_| "输入数据过长，超出 DPAPI 单次处理上限 (u32::MAX)".to_string())
    }

    /// 取出 DPAPI 输出：成功分支校验指针非空后拷贝，且恰好 `LocalFree` 一次。
    ///
    /// SAFETY: `data_out` 必须是 DPAPI 成功返回的 blob；`cb_data` 字节有效。
    fn take_output(data_out: DataBlob) -> Result<Vec<u8>, String> {
        if data_out.pb_data.is_null() {
            return Err("DPAPI 返回成功但输出指针为空".to_string());
        }
        let out =
            unsafe { std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize) }
                .to_vec();
        unsafe { LocalFree(data_out.pb_data as *mut c_void) };
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::to_u32_len;

        #[test]
        fn len_conversion_boundary() {
            // 可测试的长度转换边界：u32::MAX 恰好合法，+1 报错（不真实分配 4GB）
            assert_eq!(to_u32_len(u32::MAX as usize).unwrap(), u32::MAX);
            assert!(to_u32_len((u32::MAX as usize) + 1).is_err());
            assert_eq!(to_u32_len(0).unwrap(), 0);
        }
    }
}

// ============================================================
// 单元测试（依赖 Windows DPAPI，仅在 Windows 运行；CI 使用 Windows runner）
// 纪律：断言失败信息不得包含明文 secret
// ============================================================

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let original = "sk-test-api-key-12345";
        let encrypted = encrypt_string(original).unwrap();
        assert_ne!(encrypted, original, "密文不得等于明文");
        let decrypted = decrypt_string(&encrypted).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(encrypt_string("").unwrap(), "");
        assert_eq!(decrypt_string("").unwrap(), "");
    }

    #[test]
    fn test_invalid_base64_rejected() {
        // 非法 Base64 持久化串必须在解码阶段报错，不得触达 DPAPI
        let err = decrypt_string("!!!not-valid-base64!!!").unwrap_err();
        assert!(!err.contains("sk-"), "错误信息不得泄露密钥内容");
    }

    #[test]
    fn test_corrupted_dpapi_blob_rejected() {
        // 合法 Base64 但并非有效 DPAPI blob（随机字节）→ CryptUnprotectData 失败
        let garbage =
            crate::base64::base64_encode(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11, 0x22, 0x33]);
        assert!(decrypt_string(&garbage).is_err());
        assert!(decrypt(&[]).is_err(), "空 blob 应解密失败");
    }

    #[test]
    fn test_binary_roundtrip_including_non_utf8() {
        // 二进制层面 roundtrip：含非 UTF-8 字节也能原样返回
        let data: Vec<u8> = vec![0x00, 0xFF, 0x7F, 0x80, 0x01];
        let enc = encrypt(&data).unwrap();
        assert_ne!(enc, data);
        assert_eq!(decrypt(&enc).unwrap(), data);
    }
}
