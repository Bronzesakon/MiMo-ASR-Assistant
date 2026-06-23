use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom};
use std::path::Path;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use symphonia::core::codecs::audio::well_known::{CODEC_ID_PCM_S16BE, CODEC_ID_PCM_S16LE};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::TrackType;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

#[derive(Debug)]
pub enum FfmpegError {
    InputNotFound(String),
    OutputDirNotFound(String),
    FfmpegFailed(String),
    InvalidOutput(String),
}

impl std::fmt::Display for FfmpegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FfmpegError::InputNotFound(path) => write!(f, "输入文件不存在: {}", path),
            FfmpegError::OutputDirNotFound(path) => write!(f, "输出目录不存在: {}", path),
            FfmpegError::FfmpegFailed(msg) => write!(f, "音频处理失败: {}", msg),
            FfmpegError::InvalidOutput(msg) => write!(f, "无效输出: {}", msg),
        }
    }
}

/// 音频格式信息
#[derive(Debug)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
    pub codec: String,
    pub is_wav: bool,
}

/// 探测音频文件格式信息
pub fn get_audio_info(path: &Path) -> Result<AudioInfo, FfmpegError> {
    if !path.exists() {
        return Err(FfmpegError::InputNotFound(path.display().to_string()));
    }

    let file = File::open(path)
        .map_err(|e| FfmpegError::FfmpegFailed(format!("打开文件失败: {}", e)))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let format_reader = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| FfmpegError::FfmpegFailed(format!("探测格式失败: {}", e)))?;

    let track = format_reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| FfmpegError::InvalidOutput("未找到音频轨道".to_string()))?;

    let codec_params = track.codec_params.as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| FfmpegError::InvalidOutput("无音频编码参数".to_string()))?;

    let sample_rate = codec_params.sample_rate.unwrap_or(0);
    let channels = codec_params.channels.as_ref().map(|c| c.count() as u32).unwrap_or(0);
    let bits_per_sample = codec_params.bits_per_sample.unwrap_or(0);
    let codec_id = codec_params.codec;
    let codec_name = format!("{:?}", codec_id);
    let is_wav = codec_id == CODEC_ID_PCM_S16LE || codec_id == CODEC_ID_PCM_S16BE;

    Ok(AudioInfo {
        sample_rate,
        channels,
        bits_per_sample,
        codec: codec_name,
        is_wav,
    })
}

/// 检查是否已是目标格式（16kHz / 单声道 / 16-bit WAV）
pub fn is_target_format(info: &AudioInfo) -> bool {
    info.is_wav
        && info.sample_rate == 16000
        && info.channels == 1
        && info.bits_per_sample == 16
}

/// 将任意音频转换为 16kHz 单声道 16-bit WAV
pub fn convert_to_wav(input_path: &Path, output_path: &Path) -> Result<(), FfmpegError> {
    if !input_path.exists() {
        return Err(FfmpegError::InputNotFound(input_path.display().to_string()));
    }

    let info = get_audio_info(input_path)?;
    if is_target_format(&info) {
        if input_path != output_path {
            if let Some(parent) = output_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::copy(input_path, output_path)
                .map_err(|e| FfmpegError::FfmpegFailed(format!("复制文件失败: {}", e)))?;
        }
        return Ok(());
    }

    if let Some(parent) = output_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // 探测 + 解码
    let file = File::open(input_path)
        .map_err(|e| FfmpegError::FfmpegFailed(format!("打开文件失败: {}", e)))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = input_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format_reader = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| FfmpegError::FfmpegFailed(format!("探测格式失败: {}", e)))?;

    let track = format_reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| FfmpegError::InvalidOutput("无音频轨道".to_string()))?
        .clone();

    let codec_params = track.codec_params.as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| FfmpegError::InvalidOutput("无音频编码参数".to_string()))?;

    let src_rate = codec_params.sample_rate.unwrap_or(44100);
    let src_channels = codec_params.channels.as_ref().map(|c| c.count()).unwrap_or(1);

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(codec_params, &AudioDecoderOptions::default())
        .map_err(|e| FfmpegError::FfmpegFailed(format!("创建解码器失败: {}", e)))?;

    // 解码所有采样为 f32 interleaved，然后转 mono
    let mut mono_samples: Vec<f32> = Vec::new();

    loop {
        match format_reader.next_packet() {
            Ok(Some(packet)) => match decoder.decode(&packet) {
                Ok(decoded) => {
                    // 用 copy_to_vec_interleaved 自动转换为 f32
                    let mut interleaved = Vec::with_capacity(decoded.frames() * src_channels);
                    decoded.copy_to_vec_interleaved::<f32>(&mut interleaved);

                    if src_channels == 1 {
                        mono_samples.extend_from_slice(&interleaved);
                    } else {
                        for frame in interleaved.chunks(src_channels) {
                            let sum: f32 = frame.iter().sum();
                            mono_samples.push(sum / src_channels as f32);
                        }
                    }
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(e) => return Err(FfmpegError::FfmpegFailed(format!("解码错误: {}", e))),
            },
            Ok(None) => break,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(FfmpegError::FfmpegFailed(format!("读取错误: {}", e))),
        }
    }

    if mono_samples.is_empty() {
        return Err(FfmpegError::InvalidOutput("解码后无音频数据".to_string()));
    }

    // 重采样到 16kHz
    let final_samples = if src_rate != 16000 {
        resample_f32(&mono_samples, src_rate, 16000)
            .map_err(|e| FfmpegError::FfmpegFailed(format!("重采样失败: {}", e)))?
    } else {
        mono_samples
    };

    // f32 → i16 + 写 WAV
    let spec = WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(output_path, spec)
        .map_err(|e| FfmpegError::FfmpegFailed(format!("创建 WAV 失败: {}", e)))?;
    for &s in &final_samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .map_err(|e| FfmpegError::FfmpegFailed(format!("写入 WAV 失败: {}", e)))?;
    }
    writer.finalize()
        .map_err(|e| FfmpegError::FfmpegFailed(format!("完成 WAV 写入失败: {}", e)))?;

    Ok(())
}

/// 切割 WAV 文件（输入为 16kHz/mono/16bit）
pub fn slice_wav(
    input_path: &Path,
    output_dir: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<std::path::PathBuf, FfmpegError> {
    if !input_path.exists() {
        return Err(FfmpegError::InputNotFound(input_path.display().to_string()));
    }
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir)
            .map_err(|_| FfmpegError::OutputDirNotFound(output_dir.display().to_string()))?;
    }

    let output_filename = format!(
        "slice_{:.1}_{:.1}.wav",
        start_seconds,
        start_seconds + duration_seconds
    );
    let output_path = output_dir.join(&output_filename);

    let mut reader = WavReader::open(input_path)
        .map_err(|e| FfmpegError::FfmpegFailed(format!("打开 WAV 失败: {}", e)))?;
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
        inner.seek(SeekFrom::Start(header.data_offset + byte_offset)).map_err(io_err)?;

        let mut pcm_buf = vec![0u8; data_size as usize];
        inner.read_exact(&mut pcm_buf).map_err(io_err)?;

        let out_spec = WavSpec {
            channels: spec.channels,
            sample_rate: spec.sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let out_file = File::create(&output_path)
            .map_err(|e| FfmpegError::FfmpegFailed(format!("创建输出失败: {}", e)))?;
        let mut writer = WavWriter::new(BufWriter::new(out_file), out_spec)
            .map_err(|e| FfmpegError::FfmpegFailed(format!("创建 WAV 写入器失败: {}", e)))?;

        for chunk in pcm_buf.chunks_exact(2) {
            writer.write_sample(i16::from_le_bytes([chunk[0], chunk[1]]))
                .map_err(hound_err)?;
        }
        writer.finalize().map_err(hound_err)?;
        return Ok(output_path);
    }

    // 通用路径
    let mut writer = WavWriter::create(&output_path, spec)
        .map_err(|e| FfmpegError::FfmpegFailed(format!("创建 WAV 失败: {}", e)))?;
    let mut samples_iter = reader.samples::<i16>();
    let skip = start_sample as u64 * spec.channels as u64;
    for _ in 0..skip { let _ = samples_iter.next(); }
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

/// 获取音频时长（秒）
pub fn get_duration(path: &Path) -> Result<f64, FfmpegError> {
    if !path.exists() {
        return Err(FfmpegError::InputNotFound(path.display().to_string()));
    }

    let file = File::open(path)
        .map_err(|e| FfmpegError::FfmpegFailed(format!("打开文件失败: {}", e)))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let format_reader = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| FfmpegError::FfmpegFailed(format!("探测格式失败: {}", e)))?;

    let track = format_reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| FfmpegError::InvalidOutput("无音频轨道".to_string()))?;

    let codec_params = track.codec_params.as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| FfmpegError::InvalidOutput("无音频编码参数".to_string()))?;

    let sample_rate = codec_params.sample_rate.unwrap_or(44100) as f64;
    let codec_id = codec_params.codec;

    // WAV：从文件大小计算
    if codec_id == CODEC_ID_PCM_S16LE || codec_id == CODEC_ID_PCM_S16BE {
        if let Ok(meta) = std::fs::metadata(path) {
            let channels = codec_params.channels.as_ref().map(|c| c.count() as u64).unwrap_or(1);
            let bits = codec_params.bits_per_sample.unwrap_or(16) as u64;
            let bytes_per_frame = channels * bits / 8;
            if bytes_per_frame > 0 && meta.len() > 44 {
                let data_size = meta.len() - 44;
                let frames = data_size / bytes_per_frame;
                return Ok(frames as f64 / sample_rate);
            }
        }
    }

    // 其他格式：从 track 的 num_frames
    if let Some(n_frames) = track.num_frames {
        let dur = n_frames as f64 / sample_rate;
        if dur > 0.0 {
            return Ok(dur);
        }
    }

    Err(FfmpegError::InvalidOutput(
        "无法获取音频时长（缺少帧数信息）".to_string(),
    ))
}

// ============================================================
// 内部辅助
// ============================================================

/// f32 mono 重采样
fn resample_f32(input: &[f32], src_rate: u32, dst_rate: u32) -> Result<Vec<f32>, String> {
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let chunk_size = input.len().min(4096).max(64);
    let mut resampler = SincFixedIn::<f32>::new(
        dst_rate as f64 / src_rate as f64,
        2.0,
        params,
        chunk_size,
        1,
    )
    .map_err(|e| format!("创建重采样器失败: {}", e))?;

    let mut output = Vec::new();
    let mut pos = 0;

    while pos < input.len() {
        let end = (pos + chunk_size).min(input.len());
        let chunk_len = end - pos;

        let mut padded = vec![0.0f32; chunk_size];
        padded[..chunk_len].copy_from_slice(&input[pos..end]);

        let waves_in = vec![padded];
        let waves_out = resampler
            .process(&waves_in, None)
            .map_err(|e| format!("重采样执行失败: {}", e))?;

        if let Some(channel_out) = waves_out.into_iter().next() {
            if pos + chunk_size >= input.len() {
                let ratio = dst_rate as f64 / src_rate as f64;
                let valid_out = (chunk_len as f64 * ratio).ceil() as usize;
                output.extend_from_slice(&channel_out[..valid_out.min(channel_out.len())]);
            } else {
                output.extend_from_slice(&channel_out);
            }
        }

        pos = end;
    }

    Ok(output)
}

struct WavDataInfo { data_offset: u64 }

fn read_wav_data_offset<R: Read + Seek>(reader: &mut R) -> Result<WavDataInfo, FfmpegError> {
    reader.seek(SeekFrom::Start(0)).map_err(io_err)?;
    let mut buf4 = [0u8; 4];
    reader.read_exact(&mut buf4).map_err(io_err)?;
    if &buf4 != b"RIFF" {
        return Err(FfmpegError::InvalidOutput("不是有效的 WAV 文件".to_string()));
    }
    reader.read_exact(&mut buf4).map_err(io_err)?;
    reader.read_exact(&mut buf4).map_err(io_err)?;
    if &buf4 != b"WAVE" {
        return Err(FfmpegError::InvalidOutput("不是有效的 WAV 文件".to_string()));
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
            return Ok(WavDataInfo { data_offset: reader.stream_position().map_err(io_err)? });
        }

        reader.seek(SeekFrom::Current((chunk_size + chunk_size % 2) as i64)).map_err(io_err)?;
    }

    Err(FfmpegError::InvalidOutput("WAV 文件中未找到 data 块".to_string()))
}

fn io_err(e: std::io::Error) -> FfmpegError {
    FfmpegError::FfmpegFailed(e.to_string())
}

fn hound_err(e: hound::Error) -> FfmpegError {
    FfmpegError::FfmpegFailed(e.to_string())
}
