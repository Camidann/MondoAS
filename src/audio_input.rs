use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tempfile::Builder;

const WHISPER_SAMPLE_RATE: u32 = 16_000;
const LIVE_WINDOW_SECONDS: u32 = 4;

pub struct AudioTempFile {
    path: PathBuf,
}

impl AudioTempFile {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for AudioTempFile {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "No se pudo eliminar el audio temporal {}: {err}",
                self.path.display()
            );
        }
    }
}

pub struct AudioInput {
    recording: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<f32>>>,
    level: Arc<AtomicU32>,
    stream_error: Arc<Mutex<Option<String>>>,
    _stream: cpal::Stream,
    sample_rate: u32,
    channels: usize,
}

#[derive(Clone)]
pub struct AudioSnapshotSource {
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    channels: usize,
}

impl AudioSnapshotSource {
    pub fn sample_count(&self) -> Result<usize> {
        Ok(self
            .samples
            .lock()
            .map_err(|_| anyhow::anyhow!("Se dañó el estado de captura de audio"))?
            .len())
    }

    pub fn snapshot_wav(&self) -> Result<AudioTempFile> {
        let captured = self
            .samples
            .lock()
            .map_err(|_| anyhow::anyhow!("Se dañó el estado de captura de audio"))?
            .clone();
        let window_size = self.sample_rate as usize * self.channels * LIVE_WINDOW_SECONDS as usize;
        let first_sample = captured.len().saturating_sub(window_size);
        let mono = downmix(&captured[first_sample..], self.channels);
        let resampled = resample(&mono, self.sample_rate, WHISPER_SAMPLE_RATE);
        let cleaned = clean_audio(&resampled);
        write_wav(&cleaned)
    }
}

pub fn input_device_names() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .context("No se pudieron enumerar los dispositivos de entrada de audio")?;
    let mut names = devices
        .filter_map(|device| device.name().ok())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    Ok(names)
}

impl AudioInput {
    pub fn new(selected_device: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = match selected_device.filter(|name| !name.is_empty()) {
            Some(selected_name) => host
                .input_devices()
                .context("No se pudieron enumerar los micrófonos")?
                .find_map(|device| {
                    device
                        .name()
                        .ok()
                        .filter(|name| name == selected_name)
                        .map(|_| device)
                })
                .with_context(|| {
                    format!("No se encontró el micrófono seleccionado: {selected_name}")
                })?,
            None => host
                .default_input_device()
                .context("No hay un micrófono predeterminado. Conecta uno o selecciónalo.")?,
        };
        let device_name = device
            .name()
            .context("No se pudo obtener el nombre del micrófono")?;
        let supported_config = device
            .default_input_config()
            .with_context(|| format!("No se pudo abrir la configuración de entrada de `{device_name}`. Comprueba que el dispositivo esté habilitado."))?;
        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;
        let config = supported_config.config();
        let recording = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let level = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let stream_error = Arc::new(Mutex::new(None));

        let stream = match supported_config.sample_format() {
            cpal::SampleFormat::F32 => {
                let recording = Arc::clone(&recording);
                let samples = Arc::clone(&samples);
                let level = Arc::clone(&level);
                let stream_error = Arc::clone(&stream_error);
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        store_samples(data, &recording, &samples, &level, |x| *x)
                    },
                    move |error| report_audio_error(error, &stream_error),
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let recording = Arc::clone(&recording);
                let samples = Arc::clone(&samples);
                let level = Arc::clone(&level);
                let stream_error = Arc::clone(&stream_error);
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        store_samples(data, &recording, &samples, &level, |x| *x as f32 / 32768.0)
                    },
                    move |error| report_audio_error(error, &stream_error),
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let recording = Arc::clone(&recording);
                let samples = Arc::clone(&samples);
                let level = Arc::clone(&level);
                let stream_error = Arc::clone(&stream_error);
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        store_samples(data, &recording, &samples, &level, |x| {
                            (*x as f32 - 32768.0) / 32768.0
                        })
                    },
                    move |error| report_audio_error(error, &stream_error),
                    None,
                )
            }
            format => bail!("Formato de audio de micrófono no compatible: {format:?}"),
        }
        .with_context(|| format!("No se pudo crear la captura de audio para `{device_name}`"))?;

        stream
            .play()
            .with_context(|| format!("No se pudo iniciar `{device_name}`. Comprueba los permisos de micrófono y que ninguna otra aplicación lo esté bloqueando."))?;

        Ok(Self {
            recording,
            samples,
            level,
            stream_error,
            _stream: stream,
            sample_rate,
            channels,
        })
    }

    pub fn start(&self) -> Result<()> {
        if self.recording.load(Ordering::Relaxed) {
            return Ok(());
        }

        self.samples
            .lock()
            .map_err(|_| anyhow::anyhow!("Se dañó el estado de captura de audio"))?
            .clear();
        self.recording.store(true, Ordering::Relaxed);
        self.level.store(0.0f32.to_bits(), Ordering::Relaxed);
        Ok(())
    }

    pub fn input_level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }

    pub fn take_stream_error(&self) -> Option<String> {
        self.stream_error.lock().ok()?.take()
    }

    pub fn snapshot_source(&self) -> AudioSnapshotSource {
        AudioSnapshotSource {
            samples: Arc::clone(&self.samples),
            sample_rate: self.sample_rate,
            channels: self.channels,
        }
    }

    pub fn finish(&self) -> Result<AudioTempFile> {
        self.recording.store(false, Ordering::Relaxed);
        let captured = std::mem::take(
            &mut *self
                .samples
                .lock()
                .map_err(|_| anyhow::anyhow!("Se dañó el estado de captura de audio"))?,
        );
        let mono = downmix(&captured, self.channels);
        let resampled = resample(&mono, self.sample_rate, WHISPER_SAMPLE_RATE);
        let cleaned = clean_audio(&resampled);

        if cleaned.is_empty() {
            bail!(
                "No se capturó audio. Confirma el micrófono seleccionado, los permisos del sistema y que el dispositivo no esté silenciado."
            );
        }

        write_wav(&cleaned)
    }
}

fn clean_audio(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let noise_floor = 0.008f32;
    let frame_size = 160usize;
    let mut start = 0usize;
    while start + frame_size <= samples.len() {
        let frame = &samples[start..start + frame_size];
        let rms = frame.iter().map(|value| value * value).sum::<f32>() / frame.len() as f32;
        if rms.sqrt() > noise_floor {
            break;
        }
        start += frame_size;
    }
    if start + frame_size > samples.len()
        && !samples[start..]
            .iter()
            .any(|sample| sample.abs() > noise_floor)
    {
        return Vec::new();
    }

    let mut end = samples.len();
    while end > start + frame_size {
        let frame = &samples[end - frame_size..end];
        let rms = frame.iter().map(|value| value * value).sum::<f32>() / frame.len() as f32;
        if rms.sqrt() > noise_floor {
            break;
        }
        end -= frame_size;
    }

    if end <= start {
        return Vec::new();
    }
    let padding = (WHISPER_SAMPLE_RATE as usize / 10).min(samples.len());
    let start = start.saturating_sub(padding);
    let end = end.saturating_add(padding).min(samples.len());
    samples[start..end].to_vec()
}

fn write_wav(samples: &[f32]) -> Result<AudioTempFile> {
    let mut file = Builder::new()
        .suffix(".wav")
        .tempfile()
        .context("No se pudo crear el archivo temporal de audio")?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WHISPER_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(file.as_file_mut(), spec)
        .context("No se pudo preparar el audio para whisper.cpp")?;
    for sample in samples {
        let sample = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer
            .write_sample(sample)
            .context("No se pudo escribir el audio capturado")?;
    }
    writer
        .finalize()
        .context("No se pudo terminar el archivo de audio")?;

    let (file, path) = file
        .keep()
        .context("No se pudo preparar el WAV temporal para que whisper.cpp lo abra")?;
    drop(file);
    Ok(AudioTempFile { path })
}

fn store_samples<T>(
    input: &[T],
    recording: &AtomicBool,
    samples: &Mutex<Vec<f32>>,
    level: &AtomicU32,
    convert: impl Fn(&T) -> f32,
) {
    if !recording.load(Ordering::Relaxed) {
        return;
    }
    let Ok(mut samples) = samples.lock() else {
        return;
    };
    if !recording.load(Ordering::Relaxed) {
        return;
    }

    let converted = input.iter().map(convert).collect::<Vec<_>>();
    let rms = (converted.iter().map(|sample| sample * sample).sum::<f32>()
        / converted.len().max(1) as f32)
        .sqrt();
    level.store(rms.to_bits(), Ordering::Relaxed);
    samples.extend(converted);
}

fn report_audio_error(error: cpal::StreamError, last_error: &Mutex<Option<String>>) {
    eprintln!("Error de captura de audio: {error}");
    if let Ok(mut last_error) = last_error.lock() {
        *last_error = Some(format!("{error}"));
    }
}

fn downmix(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 0 {
        return Vec::new();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn resample(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if samples.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return samples.to_vec();
    }

    let output_len =
        ((samples.len() - 1) as u64 * target_rate as u64 / source_rate as u64 + 1) as usize;
    (0..output_len)
        .map(|index| {
            let position = index as f64 * source_rate as f64 / target_rate as f64;
            let left = position.floor() as usize;
            let right = (left + 1).min(samples.len() - 1);
            let fraction = (position - left as f64) as f32;
            samples[left] + (samples[right] - samples[left]) * fraction
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AudioSnapshotSource, LIVE_WINDOW_SECONDS, WHISPER_SAMPLE_RATE, clean_audio, downmix,
        resample, write_wav,
    };
    use std::sync::{Arc, Mutex};

    #[test]
    fn downmix_averages_interleaved_channels() {
        assert_eq!(downmix(&[1.0, -1.0, 0.5, 0.5], 2), vec![0.0, 0.5]);
    }

    #[test]
    fn resample_interpolates_samples() {
        assert_eq!(resample(&[0.0, 1.0], 1, 2), vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn clean_audio_keeps_quiet_speech_and_preserves_edges() {
        let mut samples = vec![0.0; WHISPER_SAMPLE_RATE as usize];
        samples[4_000..8_000].fill(0.01);

        let cleaned = clean_audio(&samples);

        assert!(cleaned.len() >= 4_000);
        assert!(cleaned.len() < samples.len());
        assert!(cleaned.first().is_some_and(|sample| *sample == 0.0));
        assert!(cleaned.last().is_some_and(|sample| *sample == 0.0));
    }

    #[test]
    fn generated_wav_is_complete_pcm_audio() {
        let samples = vec![0.25, -0.25, 0.5, -0.5];
        let wav = write_wav(&samples).unwrap();
        let reader = hound::WavReader::open(wav.path()).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, WHISPER_SAMPLE_RATE);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.duration(), samples.len() as u32);
        assert_eq!(
            reader
                .into_samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![8191, -8191, 16383, -16383]
        );
    }

    #[test]
    fn generated_wav_is_reopenable_by_another_process() {
        let wav = write_wav(&vec![0.25; WHISPER_SAMPLE_RATE as usize]).unwrap();
        let bytes = std::fs::read(wav.path()).unwrap();

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[36..40], b"data");

        #[cfg(windows)]
        {
            let status = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &format!(
                        "[System.IO.File]::ReadAllBytes('{}') | Out-Null",
                        wav.path().display()
                    ),
                ])
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    #[test]
    fn whisper_cli_can_decode_a_generated_temp_wav() {
        let Some(project_dir) = std::env::current_exe().ok().and_then(|exe| {
            exe.ancestors()
                .find(|dir| {
                    dir.join(r"src\whisper.cpp\build\bin\Release\whisper-cli.exe")
                        .is_file()
                        && dir.join("ggml-tiny.bin").is_file()
                })
                .map(std::path::Path::to_path_buf)
        }) else {
            return;
        };

        let whisper = project_dir.join(r"src\whisper.cpp\build\bin\Release\whisper-cli.exe");
        let model = project_dir.join("ggml-tiny.bin");
        let samples = (0..WHISPER_SAMPLE_RATE as usize * 2)
            .map(|index| {
                (std::f32::consts::TAU * 440.0 * index as f32 / WHISPER_SAMPLE_RATE as f32).sin()
                    * 0.1
            })
            .collect::<Vec<_>>();
        let wav = write_wav(&samples).unwrap();
        let output = std::process::Command::new(whisper)
            .args(["-m"])
            .arg(model)
            .args(["-f"])
            .arg(wav.path())
            .args(["-l", "en", "-np", "-nt"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "whisper-cli failed to decode the generated WAV: {stderr}"
        );
        assert!(
            !stderr.contains("failed to read audio file"),
            "whisper-cli could not open the generated WAV: {stderr}"
        );
    }

    #[test]
    fn live_snapshot_is_limited_to_a_recent_four_second_window() {
        let sample_rate = WHISPER_SAMPLE_RATE;
        let source = AudioSnapshotSource {
            samples: Arc::new(Mutex::new(vec![
                0.1;
                sample_rate as usize
                    * (LIVE_WINDOW_SECONDS as usize + 4)
            ])),
            sample_rate,
            channels: 1,
        };

        let snapshot = source.snapshot_wav().unwrap();
        let reader = hound::WavReader::open(snapshot.path()).unwrap();

        assert_eq!(reader.duration(), sample_rate * LIVE_WINDOW_SECONDS);
    }
}
