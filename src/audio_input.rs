use std::collections::VecDeque;
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
const PRE_ROLL_MILLISECONDS: usize = 500;

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
    pre_roll: Arc<Mutex<VecDeque<f32>>>,
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
        let window_size = self.sample_rate as usize * self.channels * LIVE_WINDOW_SECONDS as usize;
        let recent_samples = {
            let captured = self
                .samples
                .lock()
                .map_err(|_| anyhow::anyhow!("Se dañó el estado de captura de audio"))?;
            let first_sample = captured.len().saturating_sub(window_size);
            captured[first_sample..].to_vec()
        };
        let mono = downmix(&recent_samples, self.channels);
        let resampled = resample(&mono, self.sample_rate, WHISPER_SAMPLE_RATE);
        let cleaned = prepare_audio(&resampled);
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
        let pre_roll = Arc::new(Mutex::new(VecDeque::new()));
        let pre_roll_capacity = sample_rate as usize * channels * PRE_ROLL_MILLISECONDS / 1000;
        let level = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let stream_error = Arc::new(Mutex::new(None));

        let stream = match supported_config.sample_format() {
            cpal::SampleFormat::F32 => {
                let recording = Arc::clone(&recording);
                let samples = Arc::clone(&samples);
                let pre_roll = Arc::clone(&pre_roll);
                let level = Arc::clone(&level);
                let stream_error = Arc::clone(&stream_error);
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        store_samples(
                            data,
                            &recording,
                            &samples,
                            &pre_roll,
                            pre_roll_capacity,
                            &level,
                            |x| *x,
                        )
                    },
                    move |error| report_audio_error(error, &stream_error),
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let recording = Arc::clone(&recording);
                let samples = Arc::clone(&samples);
                let pre_roll = Arc::clone(&pre_roll);
                let level = Arc::clone(&level);
                let stream_error = Arc::clone(&stream_error);
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        store_samples(
                            data,
                            &recording,
                            &samples,
                            &pre_roll,
                            pre_roll_capacity,
                            &level,
                            |x| *x as f32 / 32768.0,
                        )
                    },
                    move |error| report_audio_error(error, &stream_error),
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let recording = Arc::clone(&recording);
                let samples = Arc::clone(&samples);
                let pre_roll = Arc::clone(&pre_roll);
                let level = Arc::clone(&level);
                let stream_error = Arc::clone(&stream_error);
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        store_samples(
                            data,
                            &recording,
                            &samples,
                            &pre_roll,
                            pre_roll_capacity,
                            &level,
                            |x| (*x as f32 - 32768.0) / 32768.0,
                        )
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
            pre_roll,
            level,
            stream_error,
            _stream: stream,
            sample_rate,
            channels,
        })
    }

    pub fn start_with_pre_roll(&self) -> Result<()> {
        if self.recording.load(Ordering::Relaxed) {
            return Ok(());
        }

        let mut pre_roll = self
            .pre_roll
            .lock()
            .map_err(|_| anyhow::anyhow!("Se dañó el estado de captura de audio"))?;
        let mut samples = self
            .samples
            .lock()
            .map_err(|_| anyhow::anyhow!("Se dañó el estado de captura de audio"))?;
        samples.clear();
        samples.extend(pre_roll.iter().copied());
        pre_roll.clear();
        self.level.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.recording.store(true, Ordering::Relaxed);
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
        let cleaned = prepare_audio(&resampled);

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

    let noise_floor = 0.003f32;
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

fn prepare_audio(samples: &[f32]) -> Vec<f32> {
    let mut cleaned = clean_audio(samples);
    if cleaned.is_empty() {
        return cleaned;
    }

    let frame_size = WHISPER_SAMPLE_RATE as usize / 100;
    let mut frame_levels = cleaned
        .chunks(frame_size)
        .map(|frame| {
            (frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32).sqrt()
        })
        .filter(|level| *level >= 0.003)
        .collect::<Vec<_>>();
    if frame_levels.is_empty() {
        return cleaned;
    }
    frame_levels.sort_by(f32::total_cmp);
    let reference_level = frame_levels[frame_levels.len() * 3 / 4];
    let peak = cleaned
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    let gain = (0.12 / reference_level)
        .clamp(0.25, 8.0)
        .min(0.95 / peak.max(0.001));
    for sample in &mut cleaned {
        *sample = (*sample * gain).clamp(-0.95, 0.95);
    }
    cleaned
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
    pre_roll: &Mutex<VecDeque<f32>>,
    pre_roll_capacity: usize,
    level: &AtomicU32,
    convert: impl Fn(&T) -> f32,
) {
    let mut converted = Vec::with_capacity(input.len());
    let mut sum_squares = 0.0;
    for value in input {
        let sample = convert(value);
        sum_squares += sample * sample;
        converted.push(sample);
    }
    let rms = (sum_squares / input.len().max(1) as f32).sqrt();
    level.store(rms.to_bits(), Ordering::Relaxed);

    if recording.load(Ordering::Relaxed) {
        if let Ok(mut samples) = samples.lock()
            && recording.load(Ordering::Relaxed)
        {
            samples.extend(converted);
        }
        return;
    }

    if let Ok(mut pre_roll) = pre_roll.lock() {
        if recording.load(Ordering::Relaxed) {
            drop(pre_roll);
            if let Ok(mut samples) = samples.lock()
                && recording.load(Ordering::Relaxed)
            {
                samples.extend(converted);
            }
            return;
        }
        pre_roll.extend(converted);
        while pre_roll.len() > pre_roll_capacity {
            pre_roll.pop_front();
        }
    }
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
    let mut channel_energy = vec![0.0; channels];
    for frame in samples.chunks_exact(channels) {
        for (energy, sample) in channel_energy.iter_mut().zip(frame) {
            *energy += sample * sample;
        }
    }
    let mut best_channel = 0;
    for channel in 1..channels {
        if channel_energy[channel] > channel_energy[best_channel] {
            best_channel = channel;
        }
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame[best_channel])
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
    let cutoff = (target_rate as f64 / source_rate as f64).min(1.0) * 0.94;
    let radius = 32isize;
    const RESAMPLE_PHASES: usize = 256;
    let kernel = (0..RESAMPLE_PHASES)
        .map(|phase| {
            let fraction = phase as f64 / RESAMPLE_PHASES as f64;
            (0..64)
                .map(|tap| {
                    let offset = tap as isize - radius + 1;
                    let distance = fraction - offset as f64;
                    let window =
                        0.5 + 0.5 * (std::f64::consts::PI * distance / radius as f64).cos();
                    let scaled_distance = cutoff * distance;
                    let sinc = if scaled_distance.abs() < f64::EPSILON {
                        1.0
                    } else {
                        (std::f64::consts::PI * scaled_distance).sin()
                            / (std::f64::consts::PI * scaled_distance)
                    };
                    cutoff * sinc * window
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    (0..output_len)
        .map(|index| {
            let position = index as f64 * source_rate as f64 / target_rate as f64;
            let mut center = position.floor() as isize;
            let mut phase = ((position - center as f64) * RESAMPLE_PHASES as f64).round() as usize;
            if phase == RESAMPLE_PHASES {
                center += 1;
                phase = 0;
            }
            let mut weighted_sum = 0.0;
            let mut weight_sum = 0.0;
            for (tap, weight) in kernel[phase].iter().enumerate() {
                let source_index = center + tap as isize - radius + 1;
                if !(0..samples.len() as isize).contains(&source_index) {
                    continue;
                }
                weighted_sum += samples[source_index as usize] as f64 * weight;
                weight_sum += weight;
            }
            if weight_sum.abs() < f64::EPSILON {
                0.0
            } else {
                (weighted_sum / weight_sum) as f32
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AudioSnapshotSource, LIVE_WINDOW_SECONDS, WHISPER_SAMPLE_RATE, clean_audio, downmix,
        prepare_audio, resample, store_samples, write_wav,
    };
    use std::collections::VecDeque;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    };

    #[test]
    fn idle_audio_detection_keeps_only_recent_samples_and_updates_level() {
        let recording = AtomicBool::new(false);
        let samples = Mutex::new(Vec::new());
        let pre_roll = Mutex::new(VecDeque::new());
        let level = AtomicU32::new(0.0f32.to_bits());

        store_samples(
            &[0.1, 0.2, 0.3],
            &recording,
            &samples,
            &pre_roll,
            2,
            &level,
            |sample| *sample,
        );

        assert_eq!(
            pre_roll.lock().unwrap().iter().copied().collect::<Vec<_>>(),
            vec![0.2, 0.3]
        );
        assert!(samples.lock().unwrap().is_empty());
        assert!((f32::from_bits(level.load(Ordering::Relaxed)) - 0.216_024_68).abs() < 0.0001);
    }

    #[test]
    fn downmix_selects_the_strongest_interleaved_channel() {
        assert_eq!(downmix(&[1.0, -1.0, 0.5, 0.5], 2), vec![1.0, 0.5]);
    }

    #[test]
    fn downmix_avoids_cancelling_out_of_phase_channels() {
        assert_eq!(downmix(&[0.5, -0.5, 0.25, -0.25], 2), vec![0.5, 0.25]);
    }

    #[test]
    fn resample_preserves_dc_and_output_length() {
        let samples = vec![0.25; 48_000];
        let resampled = resample(&samples, 48_000, WHISPER_SAMPLE_RATE);

        assert_eq!(resampled.len(), WHISPER_SAMPLE_RATE as usize);
        assert!(
            resampled
                .iter()
                .all(|sample| (*sample - 0.25).abs() < 0.001)
        );
    }

    #[test]
    fn resample_rejects_frequencies_above_the_target_nyquist_limit() {
        let samples = (0..48_000)
            .map(|index| (std::f32::consts::TAU * 12_000.0 * index as f32 / 48_000.0).sin())
            .collect::<Vec<_>>();

        let resampled = resample(&samples, 48_000, WHISPER_SAMPLE_RATE);
        let stable = &resampled[100..resampled.len() - 100];
        let rms =
            (stable.iter().map(|sample| sample * sample).sum::<f32>() / stable.len() as f32).sqrt();

        assert!(rms < 0.05, "out-of-band RMS was {rms}");
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
    fn prepare_audio_boosts_quiet_speech_without_clipping() {
        let mut samples = vec![0.0; WHISPER_SAMPLE_RATE as usize];
        samples[2_000..14_000].fill(0.01);

        let prepared = prepare_audio(&samples);

        assert!(prepared.iter().any(|sample| *sample > 0.05));
        assert!(prepared.iter().all(|sample| sample.abs() <= 0.95));
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
