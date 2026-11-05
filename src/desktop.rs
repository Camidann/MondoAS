use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

use anyhow::{Context, Result};
use eframe::egui::{self, ColorImage};
use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};

use crate::{
    agent::Agent,
    audio_input::{AudioInput, AudioSnapshotSource, input_device_names},
    config::AgentConfig,
    speech::speak,
    transcription::WhisperTranscriber,
};

enum AppEvent {
    UserTranscript(String),
    LiveTranscript(String),
    Reply(String),
    ReplyChunk(String),
    Status(String),
    Speaking(bool),
    Finished,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FaceState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    Error,
    Warmup,
}

const VOICE_ACTIVITY_THRESHOLD: f32 = 0.015;
const VOICE_START_CONFIRMATION: Duration = Duration::from_millis(150);
const VOICE_END_SILENCE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VoiceActivity {
    Started,
    Finished,
}

#[derive(Default)]
struct AutomaticSpeechDetector {
    voice_since: Option<Instant>,
    silence_since: Option<Instant>,
}

impl AutomaticSpeechDetector {
    fn update(&mut self, now: Instant, level: f32, recording: bool) -> Option<VoiceActivity> {
        if recording {
            self.voice_since = None;
            if level >= VOICE_ACTIVITY_THRESHOLD {
                self.silence_since = None;
                return None;
            }

            let silence_since = *self.silence_since.get_or_insert(now);
            if now.duration_since(silence_since) >= VOICE_END_SILENCE {
                self.reset();
                return Some(VoiceActivity::Finished);
            }
            return None;
        }

        self.silence_since = None;
        if level < VOICE_ACTIVITY_THRESHOLD {
            self.voice_since = None;
            return None;
        }

        let voice_since = *self.voice_since.get_or_insert(now);
        if now.duration_since(voice_since) >= VOICE_START_CONFIRMATION {
            self.voice_since = None;
            return Some(VoiceActivity::Started);
        }
        None
    }

    fn reset(&mut self) {
        self.voice_since = None;
        self.silence_since = None;
    }
}

fn default_input_device_label() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Predeterminado de Windows"
    }
    #[cfg(target_os = "linux")]
    {
        "Dispositivo predeterminado de Linux"
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        "Dispositivo predeterminado"
    }
}

struct FaceAnimation {
    idle_frame: usize,
    blink_frame: Option<usize>,
    capturing_frames: Vec<usize>,
    listening_frames: Vec<usize>,
    thinking_frames: Vec<usize>,
    speaking_frames: Vec<usize>,
    error_frame: Option<usize>,
    warmup_frame: Option<usize>,
    fallback_frames: Vec<usize>,
    breath_frame: Option<usize>,
    current_frame: usize,
    current_state: Option<FaceState>,
    next_sequence_frame: f64,
    sequence_step: usize,
    capturing_since: f64,
    next_blink: f64,
    blink_until: f64,
    next_breath: f64,
    breath_until: f64,
}

impl FaceAnimation {
    fn new(paths: &[std::path::PathBuf]) -> Self {
        let mut animation = Self {
            idle_frame: 0,
            blink_frame: None,
            capturing_frames: Vec::new(),
            listening_frames: Vec::new(),
            thinking_frames: Vec::new(),
            speaking_frames: Vec::new(),
            error_frame: None,
            warmup_frame: None,
            fallback_frames: Vec::new(),
            breath_frame: None,
            current_frame: 0,
            current_state: None,
            next_sequence_frame: 0.0,
            sequence_step: 0,
            capturing_since: 0.0,
            next_blink: 3.5,
            blink_until: 0.0,
            next_breath: 4.0,
            breath_until: 0.0,
        };

        for (index, path) in paths.iter().enumerate() {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if ["idle", "normal", "reposo", "base"]
                .iter()
                .any(|tag| name.contains(tag))
            {
                animation.idle_frame = index;
            } else if ["blink", "parpade", "pestane"]
                .iter()
                .any(|tag| name.contains(tag))
            {
                animation.blink_frame.get_or_insert(index);
            } else if ["breath", "aire", "inhal", "pausa"]
                .iter()
                .any(|tag| name.contains(tag))
            {
                animation.breath_frame.get_or_insert(index);
            } else if ["captur", "capture"].iter().any(|tag| name.contains(tag)) {
                animation.capturing_frames.push(index);
            } else if ["listen", "escuch"].iter().any(|tag| name.contains(tag)) {
                animation.listening_frames.push(index);
            } else if ["think", "pens"].iter().any(|tag| name.contains(tag)) {
                animation.thinking_frames.push(index);
            } else if ["error", "fall", "fail"]
                .iter()
                .any(|tag| name.contains(tag))
            {
                animation.error_frame.get_or_insert(index);
            } else if ["warmup", "calent", "cargando"]
                .iter()
                .any(|tag| name.contains(tag))
            {
                animation.warmup_frame.get_or_insert(index);
            } else if [
                "speak", "talk", "habla", "boca", "mouth", "lengua", "tongue", "abierta", "open",
            ]
            .iter()
            .any(|tag| name.contains(tag))
            {
                animation.speaking_frames.push(index);
            } else {
                animation.fallback_frames.push(index);
            }
        }

        if !paths.is_empty()
            && animation.idle_frame == 0
            && !paths[0]
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
                .contains("idle")
        {
            animation.idle_frame = animation.fallback_frames.first().copied().unwrap_or(0);
        }
        animation
    }

    fn frame(&mut self, now: f64, state: FaceState) -> usize {
        if self.current_state != Some(state) {
            self.current_state = Some(state);
            self.current_frame = self.frame_for_state(state);
            self.sequence_step = 0;
            self.next_sequence_frame = now;
            self.capturing_since = now;
            self.next_blink = now + 3.5;
            self.blink_until = 0.0;
            self.next_breath = now + 3.8;
            self.breath_until = 0.0;
        }

        match state {
            FaceState::Idle => {
                if let Some(blink_frame) = self.blink_frame {
                    if now >= self.next_blink {
                        self.blink_until = now + 0.14;
                        self.next_blink = now + 4.2;
                    }
                    self.current_frame = if now < self.blink_until {
                        blink_frame
                    } else {
                        self.idle_frame
                    };
                }
            }
            FaceState::Listening => {
                if now - self.capturing_since < 0.55 && !self.capturing_frames.is_empty() {
                    self.current_frame = self.capturing_frames[0];
                } else {
                    self.animate_sequence(now, &self.listening_frames.clone(), 0.65);
                }
            }
            FaceState::Thinking => {
                self.animate_sequence(now, &self.thinking_frames.clone(), 0.42);
            }
            FaceState::Speaking => {
                if now >= self.next_breath {
                    self.breath_until = now + 0.28;
                    self.next_breath = now + 4.1 + (self.sequence_step % 3) as f64 * 0.55;
                }
                if now < self.breath_until && self.breath_frame.is_some() {
                    self.current_frame = self.breath_frame.unwrap_or(self.idle_frame);
                } else {
                    let durations = [2.19, 2.27, 2.22, 2.31, 2.24];
                    let interval = durations[self.sequence_step % durations.len()];
                    self.animate_sequence(now, &self.speaking_frames.clone(), interval);
                }
            }
            FaceState::Error => {
                self.current_frame = self.error_frame.unwrap_or(self.idle_frame);
            }
            FaceState::Warmup => {
                self.current_frame = self.warmup_frame.unwrap_or(self.idle_frame);
            }
        }

        self.current_frame
    }

    fn frame_for_state(&self, state: FaceState) -> usize {
        match state {
            FaceState::Idle => self.idle_frame,
            FaceState::Listening => self
                .capturing_frames
                .first()
                .or_else(|| self.listening_frames.first())
                .copied()
                .unwrap_or(self.idle_frame),
            FaceState::Thinking => self
                .thinking_frames
                .first()
                .copied()
                .unwrap_or(self.idle_frame),
            FaceState::Speaking => self
                .speaking_frames
                .first()
                .or_else(|| self.fallback_frames.first())
                .copied()
                .unwrap_or(self.idle_frame),
            FaceState::Error => self.error_frame.unwrap_or(self.idle_frame),
            FaceState::Warmup => self.warmup_frame.unwrap_or(self.idle_frame),
        }
    }

    fn animate_sequence(&mut self, now: f64, frames: &[usize], interval: f64) {
        let frames = if frames.is_empty() {
            &self.fallback_frames
        } else {
            frames
        };
        if frames.is_empty() || now < self.next_sequence_frame {
            return;
        }

        self.current_frame = frames[self.sequence_step % frames.len()];
        self.sequence_step += 1;
        self.next_sequence_frame = now + interval;
    }
}

pub struct DesktopApp {
    config: AgentConfig,
    agent: Agent,
    transcriber: WhisperTranscriber,
    runtime: tokio::runtime::Runtime,
    audio: Option<AudioInput>,
    audio_error: Option<String>,
    input_devices: Vec<String>,
    selected_input_device: String,
    event_tx: Sender<AppEvent>,
    event_rx: Receiver<AppEvent>,
    messages: Vec<(String, String)>,
    input: String,
    status: String,
    recording: bool,
    automatic_recording: bool,
    auto_listening: bool,
    speech_detector: AutomaticSpeechDetector,
    live_transcription: Option<JoinHandle<()>>,
    live_transcript: String,
    request_running: bool,
    speaking: Arc<AtomicBool>,
    avatar_visible: bool,
    avatar_textures: Vec<egui::TextureHandle>,
    face_animation: FaceAnimation,
    fullscreen: bool,
    avatar_size_save_at: Option<f64>,
    chat_visible: bool,
    focus_input: bool,
    _hotkey_manager: Option<GlobalHotKeyManager>,
    push_to_talk_hotkey: HotKey,
    avatar_toggle_hotkey: HotKey,
    push_to_talk_registered: bool,
    avatar_toggle_registered: bool,
    hotkey_registration_error: Option<String>,
}

impl DesktopApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: AgentConfig,
        runtime: tokio::runtime::Runtime,
    ) -> Result<Self> {
        let push_to_talk_hotkey =
            HotKey::from_str(&config.push_to_talk_hotkey).with_context(|| {
                format!(
                    "push_to_talk_hotkey inválido: {}",
                    config.push_to_talk_hotkey
                )
            })?;
        let avatar_toggle_hotkey =
            HotKey::from_str(&config.avatar_toggle_hotkey).with_context(|| {
                format!(
                    "avatar_toggle_hotkey inválido: {}",
                    config.avatar_toggle_hotkey
                )
            })?;
        if push_to_talk_hotkey.id() == avatar_toggle_hotkey.id() {
            anyhow::bail!("push_to_talk_hotkey y avatar_toggle_hotkey deben ser diferentes");
        }
        let (hotkey_manager, push_to_talk_registered, avatar_toggle_registered, hotkey_error) =
            match GlobalHotKeyManager::new() {
                Ok(manager) => {
                    let mut errors = Vec::new();
                    let push_to_talk_registered = match manager.register(push_to_talk_hotkey) {
                        Ok(()) => true,
                        Err(err) => {
                            errors.push(format!(
                                "No se pudo registrar {}: {err}",
                                config.push_to_talk_hotkey
                            ));
                            false
                        }
                    };
                    let avatar_toggle_registered = match manager.register(avatar_toggle_hotkey) {
                        Ok(()) => true,
                        Err(err) => {
                            errors.push(format!(
                                "No se pudo registrar {}: {err}",
                                config.avatar_toggle_hotkey
                            ));
                            false
                        }
                    };
                    (
                        Some(manager),
                        push_to_talk_registered,
                        avatar_toggle_registered,
                        (!errors.is_empty()).then(|| errors.join(" | ")),
                    )
                }
                Err(err) => (
                    None,
                    false,
                    false,
                    Some(format!(
                        "No se pudo iniciar el registro de atajos globales: {err}"
                    )),
                ),
            };

        let (event_tx, event_rx) = mpsc::channel();
        let (input_devices, mut audio_error) = match input_device_names() {
            Ok(devices) => (devices, None),
            Err(err) => (Vec::new(), Some(format!("{err:#}"))),
        };
        let selected_input_device = config.input_device.clone();
        let selected_device =
            (!selected_input_device.is_empty()).then_some(selected_input_device.as_str());
        let audio = match AudioInput::new(selected_device) {
            Ok(audio) => Some(audio),
            Err(err) => {
                let message = format!("{err:#}");
                audio_error = Some(message);
                None
            }
        };
        let configured_paths = configured_avatar_paths(&config);
        let (avatar_paths, path_warning) = match resolve_avatar_paths(&config) {
            Ok(paths) => (paths, None),
            Err(err) => (configured_paths, Some(format!("{err:#}"))),
        };
        let (avatar_textures, texture_warning) =
            match load_avatar_textures(&cc.egui_ctx, &avatar_paths) {
                Ok(textures) => (textures, None),
                Err(err) => (Vec::new(), Some(format!("{err:#}"))),
            };
        let avatar_warning = path_warning.or(texture_warning);
        let initial_status = avatar_warning
            .or_else(|| {
                if audio.is_none() {
                    audio_error
                        .as_ref()
                        .map(|error| format!("Micrófono no disponible: {error}"))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "Listo para hablar con el atajo o el botón Hablar.".to_string());
        let auto_listening = false;
        let agent = Agent::new(config.clone())?;

        Ok(Self {
            avatar_visible: config.avatar_enabled,
            agent,
            transcriber: WhisperTranscriber::new(config.clone()),
            config,
            runtime,
            audio,
            audio_error,
            input_devices,
            selected_input_device,
            event_tx,
            event_rx,
            messages: Vec::new(),
            input: String::new(),
            status: initial_status,
            recording: false,
            automatic_recording: false,
            auto_listening,
            speech_detector: AutomaticSpeechDetector::default(),
            live_transcription: None,
            live_transcript: String::new(),
            request_running: false,
            speaking: Arc::new(AtomicBool::new(false)),
            avatar_textures,
            face_animation: FaceAnimation::new(&avatar_paths),
            fullscreen: false,
            avatar_size_save_at: None,
            chat_visible: false,
            focus_input: false,
            _hotkey_manager: hotkey_manager,
            push_to_talk_hotkey,
            avatar_toggle_hotkey,
            push_to_talk_registered,
            avatar_toggle_registered,
            hotkey_registration_error: hotkey_error,
        })
    }

    fn poll_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::UserTranscript(transcript) => {
                    self.messages.push(("Tú".to_string(), transcript));
                    self.live_transcript.clear();
                }
                AppEvent::LiveTranscript(transcript) => self.live_transcript = transcript,
                AppEvent::Reply(reply) => {
                    self.messages
                        .push((self.config.assistant_name.clone(), reply));
                }
                AppEvent::ReplyChunk(chunk) => {
                    if let Some((speaker, reply)) = self.messages.last_mut()
                        && speaker == &self.config.assistant_name
                    {
                        reply.push_str(&chunk);
                    } else {
                        self.messages
                            .push((self.config.assistant_name.clone(), chunk));
                    }
                }
                AppEvent::Status(status) => self.status = status,
                AppEvent::Speaking(speaking) => {
                    self.speaking.store(speaking, Ordering::Relaxed);
                }
                AppEvent::Finished => {
                    self.request_running = false;
                }
            }
        }
    }

    fn poll_audio_error(&mut self) {
        if let Some(error) = self.audio.as_ref().and_then(AudioInput::take_stream_error) {
            self.status = format!("Error del micrófono: {error}");
            self.audio_error = Some(error);
            self.audio = None;
            self.auto_listening = false;
            self.speech_detector.reset();
            self.recording = false;
            self.automatic_recording = false;
        }
    }

    fn open_audio_input(&mut self) -> Result<()> {
        let selected =
            (!self.selected_input_device.is_empty()).then_some(self.selected_input_device.as_str());
        self.audio = Some(AudioInput::new(selected)?);
        self.audio_error = None;
        Ok(())
    }

    fn toggle_auto_listening(&mut self) {
        if self.recording || self.request_running {
            return;
        }

        self.speech_detector.reset();
        if self.auto_listening {
            self.auto_listening = false;
            self.audio = None;
            self.status = "Escucha automática pausada".to_string();
            return;
        }

        match self.open_audio_input() {
            Ok(()) => {
                self.auto_listening = true;
                self.status = "Escucha automática activa. Habla cuando quieras.".to_string();
            }
            Err(err) => {
                let message = format!("{err:#}");
                self.audio_error = Some(message.clone());
                self.status = format!("No se pudo activar la escucha automática: {message}");
            }
        }
    }

    fn poll_automatic_speech(&mut self) {
        if !self.auto_listening
            || self.request_running
            || self.speaking.load(Ordering::Relaxed)
            || (self.recording && !self.automatic_recording)
        {
            self.speech_detector.reset();
            return;
        }

        let Some(audio) = &self.audio else {
            self.speech_detector.reset();
            return;
        };
        let level = audio.input_level();
        match self
            .speech_detector
            .update(Instant::now(), level, self.automatic_recording)
        {
            Some(VoiceActivity::Started) => self.start_recording(true),
            Some(VoiceActivity::Finished) => self.finish_recording(),
            None => {}
        }
    }

    fn face_state(&self) -> FaceState {
        let status = self.status.to_lowercase();
        if ["error", "no pude", "no se pudo"]
            .iter()
            .any(|marker| status.contains(marker))
        {
            FaceState::Error
        } else if self.speaking.load(Ordering::Relaxed) {
            FaceState::Speaking
        } else if self.recording {
            FaceState::Listening
        } else if self.request_running && status.contains("transcrib") {
            FaceState::Warmup
        } else if self.request_running {
            FaceState::Thinking
        } else {
            FaceState::Idle
        }
    }

    fn poll_hotkey(&mut self) {
        while let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().try_recv() {
            if self.avatar_toggle_registered
                && event.id == self.avatar_toggle_hotkey.id()
                && event.state == global_hotkey::HotKeyState::Pressed
            {
                self.avatar_visible = !self.avatar_visible;
                self.status = if self.avatar_visible {
                    "Personaje visible".to_string()
                } else {
                    "Personaje oculto".to_string()
                };
            } else if self.push_to_talk_registered && event.id == self.push_to_talk_hotkey.id() {
                match event.state {
                    global_hotkey::HotKeyState::Pressed
                        if !self.recording && !self.request_running =>
                    {
                        self.toggle_recording();
                    }
                    global_hotkey::HotKeyState::Released if self.recording => {
                        self.toggle_recording();
                    }
                    _ => {}
                }
            }
        }
    }

    fn send_prompt(&mut self, prompt: String) {
        if prompt.trim().is_empty() || self.request_running {
            return;
        }
        self.messages.push(("Tú".to_string(), prompt.clone()));
        self.request_running = true;
        self.status = "Pensando...".to_string();

        let agent = self.agent.clone();
        let tx = self.event_tx.clone();
        let voice = self.config.voice_name.clone();
        let speech_enabled = self.config.speech_enabled;
        let assistant_name = self.config.assistant_name.clone();
        let speaking = Arc::clone(&self.speaking);
        self.runtime.spawn(async move {
            match agent
                .respond_streaming(&prompt, |chunk| {
                    let _ = tx.send(AppEvent::ReplyChunk(chunk.to_string()));
                })
                .await
            {
                Ok(reply) => {
                    if speech_enabled {
                        speaking.store(true, Ordering::Relaxed);
                        let _ = tx.send(AppEvent::Speaking(true));
                        let _ = tx.send(AppEvent::Status(format!(
                            "{assistant_name} está hablando..."
                        )));
                        match speak(&reply, &voice).await {
                            Ok(()) => {
                                let _ = tx.send(AppEvent::Status("Listo".to_string()));
                            }
                            Err(err) => {
                                let _ = tx.send(AppEvent::Status(format!("Error de voz: {err:#}")));
                            }
                        }
                        speaking.store(false, Ordering::Relaxed);
                        let _ = tx.send(AppEvent::Speaking(false));
                    } else {
                        let _ = tx.send(AppEvent::Status("Listo".to_string()));
                    }
                    let _ = tx.send(AppEvent::Finished);
                }
                Err(err) => {
                    let _ = tx.send(AppEvent::Status(format!("Error del asistente: {err:#}")));
                    let _ = tx.send(AppEvent::Reply(format!("No pude responder: {err:#}")));
                    let _ = tx.send(AppEvent::Finished);
                }
            }
        });
    }

    fn toggle_recording(&mut self) {
        if self.request_running {
            return;
        }

        if self.recording {
            self.finish_recording();
        } else {
            self.start_recording(false);
        }
    }

    fn start_recording(&mut self, automatic: bool) {
        if self.request_running {
            return;
        }

        if self.audio.is_none()
            && let Err(err) = self.open_audio_input()
        {
            let message = format!("{err:#}");
            self.audio_error = Some(message.clone());
            self.status = message;
            return;
        }
        let Some(audio) = &self.audio else {
            self.status = "No se pudo iniciar el micrófono".to_string();
            return;
        };
        let start_result = if automatic {
            audio.start_with_pre_roll()
        } else {
            audio.start()
        };
        match start_result {
            Ok(()) => {
                self.recording = true;
                self.automatic_recording = automatic;
                self.live_transcript.clear();
                self.speech_detector.reset();
                if automatic {
                    self.status =
                        "Te escucho; responderé después de 2 segundos de silencio.".to_string();
                } else {
                    self.status = "Escuchando y transcribiendo en vivo...".to_string();
                    let source = audio.snapshot_source();
                    let transcriber = self.transcriber.clone();
                    let tx = self.event_tx.clone();
                    self.live_transcription = Some(self.runtime.spawn(async move {
                        transcribe_live_audio(source, transcriber, tx).await;
                    }));
                }
            }
            Err(err) => self.status = format!("Error al iniciar el micrófono: {err:#}"),
        }
    }

    fn finish_recording(&mut self) {
        if !self.recording {
            return;
        }
        self.recording = false;
        self.automatic_recording = false;
        self.speech_detector.reset();
        if let Some(worker) = self.live_transcription.take() {
            worker.abort();
        }
        self.live_transcript.clear();
        let Some(audio) = &self.audio else {
            return;
        };
        let finish_result = audio.finish();
        match finish_result {
            Ok(wav) => {
                if !self.auto_listening {
                    self.audio = None;
                }
                let path = wav.path().to_path_buf();
                let transcriber = self.transcriber.clone();
                let agent = self.agent.clone();
                let tx = self.event_tx.clone();
                let voice = self.config.voice_name.clone();
                let speech_enabled = self.config.speech_enabled;
                let assistant_name = self.config.assistant_name.clone();
                let speaking = Arc::clone(&self.speaking);
                self.request_running = true;
                self.status = "Transcribiendo...".to_string();
                self.runtime.spawn(async move {
                    let _wav = wav;
                    match transcriber
                        .transcribe(path.to_string_lossy().as_ref())
                        .await
                    {
                        Ok(transcript) => {
                            let _ = tx.send(AppEvent::UserTranscript(transcript.clone()));
                            let _ = tx.send(AppEvent::Status("Pensando...".to_string()));
                            match agent
                                .respond_streaming(&transcript, |chunk| {
                                    let _ = tx.send(AppEvent::ReplyChunk(chunk.to_string()));
                                })
                                .await
                            {
                                Ok(reply) => {
                                    if speech_enabled {
                                        speaking.store(true, Ordering::Relaxed);
                                        let _ = tx.send(AppEvent::Speaking(true));
                                        let _ = tx.send(AppEvent::Status(format!(
                                            "{assistant_name} está hablando..."
                                        )));
                                        match speak(&reply, &voice).await {
                                            Ok(()) => {
                                                let _ =
                                                    tx.send(AppEvent::Status("Listo".to_string()));
                                            }
                                            Err(err) => {
                                                let _ = tx.send(AppEvent::Status(format!(
                                                    "Error de voz: {err:#}"
                                                )));
                                            }
                                        }
                                        speaking.store(false, Ordering::Relaxed);
                                        let _ = tx.send(AppEvent::Speaking(false));
                                    } else {
                                        let _ = tx.send(AppEvent::Status("Listo".to_string()));
                                    }
                                    let _ = tx.send(AppEvent::Finished);
                                }
                                Err(err) => {
                                    let _ = tx.send(AppEvent::Status(format!(
                                        "Error del asistente: {err:#}"
                                    )));
                                    let _ = tx.send(AppEvent::Reply(format!(
                                        "No pude responder: {err:#}"
                                    )));
                                    let _ = tx.send(AppEvent::Finished);
                                }
                            }
                        }
                        Err(err) => {
                            let _ = tx
                                .send(AppEvent::Status(format!("Error de transcripción: {err:#}")));
                            let _ = tx
                                .send(AppEvent::Reply("No pude transcribir el audio.".to_string()));
                            let _ = tx.send(AppEvent::Finished);
                        }
                    }
                });
            }
            Err(err) => self.status = format!("Error al capturar audio: {err:#}"),
        }
    }

    fn reload_avatar(&mut self, ctx: &egui::Context) {
        let paths = match resolve_avatar_paths(&self.config) {
            Ok(paths) => paths,
            Err(err) => {
                self.status = format!("No se pudo encontrar la secuencia de caras: {err:#}");
                return;
            }
        };
        match load_avatar_textures(ctx, &paths) {
            Ok(textures) => {
                self.avatar_textures = textures;
                self.face_animation = FaceAnimation::new(&paths);
                self.status = format!("Secuencia actualizada ({} caras)", paths.len());
            }
            Err(err) => self.status = format!("No se pudo cargar la secuencia: {err:#}"),
        }
    }

    fn choose_avatar_sequence(&mut self, ctx: &egui::Context) {
        let Some(mut paths) = rfd::FileDialog::new()
            .add_filter("Imágenes", &["png", "jpg", "jpeg"])
            .pick_files()
        else {
            return;
        };
        paths.sort_by_key(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
        });

        match load_avatar_textures(ctx, &paths) {
            Ok(textures) => match self.config.save_avatar_sequence(paths.clone()) {
                Ok(()) => {
                    self.avatar_textures = textures;
                    self.face_animation = FaceAnimation::new(&paths);
                    self.status = format!("Secuencia guardada ({} caras)", paths.len());
                }
                Err(err) => self.status = format!("No se pudo guardar la secuencia: {err:#}"),
            },
            Err(err) => self.status = format!("No se pudo cargar la secuencia: {err:#}"),
        }
    }
}

fn load_avatar_textures(
    ctx: &egui::Context,
    paths: &[PathBuf],
) -> Result<Vec<egui::TextureHandle>> {
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let image = image::open(path)
                .with_context(|| format!("No se pudo abrir {}", path.display()))?
                .to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let color_image = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
            Ok(ctx.load_texture(
                format!("assistant-avatar-{index}"),
                color_image,
                egui::TextureOptions::LINEAR,
            ))
        })
        .collect()
}

fn configured_avatar_paths(config: &AgentConfig) -> Vec<PathBuf> {
    if config.avatar_sequence.is_empty() {
        vec![config.avatar_file.clone()]
    } else {
        config.avatar_sequence.clone()
    }
}

fn resolve_avatar_paths(config: &AgentConfig) -> Result<Vec<PathBuf>> {
    let configured = configured_avatar_paths(config);
    if !config.avatar_sequence.is_empty() {
        return Ok(configured);
    }

    let config_dir = config
        .config_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let faces_dir = config_dir.join("FACES");
    if !faces_dir.is_dir() {
        return Ok(configured);
    }

    let faces = read_face_paths(&faces_dir)?;

    if faces.is_empty() {
        Ok(configured)
    } else {
        Ok(faces)
    }
}

fn read_face_paths(faces_dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(faces_dir)
        .with_context(|| format!("No se pudo leer {}", faces_dir.display()))?;
    let mut faces = entries
        .map(|entry| {
            entry
                .with_context(|| format!("No se pudo leer una entrada de {}", faces_dir.display()))
                .map(|entry| entry.path())
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| {
            path.is_file()
                && path.extension().is_some_and(|ext| {
                    matches!(
                        ext.to_string_lossy().to_lowercase().as_str(),
                        "png" | "jpg" | "jpeg"
                    )
                })
        })
        .collect::<Vec<_>>();
    faces.sort_by_key(|path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    });
    Ok(faces)
}

async fn transcribe_live_audio(
    source: AudioSnapshotSource,
    transcriber: WhisperTranscriber,
    tx: Sender<AppEvent>,
) {
    let mut last_sample_count = 0usize;
    let mut last_transcript = String::new();
    let mut interval = tokio::time::interval(Duration::from_millis(1800));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;

    loop {
        interval.tick().await;
        let sample_count = match source.sample_count() {
            Ok(count) => count,
            Err(err) => {
                let _ = tx.send(AppEvent::Status(format!(
                    "Error leyendo audio del micrófono: {err:#}"
                )));
                return;
            }
        };
        let minimum_samples = 16_000 * 2;
        if sample_count < minimum_samples || sample_count.saturating_sub(last_sample_count) < 16_000
        {
            continue;
        }
        last_sample_count = sample_count;

        let wav = match source.snapshot_wav() {
            Ok(wav) => wav,
            Err(err) => {
                let _ = tx.send(AppEvent::Status(format!(
                    "Error preparando el audio en vivo: {err:#}"
                )));
                return;
            }
        };
        match transcriber.transcribe(&wav.path().to_string_lossy()).await {
            Ok(transcript) if !transcript.is_empty() && transcript != last_transcript => {
                last_transcript = transcript.clone();
                if tx.send(AppEvent::LiveTranscript(transcript)).is_err() {
                    return;
                }
            }
            Ok(_) => {}
            Err(err) if err.to_string().contains("no devolvió una transcripción") => {}
            Err(err) => {
                let _ = tx.send(AppEvent::Status(format!(
                    "Error de reconocimiento en vivo: {err:#}"
                )));
                return;
            }
        }
    }
}

impl eframe::App for DesktopApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::BLACK;
        visuals.window_fill = egui::Color32::from_rgb(12, 12, 12);
        visuals.extreme_bg_color = egui::Color32::from_rgb(18, 18, 18);
        ctx.set_visuals(visuals);

        self.poll_hotkey();
        self.poll_events();
        self.poll_audio_error();
        self.poll_automatic_speech();

        let mut toggle_fullscreen =
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F11));
        let escape_pressed =
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if self.fullscreen && escape_pressed {
            toggle_fullscreen = true;
        } else if self.chat_visible && escape_pressed {
            self.chat_visible = false;
        }

        if !self.fullscreen
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F))
        {
            self.chat_visible = !self.chat_visible;
            self.focus_input = self.chat_visible;
        }

        let mut prompt_to_send = None;
        let mut choose_sequence = false;
        let mut toggle_recording = false;
        let mut toggle_auto_listening = false;
        let mut toggle_avatar = false;
        let selected_input_device_before = self.selected_input_device.clone();
        let avatar_size_before = self.config.avatar_size;
        let avatar_height_before = self.config.avatar_height;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                if self.fullscreen {
                    ui.centered_and_justified(|ui| {
                        if self.avatar_visible {
                            let time = ctx.input(|input| input.time);
                            let frame_index = self.face_animation.frame(time, self.face_state());
                            if let Some(texture) = self.avatar_textures.get(frame_index) {
                                let available = ui.available_size();
                                let image_size = texture.size_vec2();
                                let scale =
                                    (available.x / image_size.x).min(available.y / image_size.y);
                                ui.add(
                                    egui::Image::new(texture).fit_to_exact_size(image_size * scale),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("🙂")
                                        .size(
                                            ui.available_size().y.min(ui.available_size().x) * 0.75,
                                        )
                                        .color(egui::Color32::LIGHT_BLUE),
                                );
                            }
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.heading(&self.config.assistant_name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Chat  Ctrl+F").clicked() {
                                self.chat_visible = true;
                                self.focus_input = true;
                            }
                            if ui.button("Elegir caras").clicked() {
                                choose_sequence = true;
                            }
                            if ui
                                .button(if self.fullscreen {
                                    "Salir de pantalla completa"
                                } else {
                                    "Pantalla completa"
                                })
                                .clicked()
                            {
                                toggle_fullscreen = true;
                            }
                            if ui
                                .button(if self.recording {
                                    "Enviar voz"
                                } else {
                                    "Hablar"
                                })
                                .clicked()
                            {
                                toggle_recording = true;
                            }
                        });
                    });
                    ui.separator();

                    let available = ui.available_size();
                    let max_face_size = egui::vec2(
                        available.x.clamp(100.0, 512.0),
                        (available.y - 100.0).clamp(100.0, 768.0),
                    );
                    let face_size = egui::vec2(
                        self.config
                            .avatar_size
                            .clamp(100.0, 512.0)
                            .min(max_face_size.x),
                        self.config
                            .avatar_height
                            .clamp(100.0, 768.0)
                            .min(max_face_size.y),
                    );
                    let vertical_space = ((available.y - face_size.y - 70.0) / 2.0).max(0.0);
                    ui.add_space(vertical_space);
                    ui.vertical_centered(|ui| {
                        if self.avatar_visible {
                            let time = ctx.input(|input| input.time);
                            let face_state = self.face_state();
                            let frame_index = self.face_animation.frame(time, face_state);
                            let resized_size = egui::Resize::default()
                                .id_salt("avatar-face-resize")
                                .default_size(face_size)
                                .min_size(egui::vec2(100.0, 100.0))
                                .max_size(max_face_size)
                                .with_stroke(false)
                                .show(ui, |ui| {
                                    let size = ui.available_size().max(egui::vec2(1.0, 1.0));
                                    if let Some(texture) = self.avatar_textures.get(frame_index) {
                                        ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                                    } else {
                                        ui.centered_and_justified(|ui| {
                                            ui.label(
                                                egui::RichText::new("🙂")
                                                    .size(size.y.min(size.x) * 0.75)
                                                    .color(egui::Color32::LIGHT_BLUE),
                                            );
                                        });
                                    }
                                    size
                                });
                            self.config.avatar_size = resized_size.x.clamp(64.0, 512.0);
                            self.config.avatar_height = resized_size.y.clamp(64.0, 768.0);
                            if face_state == FaceState::Speaking && self.avatar_textures.len() > 1 {
                                ui.label(format!(
                                    "Expresión {} de {}",
                                    frame_index + 1,
                                    self.avatar_textures.len()
                                ));
                            }
                            ui.label("Arrastra la esquina inferior derecha para estirar la cara");
                        } else {
                            ui.label(
                                egui::RichText::new("Personaje oculto").color(egui::Color32::GRAY),
                            );
                        }
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(&self.status).color(egui::Color32::GRAY));
                    });

                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .button(if self.avatar_visible {
                                    "Ocultar cara"
                                } else {
                                    "Mostrar cara"
                                })
                                .clicked()
                            {
                                toggle_avatar = true;
                            }
                            ui.label(format!("{} imagen(es)", self.avatar_textures.len()));
                        });
                    });
                }
            });

        if self.chat_visible && !self.fullscreen {
            let mut open = true;
            let mut focus_requested = self.focus_input;
            let screen_size = ctx.input(|input| input.screen_rect().size());
            let max_chat_size = egui::vec2(
                (screen_size.x - 24.0).max(220.0),
                (screen_size.y - 24.0).max(180.0),
            );
            let default_chat_size = egui::vec2(
                520.0_f32.min(max_chat_size.x),
                360.0_f32.min(max_chat_size.y),
            );
            let min_chat_size = egui::vec2(
                280.0_f32.min(max_chat_size.x),
                180.0_f32.min(max_chat_size.y),
            );
            egui::Window::new(format!("Chat con {}", self.config.assistant_name))
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .default_size(default_chat_size)
                .min_size(min_chat_size)
                .max_size(max_chat_size)
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(12, 12, 12))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            egui::Color32::from_rgb(70, 70, 70),
                        )),
                )
                .show(ctx, |ui| {
                    if self.recording || !self.live_transcript.is_empty() {
                        ui.strong(if self.recording {
                            "Transcripción en vivo:"
                        } else {
                            "Último fragmento reconocido:"
                        });
                        ui.label(if self.live_transcript.is_empty() {
                            "Habla; el texto aparecerá aquí mientras Mondo escucha."
                        } else {
                            &self.live_transcript
                        });
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .stick_to_bottom(true)
                        .max_height((ui.available_height() - 125.0).max(60.0))
                        .show(ui, |ui| {
                            for (speaker, text) in &self.messages {
                                ui.strong(format!("{speaker}:"));
                                ui.label(text);
                                ui.add_space(8.0);
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!self.request_running, |ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.input)
                                    .hint_text("Escribe un mensaje...")
                                    .desired_width(f32::INFINITY),
                            );
                            if focus_requested {
                                response.request_focus();
                                focus_requested = false;
                            }
                            if response.has_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                prompt_to_send = Some(std::mem::take(&mut self.input));
                            }
                        });
                        if ui
                            .add_enabled(!self.request_running, egui::Button::new("Enviar"))
                            .clicked()
                        {
                            prompt_to_send = Some(std::mem::take(&mut self.input));
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.request_running,
                                egui::Button::new(if self.recording {
                                    "Enviar voz"
                                } else {
                                    "Hablar"
                                }),
                            )
                            .clicked()
                        {
                            toggle_recording = true;
                        }
                        if ui
                            .add_enabled(
                                !self.recording && !self.request_running,
                                egui::Button::new(if self.auto_listening {
                                    "Pausar escucha automática"
                                } else {
                                    "Reanudar escucha"
                                }),
                            )
                            .clicked()
                        {
                            toggle_auto_listening = true;
                        }
                        if ui.button("Recargar caras").clicked() {
                            self.reload_avatar(ctx);
                        }
                        ui.label(format!("Modelo: {}", self.config.model));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Micrófono:");
                        ui.add_enabled_ui(!self.recording, |ui| {
                            egui::ComboBox::from_id_salt("input-device")
                                .selected_text(if self.selected_input_device.is_empty() {
                                    default_input_device_label()
                                } else {
                                    self.selected_input_device.as_str()
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.selected_input_device,
                                        String::new(),
                                        default_input_device_label(),
                                    );
                                    for device in &self.input_devices {
                                        ui.selectable_value(
                                            &mut self.selected_input_device,
                                            device.clone(),
                                            device,
                                        );
                                    }
                                });
                        });
                        if ui.button("Actualizar lista").clicked() {
                            match input_device_names() {
                                Ok(devices) => {
                                    self.input_devices = devices;
                                    self.audio_error = None;
                                }
                                Err(err) => self.audio_error = Some(format!("{err:#}")),
                            }
                        }
                    });
                    if (self.recording || self.auto_listening)
                        && let Some(audio) = &self.audio
                    {
                        ui.add(egui::ProgressBar::new(audio.input_level()).text(
                            if self.recording {
                                "Nivel del micrófono"
                            } else {
                                "Nivel · habla para activar"
                            },
                        ));
                    }
                    if let Some(error) = &self.audio_error {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("Micrófono: {error}"));
                    }
                    if let Some(error) = &self.hotkey_registration_error {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!("Atajos globales: {error}"),
                        );
                    }
                    ui.label(format!(
                        "Atajo de voz: {} · Esc cierra este panel",
                        self.config.push_to_talk_hotkey
                    ));
                });
            self.chat_visible = open;
            self.focus_input = focus_requested;
        }

        if toggle_recording {
            self.toggle_recording();
        }
        if selected_input_device_before != self.selected_input_device && self.auto_listening {
            self.audio = None;
            self.speech_detector.reset();
            match self.open_audio_input() {
                Ok(()) => self.status = "Escucha automática activa. Habla cuando quieras.".into(),
                Err(err) => {
                    let message = format!("{err:#}");
                    self.audio_error = Some(message.clone());
                    self.status = format!("No se pudo activar la escucha automática: {message}");
                    self.auto_listening = false;
                }
            }
        }
        if toggle_auto_listening {
            self.toggle_auto_listening();
        }
        if toggle_avatar {
            self.avatar_visible = !self.avatar_visible;
        }
        if toggle_fullscreen {
            self.fullscreen = !self.fullscreen;
            if self.fullscreen {
                self.chat_visible = false;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if choose_sequence {
            self.choose_avatar_sequence(ctx);
        }
        if (self.config.avatar_size - avatar_size_before).abs() >= 0.5
            || (self.config.avatar_height - avatar_height_before).abs() >= 0.5
        {
            self.avatar_size_save_at = Some(ctx.input(|input| input.time) + 0.5);
        }
        if self
            .avatar_size_save_at
            .is_some_and(|save_at| ctx.input(|input| input.time) >= save_at)
        {
            self.avatar_size_save_at = None;
            if let Err(err) = self.config.save_avatar_size() {
                self.status = format!("No se pudo guardar el tamaño de la cara: {err:#}");
            }
        }
        if let Some(prompt) = prompt_to_send {
            self.send_prompt(prompt);
        }
        let animation_active = self.avatar_visible
            && self.avatar_textures.len() > 1
            && !matches!(
                self.face_state(),
                FaceState::Idle | FaceState::Error | FaceState::Warmup
            );
        if self.auto_listening || self.recording || self.request_running || animation_active {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod automatic_speech_tests {
    use super::{AutomaticSpeechDetector, VOICE_ACTIVITY_THRESHOLD, VoiceActivity};
    use std::time::{Duration, Instant};

    #[test]
    fn starts_only_after_sustained_speech() {
        let start = Instant::now();
        let mut detector = AutomaticSpeechDetector::default();

        assert_eq!(
            detector.update(start, VOICE_ACTIVITY_THRESHOLD, false),
            None
        );
        assert_eq!(
            detector.update(
                start + Duration::from_millis(149),
                VOICE_ACTIVITY_THRESHOLD,
                false
            ),
            None
        );
        assert_eq!(
            detector.update(
                start + Duration::from_millis(150),
                VOICE_ACTIVITY_THRESHOLD,
                false
            ),
            Some(VoiceActivity::Started)
        );
    }

    #[test]
    fn finishes_after_two_seconds_of_silence_and_resets_if_speech_resumes() {
        let start = Instant::now();
        let mut detector = AutomaticSpeechDetector::default();

        assert_eq!(detector.update(start, 0.0, true), None);
        assert_eq!(
            detector.update(
                start + Duration::from_secs(2) - Duration::from_millis(1),
                0.0,
                true
            ),
            None
        );
        assert_eq!(
            detector.update(
                start + Duration::from_secs(2),
                VOICE_ACTIVITY_THRESHOLD,
                true
            ),
            None
        );
        assert_eq!(
            detector.update(
                start + Duration::from_secs(2) + Duration::from_millis(1),
                0.0,
                true
            ),
            None
        );
        assert_eq!(
            detector.update(start + Duration::from_secs(4), 0.0, true),
            None
        );
        assert_eq!(
            detector.update(
                start + Duration::from_secs(4) + Duration::from_millis(1),
                0.0,
                true
            ),
            Some(VoiceActivity::Finished)
        );
    }
}

#[cfg(test)]
mod face_animation_tests {
    use super::{FaceAnimation, FaceState};
    use std::path::{Path, PathBuf};

    #[test]
    fn uses_named_idle_blink_talk_and_breath_faces_at_deliberate_intervals() {
        let paths = [
            "00-normal.png",
            "01-parpadeo.png",
            "02-talk-closed.png",
            "03-talk-open.png",
            "04-breath.png",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
        let mut animation = FaceAnimation::new(&paths);

        assert_eq!(animation.frame(0.0, FaceState::Idle), 0);
        assert_eq!(animation.frame(3.5, FaceState::Idle), 1);
        assert_eq!(animation.frame(3.7, FaceState::Idle), 0);
        assert_eq!(animation.frame(10.0, FaceState::Speaking), 2);
        assert_eq!(animation.frame(12.1, FaceState::Speaking), 2);
        assert_eq!(animation.frame(12.2, FaceState::Speaking), 3);
        assert_eq!(animation.frame(13.9, FaceState::Speaking), 4);
        assert_eq!(animation.frame(14.2, FaceState::Speaking), 4);
        assert_eq!(animation.frame(14.5, FaceState::Speaking), 2);
        assert_eq!(animation.frame(15.0, FaceState::Idle), 0);
    }

    #[test]
    fn animates_each_named_application_state() {
        let paths = [
            "idle 01.png",
            "capturing 01.png",
            "listen 01.png",
            "listen 02.png",
            "thinking 01.png",
            "thinking 02.png",
            "speaking 01.png",
            "speaking 02.png",
            "error 01.png",
            "warmup 01.png",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
        let mut animation = FaceAnimation::new(&paths);

        assert_eq!(animation.frame(0.0, FaceState::Listening), 1);
        assert_eq!(animation.frame(0.6, FaceState::Listening), 2);
        assert_eq!(animation.frame(1.3, FaceState::Listening), 3);
        assert_eq!(animation.frame(2.0, FaceState::Thinking), 4);
        assert_eq!(animation.frame(2.5, FaceState::Thinking), 5);
        assert_eq!(animation.frame(3.0, FaceState::Speaking), 6);
        assert_eq!(animation.frame(5.1, FaceState::Speaking), 6);
        assert_eq!(animation.frame(5.2, FaceState::Speaking), 7);
        assert_eq!(animation.frame(3.6, FaceState::Error), 8);
        assert_eq!(animation.frame(3.9, FaceState::Warmup), 9);
    }

    #[test]
    fn loads_and_routes_the_faces_folder_assets() {
        let faces_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("FACES");
        let paths = super::read_face_paths(&faces_dir).unwrap();
        for path in &paths {
            image::open(path).unwrap();
        }
        let names = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>();
        let mut animation = FaceAnimation::new(&paths);

        assert_eq!(
            names,
            [
                "capturing 01.png",
                "error 01.png",
                "idle 01.png",
                "listen 01.png",
                "listen 02.png",
                "speaking 01.png",
                "speaking 02.png",
                "speaking 03.png",
                "thinking 01.png",
                "thinking 02.png",
                "thinking 03.png",
                "thinking 04.png",
                "warmup 01.png",
            ]
        );
        assert_eq!(names[animation.frame(0.0, FaceState::Idle)], "idle 01.png");
        assert_eq!(
            names[animation.frame(1.0, FaceState::Listening)],
            "capturing 01.png"
        );
        assert_eq!(
            names[animation.frame(2.0, FaceState::Thinking)],
            "thinking 01.png"
        );
        assert_eq!(
            names[animation.frame(3.0, FaceState::Speaking)],
            "speaking 01.png"
        );
        assert_eq!(
            names[animation.frame(4.0, FaceState::Warmup)],
            "warmup 01.png"
        );
        assert_eq!(
            names[animation.frame(5.0, FaceState::Error)],
            "error 01.png"
        );
    }
}
