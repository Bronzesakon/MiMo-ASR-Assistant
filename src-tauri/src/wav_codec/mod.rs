// wav_codec/mod.rs — WAV 切片与解析（纯字节操作，零外部解码依赖）
//
// 转码（任意容器 → 16kHz/mono/16bit WAV）由捆绑 ffmpeg.exe 完成（见 ffmpeg_exe.rs），
// 本模块只保留 WAV 域能力（B-11 更名：ffmpeg → wav_codec，FfmpegError → WavError）：
//   1. slice_wav：按时间区间切割 WAV（16bit 快速路径 + 通用慢速路径）。
//   2. get_duration：基于 fmt/data chunk 的精确时长计算（禁止文件大小估算）。
//   3. is_target_wav：RIFF/WAVE 魔数 + fmt 直读判定 16kHz/mono/16bit PCM。
//
// 零 panic 纪律：全 Result、索引先查长度、任何解析失败优雅返回。

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

/// WAV 域处理错误（B-07：thiserror 统一错误体系，Display 文案不变）
#[derive(Debug, thiserror::Error)]
pub enum WavError {
    #[error("输入文件不存在: {0}")]
    InputNotFound(String),
    #[error("输出目录不存在: {0}")]
    OutputDirNotFound(String),
    #[error("音频处理失败: {0}")]
    Processing(String),
    #[error("无效输出: {0}")]
    InvalidOutput(String),
}

// ============================================================
// WAV 头部解析（fmt + data chunk 直读）
// ============================================================

/// WAV fmt/data 关键参数
struct WavFmt {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    byte_rate: u32,
    bits_per_sample: u16,
    data_size: u64,
}

/// 扫描 RIFF/WAVE chunk，直读 fmt 与 data。任何结构异常返回 InvalidOutput。
///
/// 防御点：chunk 头按「4+4」分步读取并容忍 EOF；fmt 体先查长度再索引；
/// chunk 尺寸异常（过大/奇数对齐）用饱和运算跳过，不 panic。
fn parse_wav_chunks<R: Read + Seek>(reader: &mut R) -> Result<WavFmt, WavError> {
    reader.seek(SeekFrom::Start(0)).map_err(io_err)?;

    let mut riff = [0u8; 4];
    reader.read_exact(&mut riff).map_err(io_err)?;
    if &riff != b"RIFF" {
        return Err(WavError::InvalidOutput(
            "不是有效的 WAV 文件（缺少 RIFF 魔数）".to_string(),
        ));
    }
    let mut riff_size_buf = [0u8; 4];
    reader.read_exact(&mut riff_size_buf).map_err(io_err)?;
    let riff_size = u32::from_le_bytes(riff_size_buf) as u64;
    let mut wave = [0u8; 4];
    reader.read_exact(&mut wave).map_err(io_err)?;
    if &wave != b"WAVE" {
        return Err(WavError::InvalidOutput(
            "不是有效的 WAV 文件（缺少 WAVE 魔数）".to_string(),
        ));
    }

    // RIFF 容器边界：riff_size 字段语义为「size 字段之后的字节数」，
    // 故容器结尾 = 8（RIFF 魔数 + size 字段本身）+ riff_size
    let container_end = 8u64.saturating_add(riff_size);

    let mut fmt: Option<(u16, u16, u32, u32, u16)> = None;
    let mut data_size: Option<u64> = None;
    let mut iterations = 0usize;

    loop {
        iterations += 1;
        if iterations > 4096 {
            return Err(WavError::InvalidOutput("WAV chunk 数量异常".to_string()));
        }

        // chunk id（4 字节）：EOF 收尾
        let mut id = [0u8; 4];
        match reader.read_exact(&mut id) {
            Ok(_) => {}
            Err(_) => break,
        }
        // chunk size（4 字节）：EOF 收尾
        let mut size_buf = [0u8; 4];
        match reader.read_exact(&mut size_buf) {
            Ok(_) => {}
            Err(_) => break,
        }
        let chunk_size = u32::from_le_bytes(size_buf) as u64;

        if &id == b"fmt " {
            // fmt 体最小 16 字节：audio_format(2) channels(2) sample_rate(4) byte_rate(4) block_align(2) bits(2)
            if (16..=4096).contains(&chunk_size) {
                let mut body = vec![0u8; chunk_size as usize];
                if reader.read_exact(&mut body).is_ok() && body.len() >= 16 {
                    fmt = Some((
                        u16::from_le_bytes([body[0], body[1]]),
                        u16::from_le_bytes([body[2], body[3]]),
                        u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
                        u32::from_le_bytes([body[8], body[9], body[10], body[11]]),
                        u16::from_le_bytes([body[14], body[15]]),
                    ));
                }
            } else {
                // 尺寸异常，按偶对齐跳过 chunk 体
                let skip = chunk_size.saturating_add(chunk_size % 2);
                if reader
                    .seek(SeekFrom::Current(skip.min(i64::MAX as u64) as i64))
                    .is_err()
                {
                    break;
                }
            }
        } else if &id == b"data" {
            data_size = Some(chunk_size);
            break;
        } else {
            let skip = chunk_size.saturating_add(chunk_size % 2);
            if reader
                .seek(SeekFrom::Current(skip.min(i64::MAX as u64) as i64))
                .is_err()
            {
                break;
            }
        }

        // 越过 RIFF 声明边界即停（防伪造大尺寸 chunk 无限扫描）
        match reader.stream_position() {
            Ok(pos) if pos >= container_end => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let (audio_format, channels, sample_rate, byte_rate, bits_per_sample) =
        fmt.ok_or_else(|| WavError::InvalidOutput("WAV 文件中未找到 fmt 块".to_string()))?;
    let data_size =
        data_size.ok_or_else(|| WavError::InvalidOutput("WAV 文件中未找到 data 块".to_string()))?;

    Ok(WavFmt {
        audio_format,
        channels,
        sample_rate,
        byte_rate,
        bits_per_sample,
        data_size,
    })
}

// ============================================================
// 公开 API
// ============================================================

/// 判定文件是否已是目标格式（RIFF/WAVE + PCM + 16kHz / mono / 16bit）。
///
/// 纯字节解析、零依赖；任何解析失败（含截断文件、畸形 chunk）一律返回 false。
pub fn is_target_wav(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    match parse_wav_chunks(&mut file) {
        Ok(fmt) => {
            fmt.audio_format == 1
                && fmt.channels == 1
                && fmt.sample_rate == 16000
                && fmt.bits_per_sample == 16
        }
        Err(_) => false,
    }
}

/// 获取 WAV 音频时长（秒）：data_len / bytes_per_frame / sample_rate。
///
/// fmt 优先使用 byte_rate；byte_rate 异常时按 channels * (bits/8) * rate 折算。
/// 非 WAV / 解析失败一律返回错误，不做文件大小估算。
pub fn get_duration(path: &Path) -> Result<f64, WavError> {
    if !path.exists() {
        return Err(WavError::InputNotFound(path.display().to_string()));
    }

    let mut file =
        File::open(path).map_err(|e| WavError::Processing(format!("打开文件失败: {}", e)))?;
    let fmt = parse_wav_chunks(&mut file)?;

    if fmt.sample_rate == 0 || fmt.channels == 0 || fmt.bits_per_sample == 0 {
        return Err(WavError::InvalidOutput(
            "WAV fmt 参数异常（采样率/声道数/位深为 0）".to_string(),
        ));
    }

    let bytes_per_frame = (fmt.channels as u64) * ((fmt.bits_per_sample as u64).div_ceil(8));
    let byte_rate = if fmt.byte_rate > 0 {
        fmt.byte_rate as u64
    } else {
        bytes_per_frame * (fmt.sample_rate as u64)
    };
    if byte_rate == 0 {
        return Err(WavError::InvalidOutput(
            "WAV byte_rate 无效，无法计算时长".to_string(),
        ));
    }

    let duration = fmt.data_size as f64 / byte_rate as f64;
    if duration.is_finite() {
        Ok(duration)
    } else {
        Err(WavError::InvalidOutput("WAV 时长计算结果异常".to_string()))
    }
}

/// 切割 WAV 文件
pub fn slice_wav(
    input_path: &Path,
    output_dir: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<std::path::PathBuf, WavError> {
    if !input_path.exists() {
        return Err(WavError::InputNotFound(input_path.display().to_string()));
    }
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir)
            .map_err(|_| WavError::OutputDirNotFound(output_dir.display().to_string()))?;
    }

    let output_filename = format!(
        "slice_{:.1}_{:.1}.wav",
        start_seconds,
        start_seconds + duration_seconds
    );
    let output_path = output_dir.join(&output_filename);

    let mut reader = WavReader::open(input_path)
        .map_err(|e| WavError::Processing(format!("打开 WAV 失败: {}", e)))?;
    let spec = reader.spec();

    let sample_rate = spec.sample_rate as f64;
    let start_sample = (start_seconds * sample_rate) as u32;
    let num_samples = (duration_seconds * sample_rate) as u32;

    // 快速路径：16-bit PCM → seek + 批量字节读写
    if spec.bits_per_sample == 16 && spec.sample_format == SampleFormat::Int {
        let channels: u64 = spec.channels as u64;
        let byte_offset = (start_sample as u64) * 2 * channels;
        let data_size = (num_samples as u64) * 2 * channels;

        let mut inner = reader.into_inner();
        inner.seek(SeekFrom::Start(0)).map_err(io_err)?;
        let header = read_wav_data_offset(&mut inner)?;
        inner
            .seek(SeekFrom::Start(header.data_offset + byte_offset))
            .map_err(io_err)?;

        // 读到实际可读字节数优雅结束：EOF 提前到达时按实际长度写出，
        // 与慢速路径（迭代器耗尽即 break）行为一致，不硬报 IO 错误。
        let want = data_size.min(u32::MAX as u64) as usize;
        let mut pcm_buf = vec![0u8; want];
        let mut filled = 0usize;
        while filled < want {
            match inner.read(&mut pcm_buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => return Err(io_err(e)),
            }
        }
        pcm_buf.truncate(filled);

        let out_spec = WavSpec {
            channels: spec.channels,
            sample_rate: spec.sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let data_size = pcm_buf.len() as u32;
        let out_file = File::create(&output_path)
            .map_err(|e| WavError::Processing(format!("创建输出失败: {}", e)))?;
        let mut out = BufWriter::new(out_file);
        // 直接写 WAV 头 + dump PCM 字节（跳过 hound 逐采样写入）
        write_wav_header(&mut out, &out_spec, data_size).map_err(io_err)?;
        out.write_all(&pcm_buf).map_err(io_err)?;
        out.flush().map_err(io_err)?;
        return Ok(output_path);
    }

    // 通用路径
    let mut writer = WavWriter::create(&output_path, spec)
        .map_err(|e| WavError::Processing(format!("创建 WAV 失败: {}", e)))?;
    let mut samples_iter = reader.samples::<i16>();
    let skip = start_sample as u64 * spec.channels as u64;
    for _ in 0..skip {
        let _ = samples_iter.next();
    }
    let count = num_samples as u64 * spec.channels as u64;
    for _ in 0..count {
        match samples_iter.next() {
            Some(Ok(s)) => writer.write_sample(s).map_err(hound_err)?,
            _ => break,
        }
    }
    writer.finalize().map_err(hound_err)?;
    Ok(output_path)
}

// ============================================================
// 内部辅助
// ============================================================

struct WavDataInfo {
    data_offset: u64,
}

fn read_wav_data_offset<R: Read + Seek>(reader: &mut R) -> Result<WavDataInfo, WavError> {
    reader.seek(SeekFrom::Start(0)).map_err(io_err)?;
    let mut buf4 = [0u8; 4];
    reader.read_exact(&mut buf4).map_err(io_err)?;
    if &buf4 != b"RIFF" {
        return Err(WavError::InvalidOutput("不是有效的 WAV 文件".to_string()));
    }
    reader.read_exact(&mut buf4).map_err(io_err)?;
    reader.read_exact(&mut buf4).map_err(io_err)?;
    if &buf4 != b"WAVE" {
        return Err(WavError::InvalidOutput("不是有效的 WAV 文件".to_string()));
    }

    loop {
        let mut chunk_id = [0u8; 4];
        match reader.read_exact(&mut chunk_id) {
            Ok(_) => {}
            Err(_) => break,
        }
        let mut size_buf = [0u8; 4];
        reader.read_exact(&mut size_buf).map_err(io_err)?;
        let chunk_size = u32::from_le_bytes(size_buf) as u64;

        if &chunk_id == b"data" {
            return Ok(WavDataInfo {
                data_offset: reader.stream_position().map_err(io_err)?,
            });
        }

        let skip = chunk_size.saturating_add(chunk_size % 2);
        reader
            .seek(SeekFrom::Current(skip.min(i64::MAX as u64) as i64))
            .map_err(io_err)?;
    }

    Err(WavError::InvalidOutput(
        "WAV 文件中未找到 data 块".to_string(),
    ))
}

fn io_err(e: std::io::Error) -> WavError {
    WavError::Processing(e.to_string())
}

fn hound_err(e: hound::Error) -> WavError {
    WavError::Processing(e.to_string())
}

/// 直接写 WAV 文件头（44 bytes），跳过 hound 逐采样写入
fn write_wav_header<W: std::io::Write>(
    w: &mut W,
    spec: &WavSpec,
    data_size: u32,
) -> std::io::Result<()> {
    let byte_rate = spec.sample_rate * spec.channels as u32 * (spec.bits_per_sample / 8) as u32;
    let block_align = spec.channels * (spec.bits_per_sample / 8);
    let riff_size = 36u32 + data_size;
    w.write_all(b"RIFF")?;
    w.write_all(&riff_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&spec.channels.to_le_bytes())?;
    w.write_all(&spec.sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&spec.bits_per_sample.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_size.to_le_bytes())?;
    Ok(())
}

// ============================================================
// 单元测试（B-06/B-12：音频回环、LIST chunk、尾部切片）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mimo_wav_codec_test_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).expect("创建测试目录失败");
        d
    }

    fn spec_16k_mono() -> WavSpec {
        WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        }
    }

    /// B-12 音频回环：write_wav_header → read_wav_data_offset → slice_wav 往返，
    /// 断言样本数与头部字段一致
    #[test]
    fn wav_header_read_offset_slice_roundtrip() {
        let dir = test_dir("roundtrip");
        let spec = spec_16k_mono();
        let samples: Vec<i16> = (0..16000).map(|i| (i % 1000) as i16).collect(); // 1s

        // write_wav_header + PCM
        let wav_path = dir.join("source.wav");
        {
            let mut out = BufWriter::new(File::create(&wav_path).unwrap());
            write_wav_header(&mut out, &spec, (samples.len() * 2) as u32).unwrap();
            for s in &samples {
                out.write_all(&s.to_le_bytes()).unwrap();
            }
            out.flush().unwrap();
        }

        // read_wav_data_offset：标准 44 字节头后紧跟 data
        let mut file = File::open(&wav_path).unwrap();
        let info = read_wav_data_offset(&mut file).expect("data chunk 应可定位");
        assert_eq!(info.data_offset, 44);

        // get_duration 精确时长
        let duration = get_duration(&wav_path).expect("时长解析失败");
        assert!(
            (duration - 1.0).abs() < 0.001,
            "时长应为 1.0s，实际 {}",
            duration
        );

        // slice_wav 往返：取中间 0.5s
        let slice = slice_wav(&wav_path, &dir, 0.25, 0.5).expect("切片失败");
        let reader = WavReader::open(&slice).expect("切片不可读");
        let out_spec = reader.spec();
        assert_eq!(out_spec.channels, 1);
        assert_eq!(out_spec.sample_rate, 16000);
        assert_eq!(out_spec.bits_per_sample, 16);
        let got: Vec<i16> = reader.into_samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(got.len(), 8000, "0.5s 应为 8000 样本");
        assert_eq!(got, &samples[4000..12000], "样本内容必须与源一致");

        // 切片文件头部字段断言：data chunk 长度 == 样本数*2
        let bytes = std::fs::read(&slice).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let data_size = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        assert_eq!(data_size, 16000, "data 长度应为 8000 样本 * 2 字节");
        assert_eq!(bytes.len(), 44 + data_size as usize);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 手工构造含额外 chunk 的 WAV（fmt + LIST + data），模拟真实转码产物
    fn build_wav_with_extra_chunks(samples: &[i16], sample_rate: u32) -> Vec<u8> {
        let fmt: Vec<u8> = {
            let mut v = Vec::new();
            v.extend_from_slice(b"fmt ");
            v.extend_from_slice(&16u32.to_le_bytes());
            v.extend_from_slice(&1u16.to_le_bytes()); // PCM
            v.extend_from_slice(&1u16.to_le_bytes()); // mono
            v.extend_from_slice(&sample_rate.to_le_bytes());
            v.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte_rate
            v.extend_from_slice(&2u16.to_le_bytes()); // block_align
            v.extend_from_slice(&16u16.to_le_bytes()); // bits
            v
        };
        // LIST chunk：INFO/ISFT 元数据（偶数长度，无对齐补位需求）
        let list_payload = b"INFOISFT\x08\x00\x00\x00test-app";
        let mut list = Vec::new();
        list.extend_from_slice(b"LIST");
        list.extend_from_slice(&(list_payload.len() as u32).to_le_bytes());
        list.extend_from_slice(list_payload);

        let data_size = (samples.len() * 2) as u32;
        let riff_size = 4 + fmt.len() as u32 + list.len() as u32 + 8 + data_size;

        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&riff_size.to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(&fmt);
        v.extend_from_slice(&list);
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_size.to_le_bytes());
        for s in samples {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    /// B-06 验收：含 LIST chunk 的合成 WAV，get_duration 与理论时长误差 < 0.1s，
    /// 最后一片 slice_wav 不报错
    #[test]
    fn duration_and_last_slice_with_list_chunk() {
        let dir = test_dir("list_chunk");
        let samples: Vec<i16> = (0..24000).map(|i| ((i % 500) - 250) as i16).collect(); // 1.5s
        let bytes = build_wav_with_extra_chunks(&samples, 16000);
        let wav_path = dir.join("with_list.wav");
        std::fs::write(&wav_path, &bytes).unwrap();

        // 旧实现用 meta.len()-44 估算，会因 LIST chunk 偏大；新实现必须精确
        let duration = get_duration(&wav_path).expect("含 LIST chunk 也应可解析");
        assert!(
            (duration - 1.5).abs() < 0.1,
            "误差必须 < 0.1s，实际 {}",
            duration
        );
        assert!(
            is_target_wav(&wav_path),
            "16k/mono/16bit 含 LIST chunk 仍应判定为目标格式"
        );

        // 最后一片（尾部 0.5s）切片不报错，样本数正确
        let last = slice_wav(&wav_path, &dir, 1.0, 0.5).expect("最后一片切片不得报错");
        let reader = WavReader::open(&last).unwrap();
        let got: Vec<i16> = reader.into_samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(got.len(), 8000);
        assert_eq!(got, &samples[16000..24000]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 最后一片超出实际时长：读到 EOF 优雅结束，不硬报 IO 错误
    #[test]
    fn slice_past_eof_truncates_gracefully() {
        let dir = test_dir("eof");
        let spec = spec_16k_mono();
        let samples: Vec<i16> = vec![0; 16000]; // 1s
        let wav_path = dir.join("source.wav");
        {
            let mut out = BufWriter::new(File::create(&wav_path).unwrap());
            write_wav_header(&mut out, &spec, (samples.len() * 2) as u32).unwrap();
            for s in &samples {
                out.write_all(&s.to_le_bytes()).unwrap();
            }
            out.flush().unwrap();
        }

        // 请求 0.9s 起的 0.5s：实际只剩 0.1s
        let slice = slice_wav(&wav_path, &dir, 0.9, 0.5).expect("越界切片应优雅截断而非报错");
        let reader = WavReader::open(&slice).unwrap();
        let got: Vec<i16> = reader.into_samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(got.len(), 1600, "应截断为实际剩余的 0.1s");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
