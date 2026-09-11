// build.rs — tauri_build + 捆绑 ffmpeg.exe 完整性门禁（integrity gate）
//
// 门禁语义 / gate semantics:
//   1. resources/ffmpeg.exe 缺失        -> cargo:warning 输出获取指引并放行
//                                          （干净 checkout / CI 不应因此红）
//   2. exe 存在但 sha256 基准文件缺失    -> panic! 硬失败（配置错误）
//   3. exe 存在且哈希与基准不匹配        -> panic! 硬失败（文件损坏或被篡改）
//
// 注意：第 1 条的「缺失放行」仅指本编译期门禁；`tauri build` 打包阶段对
// bundle.resources 声明的缺失文件仍会硬失败。ffmpeg.exe 已入库，正常
// checkout 不受影响。
//
// 约束：不新增任何 Cargo 依赖，SHA-256 为构建期内联实现，不进运行时二进制。

use std::fs;
use std::io::Read;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=resources/ffmpeg.exe");
    println!("cargo:rerun-if-changed=resources/ffmpeg.exe.sha256");
    verify_ffmpeg();
    tauri_build::build();
    // T-09：给测试目标补 comctl32 v6 manifest（测试 exe 拿不到 tauri-build 的
    // 资源对象，其 embed-resource 只发 `cargo:rustc-link-arg-bins`）。
    // 注意不能再用“往所有目标塞一个 windres 资源对象”的做法：MSVC 下那个对象的
    // .rsrc 会顶掉 tauri 资源对象里的图标与版本信息，产物 exe 只剩 manifest
    // （CI 安装包图标丢失的根因）。分流细节见函数头注释。
    embed_test_common_controls_v6_manifest();
}

/// T-09：给测试目标嵌入 Microsoft.Windows.Common-Controls 6.0 manifest，
/// 使测试 exe 激活 WinSxS 的 comctl32 v6（否则静态导入的 TaskDialogIndirect
/// 在系统 v5.82 上不存在 —— 已实测 System32\comctl32.dll 只导出
/// SetWindowSubclass 等，不导出 TaskDialogIndirect —— 测试 exe 以 0xc0000139 退出）。
///
/// 主 bin 不需要这份：`tauri_build::build()` 的默认 `WindowsAttributes::new()`
/// 已把同样内容的 comctl32 v6 manifest 与图标、版本信息编进同一个资源对象。
///
/// 两条工具链路径必须分开，实测结论（MSVC link.exe / GNU ld，均为本机实测）：
///   - MSVC：链接输入里若有“对象文件的 .rsrc”，资源管理段会整体覆盖从 .lib 里
///     取出的资源成员 —— 图标和 VERSION 全部丢失。因此这里不用 windres 对象，
///     而是用链接器自带的 manifest 生成（不产生第二个 .rsrc），并对 bin 关掉生成
///     （bin 已有 tauri 的 manifest，重复会 CVT1100 资源重复而链接失败）。
///   - GNU：ld 会合并 .rsrc，所以沿用 windres 对象，本地 dev/release-build.bat 不受影响。
#[cfg(target_os = "windows")]
fn embed_test_common_controls_v6_manifest() {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    const COMCTL6_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_path = out_dir.join("common_controls_v6.manifest");

    if fs::write(&manifest_path, COMCTL6_MANIFEST).is_err() {
        eprintln!(
            "[build.rs] 无法写入 comctl32 v6 manifest 临时文件；测试 exe 可能以 0xc0000139 退出"
        );
        return;
    }

    // CARGO_CFG_TARGET_ENV 由 cargo 按“目标”而非宿主设置，交叉编译也正确。
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
            manifest_path.display()
        );
        // bin 侧：tauri-build 的资源对象已经带了 manifest（RT_MANIFEST id 1）。
        // 再让链接器生成一个会因资源重复直接失败（CVT1100），故关闭生成；
        // link.exe 会提示 LNK4075「/MANIFESTINPUT 被 /MANIFEST:NO 忽略」，属预期。
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
        return;
    }

    let rc_path = out_dir.join("common_controls_v6.rc");
    let res_path = out_dir.join("common_controls_v6_res.o");
    if fs::write(&rc_path, "1 24 \"common_controls_v6.manifest\"\n").is_err() {
        eprintln!(
            "[build.rs] 无法写入 comctl32 v6 manifest 临时文件；测试 exe 可能以 0xc0000139 退出"
        );
        return;
    }

    let status = Command::new("windres")
        .arg("-i")
        .arg(&rc_path)
        .arg("-o")
        .arg(&res_path)
        .status();
    let ok = match status {
        Ok(s) => s.success(),
        Err(_) => false,
    };
    if ok {
        // 注意：这条对所有链接目标生效（含 bin）。GNU ld 会合并 .rsrc，所以 bin 的
        // 图标/版本不受影响；MSVC 请勿走这条路（见函数头注释），上面已按目标环境分流。
        println!("cargo:rustc-link-arg={}", res_path.display());
    } else {
        eprintln!(
            "[build.rs] windres 编译 comctl32 v6 manifest 失败；测试 exe 可能以 0xc0000139 退出"
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn embed_test_common_controls_v6_manifest() {}

fn verify_ffmpeg() {
    // KAT 自检：先确认内联 SHA-256 实现本身正确
    let mut kat = Sha256::new();
    kat.update(b"abc");
    let kat_hex: String = kat.finalize().iter().map(|b| format!("{b:02X}")).collect();
    assert_eq!(
        kat_hex, "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
        "internal SHA-256 KAT failed"
    );

    let exe = Path::new("resources/ffmpeg.exe");
    let sum_file = Path::new("resources/ffmpeg.exe.sha256");

    // 发布门禁开关：设置 MIMO_FFMPEG_REQUIRE=1 后，exe 缺失也硬失败。
    // 普通源码检查（cargo check/test/clippy）保持放行，Tauri build/CI 发布 job 设置该变量。
    let require_exe = std::env::var_os("MIMO_FFMPEG_REQUIRE").is_some();

    // (a) exe 缺失：普通源码检查警告 + 放行；发布门禁硬失败
    if !exe.is_file() {
        if require_exe {
            panic!(
                "resources/ffmpeg.exe not found while MIMO_FFMPEG_REQUIRE is set (release gate).\n\
                 ffmpeg.exe 缺失且已设置 MIMO_FFMPEG_REQUIRE（发布门禁），构建必须硬失败。"
            );
        }
        println!("cargo:warning=ffmpeg.exe not found at src-tauri/resources/ffmpeg.exe — integrity check skipped.");
        println!("cargo:warning=ffmpeg.exe 未找到：请获取官方捆绑的精简版 ffmpeg.exe（16kHz/mono/16bit WAV 提取专用）放入 src-tauri/resources/ 后重新构建。");
        return;
    }

    // (b) 有 exe 无基准 = 配置错误，硬失败
    let expected = match read_expected_hash(sum_file) {
        Ok(h) => h,
        Err(e) => panic!(
            "resources/ffmpeg.exe.sha256 baseline missing or invalid: {e}\n\
             存在 ffmpeg.exe 却缺少/无法解析 sha256 基准文件，属于配置错误，请补齐基准文件。"
        ),
    };

    let actual = match sha256_file(exe) {
        Ok(h) => h,
        Err(e) => panic!("failed to compute SHA-256 of resources/ffmpeg.exe: {e}"),
    };

    // (c) 哈希不匹配：硬失败，附期望值与实际值
    if expected != actual {
        panic!(
            "ffmpeg.exe SHA-256 mismatch!\n  expected: {expected}\n  actual:   {actual}\n\
             文件已损坏或被篡改，请恢复与 ffmpeg.exe.sha256 匹配的官方捆绑版本。"
        );
    }
}

/// 读取 sha256 基准文件，取第一个 64 位十六进制 token（忽略大小写、首尾空白与可能的文件名后缀）。
fn read_expected_hash(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("file not found: {}", path.display()));
    }
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    extract_hex64(&text)
        .map(|t| t.to_ascii_uppercase())
        .ok_or_else(|| format!("no 64-hex-digit SHA-256 token found in {}", path.display()))
}

/// 扫描文本，返回第一个长度恰为 64 的连续十六进制串。
fn extract_hex64(text: &str) -> Option<String> {
    let mut run = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_hexdigit() {
            run.push(ch);
        } else if !run.is_empty() {
            if run.len() == 64 {
                return Some(run);
            }
            run.clear();
        }
    }
    None
}

/// 流式计算文件 SHA-256，返回大写十六进制字符串。
fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read error on {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02X}")).collect())
}

// ---- 内联纯 Rust SHA-256 实现（仅构建期使用） ----

struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Sha256 {
            state: [
                0x6A09_E667,
                0xBB67_AE85,
                0x3C6E_F372,
                0xA54F_F53A,
                0x510E_527F,
                0x9B05_688C,
                0x1F83_D9AB,
                0x5BE0_CD19,
            ],
            buf: [0; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u64;
        // 先把内部缓冲区填满一个完整块
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        // 整块压缩
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        // 尾部余数追加到缓冲区（保留已有缓冲内容）
        self.buf[self.buf_len..self.buf_len + data.len()].copy_from_slice(data);
        self.buf_len += data.len();
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        let pad = if self.buf_len <= 56 {
            56 - self.buf_len
        } else {
            64 + 56 - self.buf_len
        };
        self.update(&vec![0u8; pad]);
        self.update(&bit_len.to_be_bytes());
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a_2f98,
            0x7137_4491,
            0xb5c0_fbcf,
            0xe9b5_dba5,
            0x3956_c25b,
            0x59f1_11f1,
            0x923f_82a4,
            0xab1c_5ed5,
            0xd807_aa98,
            0x1283_5b01,
            0x2431_85be,
            0x550c_7dc3,
            0x72be_5d74,
            0x80de_b1fe,
            0x9bdc_06a7,
            0xc19b_f174,
            0xe49b_69c1,
            0xefbe_4786,
            0x0fc1_9dc6,
            0x240c_a1cc,
            0x2de9_2c6f,
            0x4a74_84aa,
            0x5cb0_a9dc,
            0x76f9_88da,
            0x983e_5152,
            0xa831_c66d,
            0xb003_27c8,
            0xbf59_7fc7,
            0xc6e0_0bf3,
            0xd5a7_9147,
            0x06ca_6351,
            0x1429_2967,
            0x27b7_0a85,
            0x2e1b_2138,
            0x4d2c_6dfc,
            0x5338_0d13,
            0x650a_7354,
            0x766a_0abb,
            0x81c2_c92e,
            0x9272_2c85,
            0xa2bf_e8a1,
            0xa81a_664b,
            0xc24b_8b70,
            0xc76c_51a3,
            0xd192_e819,
            0xd699_0624,
            0xf40e_3585,
            0x106a_a070,
            0x19a4_c116,
            0x1e37_6c08,
            0x2748_774c,
            0x34b0_bcb5,
            0x391c_0cb3,
            0x4ed8_aa4a,
            0x5b9c_ca4f,
            0x682e_6ff3,
            0x748f_82ee,
            0x78a5_636f,
            0x84c8_7814,
            0x8cc7_0208,
            0x90be_fffa,
            0xa450_6ceb,
            0xbef9_a3f7,
            0xc671_78f2,
        ];
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            self.state[0],
            self.state[1],
            self.state[2],
            self.state[3],
            self.state[4],
            self.state[5],
            self.state[6],
            self.state[7],
        );
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, val) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(val);
        }
    }
}
