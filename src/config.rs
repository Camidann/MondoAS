//bro

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use global_hotkey::hotkey::HotKey;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub assistant_name: String,
    pub ollama_url: String,
    pub model: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub context_size: u32,
    pub input_device: String,
    pub push_to_talk_hotkey: String,
    pub avatar_toggle_hotkey: String,
    pub whisper_language: String,
    pub whisper_prompt: String,
    pub whisper_threads: usize,
    pub whisper_beam_size: usize,
    pub whisper_server_bin: PathBuf,
    pub whisper_model: PathBuf,
    pub piper_model: PathBuf,
    pub avatar_file: PathBuf,
    pub avatar_sequence: Vec<PathBuf>,
    pub avatar_enabled: bool,
    pub avatar_size: f32,
    pub avatar_height: f32,
    pub avatar_speed: f32,
    pub voice_name: String,
    pub speech_enabled: bool,
    pub config_path: PathBuf,
}

impl AgentConfig {
    pub fn load() -> Result<Self> {
        let exe = std::env::current_exe().context("No se pudo localizar el ejecutable")?;
        let executable_dir = exe
            .parent()
            .context("El ejecutable no tiene carpeta contenedora")?
            .to_path_buf();
        let config_path = config_path_for_executable(&executable_dir);

        if !config_path.exists() {
            fs::write(&config_path, Self::template())
                .with_context(|| format!("No se pudo crear {}", config_path.display()))?;
        }

        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("No se pudo leer {}", config_path.display()))?;
        let config_dir = config_path
            .parent()
            .unwrap_or(&executable_dir)
            .to_path_buf();
        Self::parse(&contents, config_dir, config_path.clone())
            .with_context(|| format!("Revisa la configuración de {}", config_path.display()))
    }

    pub fn save_avatar_sequence(&mut self, sequence: Vec<PathBuf>) -> Result<()> {
        if sequence.is_empty() {
            bail!("La secuencia de caras debe tener al menos una imagen");
        }

        let avatar_file = sequence[0].clone();
        let config_dir = self.config_path.parent().unwrap_or_else(|| Path::new("."));
        let stored_avatar_file = avatar_file.strip_prefix(config_dir).unwrap_or(&avatar_file);
        let sequence_value = sequence
            .iter()
            .map(|path| {
                path.strip_prefix(config_dir)
                    .unwrap_or(path)
                    .to_string_lossy()
            })
            .collect::<Vec<_>>()
            .join("|");
        let contents = fs::read_to_string(&self.config_path)
            .with_context(|| format!("No se pudo leer {}", self.config_path.display()))?;
        let mut has_avatar_file = false;
        let mut has_avatar_sequence = false;
        let mut updated_lines = Vec::new();
        for line in contents.lines() {
            let key = line
                .split_once('=')
                .map(|(key, _)| key.trim())
                .unwrap_or_default();
            match key {
                "avatar_file" => {
                    updated_lines.push(format!("avatar_file={}", stored_avatar_file.display()));
                    has_avatar_file = true;
                }
                "avatar_sequence" => {
                    updated_lines.push(format!("avatar_sequence={sequence_value}"));
                    has_avatar_sequence = true;
                }
                _ => updated_lines.push(line.to_string()),
            }
        }
        if !has_avatar_file {
            updated_lines.push(format!("avatar_file={}", stored_avatar_file.display()));
        }
        if !has_avatar_sequence {
            updated_lines.push(format!("avatar_sequence={sequence_value}"));
        }
        fs::write(&self.config_path, updated_lines.join("\n") + "\n")
            .with_context(|| format!("No se pudo guardar {}", self.config_path.display()))?;

        self.avatar_file = avatar_file;
        self.avatar_sequence = sequence;
        Ok(())
    }

    pub fn save_avatar_size(&self) -> Result<()> {
        let contents = fs::read_to_string(&self.config_path)
            .with_context(|| format!("No se pudo leer {}", self.config_path.display()))?;
        let mut has_avatar_size = false;
        let mut has_avatar_height = false;
        let updated_lines = contents
            .lines()
            .map(
                |line| match line.split_once('=').map(|(key, _)| key.trim()) {
                    Some("avatar_size") => {
                        has_avatar_size = true;
                        format!("avatar_size={}", self.avatar_size)
                    }
                    Some("avatar_height") => {
                        has_avatar_height = true;
                        format!("avatar_height={}", self.avatar_height)
                    }
                    _ => line.to_string(),
                },
            )
            .collect::<Vec<_>>();
        let mut updated_contents = updated_lines.join("\n");
        if !has_avatar_size {
            if !updated_contents.is_empty() {
                updated_contents.push('\n');
            }
            updated_contents.push_str(&format!("avatar_size={}", self.avatar_size));
        }
        if !has_avatar_height {
            if !updated_contents.is_empty() {
                updated_contents.push('\n');
            }
            updated_contents.push_str(&format!("avatar_height={}", self.avatar_height));
        }
        updated_contents.push('\n');
        fs::write(&self.config_path, updated_contents)
            .with_context(|| format!("No se pudo guardar {}", self.config_path.display()))
    }

    fn parse(contents: &str, config_dir: PathBuf, config_path: PathBuf) -> Result<Self> {
        let mut config = Self::defaults();
        config.config_path = config_path;
        config.avatar_file = resolve_path(&config_dir, &config.avatar_file.to_string_lossy());
        config.whisper_server_bin = resolve_whisper_path(
            &config_dir,
            &config.whisper_server_bin.to_string_lossy(),
            true,
        );
        config.whisper_model =
            resolve_whisper_path(&config_dir, &config.whisper_model.to_string_lossy(), false);
        config.piper_model = resolve_path(&config_dir, &config.piper_model.to_string_lossy());

        for (line_index, raw_line) in contents.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .with_context(|| format!("Línea {}: se esperaba clave=valor", line_index + 1))?;
            let key = key.trim();
            let value = value.trim();
            match key {
                "assistant_name" => config.assistant_name = value.to_string(),
                "ollama_url" => config.ollama_url = value.trim_end_matches('/').to_string(),
                "model" => config.model = value.to_string(),
                "system_prompt" => config.system_prompt = value.to_string(),
                "temperature" => {
                    config.temperature = value.parse().with_context(|| {
                        format!("Línea {}: temperature debe ser un número", line_index + 1)
                    })?
                }
                "context_size" => {
                    config.context_size = value.parse().with_context(|| {
                        format!("Línea {}: context_size debe ser un entero", line_index + 1)
                    })?
                }
                "input_device" => config.input_device = value.to_string(),
                "push_to_talk_hotkey" => config.push_to_talk_hotkey = value.to_string(),
                "avatar_toggle_hotkey" => config.avatar_toggle_hotkey = value.to_string(),
                "whisper_language" => config.whisper_language = value.to_string(),
                "whisper_prompt" => config.whisper_prompt = value.to_string(),
                "whisper_threads" => {
                    config.whisper_threads = value.parse().with_context(|| {
                        format!(
                            "Línea {}: whisper_threads debe ser un entero",
                            line_index + 1
                        )
                    })?
                }
                "whisper_beam_size" => {
                    config.whisper_beam_size = value.parse().with_context(|| {
                        format!(
                            "Línea {}: whisper_beam_size debe ser un entero",
                            line_index + 1
                        )
                    })?
                }
                "whisper_server_bin" => {
                    config.whisper_server_bin = resolve_whisper_path(&config_dir, value, true)
                }
                "whisper_model" => {
                    config.whisper_model = resolve_whisper_path(&config_dir, value, false)
                }
                "piper_model" => config.piper_model = resolve_path(&config_dir, value),
                "avatar_file" => config.avatar_file = resolve_path(&config_dir, value),
                "avatar_sequence" => {
                    config.avatar_sequence = if value.is_empty() {
                        Vec::new()
                    } else {
                        value
                            .split('|')
                            .map(|path| resolve_path(&config_dir, path))
                            .collect()
                    }
                }
                "avatar_enabled" => config.avatar_enabled = parse_bool(value, key, line_index + 1)?,
                "avatar_size" => {
                    config.avatar_size = value.parse().with_context(|| {
                        format!("Línea {}: avatar_size debe ser un número", line_index + 1)
                    })?
                }
                "avatar_height" => {
                    config.avatar_height = value.parse().with_context(|| {
                        format!("Línea {}: avatar_height debe ser un número", line_index + 1)
                    })?
                }
                "avatar_speed" => {
                    config.avatar_speed = value.parse().with_context(|| {
                        format!("Línea {}: avatar_speed debe ser un número", line_index + 1)
                    })?
                }
                "voice_name" => config.voice_name = value.to_string(),
                "speech_enabled" => config.speech_enabled = parse_bool(value, key, line_index + 1)?,
                _ => bail!("Línea {}: opción desconocida `{key}`", line_index + 1),
            }
        }

        if config.assistant_name.is_empty() || config.model.is_empty() {
            bail!("assistant_name y model no pueden estar vacíos");
        }
        if !(0.0..=2.0).contains(&config.temperature) {
            bail!("temperature debe estar entre 0 y 2");
        }
        if !(128..=131_072).contains(&config.context_size) {
            bail!("context_size debe estar entre 128 y 131072");
        }
        if !(64.0..=512.0).contains(&config.avatar_size) {
            bail!("avatar_size debe estar entre 64 y 512");
        }
        if !(64.0..=768.0).contains(&config.avatar_height) {
            bail!("avatar_height debe estar entre 64 y 768");
        }
        if !(0.0..=5.0).contains(&config.avatar_speed) {
            bail!("avatar_speed debe estar entre 0 y 5");
        }
        if config.ollama_url.is_empty() {
            bail!("ollama_url no puede estar vacío");
        }
        if config.whisper_language.is_empty() {
            bail!("whisper_language no puede estar vacío; usa `auto` para detección automática");
        }
        if !(1..=16).contains(&config.whisper_threads) {
            bail!("whisper_threads debe estar entre 1 y 16");
        }
        if !(1..=5).contains(&config.whisper_beam_size) {
            bail!("whisper_beam_size debe estar entre 1 y 5");
        }
        let push_to_talk = HotKey::from_str(&config.push_to_talk_hotkey)
            .context("push_to_talk_hotkey no es una combinación válida")?;
        let avatar_toggle = HotKey::from_str(&config.avatar_toggle_hotkey)
            .context("avatar_toggle_hotkey no es una combinación válida")?;
        if push_to_talk.id() == avatar_toggle.id() {
            bail!("push_to_talk_hotkey y avatar_toggle_hotkey deben ser diferentes");
        }

        Ok(config)
    }

    fn defaults() -> Self {
        Self {
            assistant_name: "Mondo".to_string(),
            ollama_url: "http://localhost:11434".to_string(),
            model: "qwen2.5:3b".to_string(),
            system_prompt: "Sos un asistente amable y útil. Responde en el idioma del usuario."
                .to_string(),
            temperature: 0.7,
            context_size: 4096,
            input_device: String::new(),
            push_to_talk_hotkey: "CTRL+ALT+SPACE".to_string(),
            avatar_toggle_hotkey: "ALT+SHIFT+P".to_string(),
            whisper_language: "es".to_string(),
            whisper_prompt: "Español rioplatense. Abrime, abrí, abrí me, abrir, cerrame, cerrá, cerrá me, cerrar, Spotify, WhatsApp, YouTube, Chrome, Firefox, Discord, Steam, Zoom, Teams, VS Code".to_string(),
            whisper_threads: default_whisper_threads(),
            whisper_beam_size: default_whisper_beam_size(),
            whisper_server_bin: default_whisper_server_path(),
            whisper_model: PathBuf::from("whisper").join(default_whisper_model_filename()),
            piper_model: PathBuf::from("es_MX-ald-medium.onnx"),
            avatar_file: PathBuf::from("avatar.png"),
            avatar_sequence: Vec::new(),
            avatar_enabled: true,
            avatar_size: 320.0,
            avatar_height: 320.0,
            avatar_speed: 0.0,
            voice_name: String::new(),
            speech_enabled: false,
            config_path: PathBuf::new(),
        }
    }

    fn template() -> String {
        format!(
            "# Personaliza el nombre, modelo, voz y apariencia de tu asistente.\n\
         # Las rutas relativas se resuelven desde la carpeta de este archivo.\n\
         assistant_name=Mondo\n\
         ollama_url=http://localhost:11434\n\
         model=qwen2.5:3b\n\
         system_prompt=Sos un asistente amable y útil. Responde en el idioma del usuario.\n\
         temperature=0.7\n\
         context_size=4096\n\
         input_device=\n\
         push_to_talk_hotkey=CTRL+ALT+SPACE\n\
         avatar_toggle_hotkey=ALT+SHIFT+P\n\
         whisper_language=es\n\
         whisper_prompt=Español rioplatense. Abrime, abrí, abrí me, abrir, cerrame, cerrá, cerrá me, cerrar, Spotify, WhatsApp, YouTube, Chrome, Firefox, Discord, Steam, Zoom, Teams, VS Code\n\
         whisper_threads={}\n\
         whisper_beam_size={}\n\
         whisper_server_bin={}\n\
         whisper_model=whisper\\{}\n\
         piper_model=es_MX-ald-medium.onnx\n\
         avatar_file=avatar.png\n\
         avatar_sequence=\n\
         avatar_enabled=true\n\
         avatar_size=320\n\
         avatar_height=320\n\
         avatar_height=320\n\
         avatar_speed=0.0\n\
         voice_name=\n\
         speech_enabled=false\n",
            default_whisper_threads(),
            default_whisper_beam_size(),
            default_whisper_server_config_value(),
            default_whisper_model_filename()
        )
    }
}

fn resolve_path(base: &Path, path: &str) -> PathBuf {
    #[cfg(target_os = "linux")]
    let path = PathBuf::from(path.replace('\\', "/"));
    #[cfg(not(target_os = "linux"))]
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn default_whisper_server_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from("whisper\\whisper-server.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("whisper").join("whisper-server")
    }
}

fn default_whisper_threads() -> usize {
    if cfg!(any(target_arch = "arm", target_arch = "aarch64")) {
        2
    } else {
        4
    }
}

fn default_whisper_beam_size() -> usize {
    if cfg!(any(target_arch = "arm", target_arch = "aarch64")) {
        1
    } else {
        3
    }
}

fn default_whisper_model_filename() -> &'static str {
    if cfg!(any(target_arch = "arm", target_arch = "aarch64")) {
        "ggml-tiny-q5_1.bin"
    } else {
        "ggml-base-q5_1.bin"
    }
}

fn default_whisper_server_config_value() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        r"whisper\whisper-server.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "whisper/whisper-server"
    }
}

fn config_path_for_executable(executable_dir: &Path) -> PathBuf {
    let is_cargo_build_dir = matches!(
        executable_dir.file_name().and_then(|name| name.to_str()),
        Some("debug" | "release")
    ) && executable_dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        == Some("target");

    if is_cargo_build_dir && let Some(project_dir) = executable_dir.parent().and_then(Path::parent)
    {
        let project_config = project_dir.join("config.txt");
        if project_config.is_file() {
            return project_config;
        }
    }

    executable_dir.join("config.txt")
}

fn resolve_whisper_path(base: &Path, configured: &str, executable: bool) -> PathBuf {
    #[cfg(target_os = "linux")]
    let configured_path = {
        let mut path = resolve_path(base, configured);
        if executable
            && matches!(
                configured.replace('\\', "/").as_str(),
                "whisper/whisper-server.exe"
            )
        {
            path.set_extension("");
        }
        path
    };
    #[cfg(not(target_os = "linux"))]
    let configured_path = resolve_path(base, configured);
    if configured_path.exists() {
        return configured_path;
    }

    let is_default_path = if executable {
        matches!(
            configured.replace('\\', "/").as_str(),
            "whisper/whisper-server" | "whisper/whisper-server.exe"
        )
    } else {
        matches!(
            configured.replace('\\', "/").as_str(),
            "whisper/ggml-base-q5_1.bin" | "whisper/ggml-tiny-q5_1.bin"
        )
    };
    if !is_default_path {
        return configured_path;
    }

    for ancestor in base.ancestors() {
        let candidates = if executable {
            vec![
                ancestor
                    .join("src")
                    .join("whisper.cpp")
                    .join("build")
                    .join("bin")
                    .join("Release")
                    .join(if cfg!(windows) {
                        "whisper-server.exe"
                    } else {
                        "whisper-server"
                    }),
                ancestor
                    .join("src")
                    .join("whisper.cpp")
                    .join("build")
                    .join("bin")
                    .join(if cfg!(windows) {
                        "whisper-server.exe"
                    } else {
                        "whisper-server"
                    }),
            ]
        } else {
            vec![
                ancestor
                    .join("src")
                    .join("whisper.cpp")
                    .join("models")
                    .join(configured_path.file_name().unwrap_or_default()),
                ancestor.join(configured_path.file_name().unwrap_or_default()),
            ]
        };
        if let Some(candidate) = candidates.into_iter().find(|path| path.is_file()) {
            return candidate;
        }
    }

    configured_path
}

fn parse_bool(value: &str, key: &str, line: usize) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("Línea {line}: {key} debe ser true o false"),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::resolve_whisper_path;
    use super::{AgentConfig, config_path_for_executable, default_whisper_server_config_value};
    use std::path::PathBuf;

    #[test]
    fn parses_custom_values_and_resolves_relative_paths() {
        let config = AgentConfig::parse(
            "assistant_name=Sol\nmodel=qwen2.5:3b\navatar_file=img/sol.png\n",
            PathBuf::from("portable"),
            PathBuf::from("portable\\config.txt"),
        )
        .unwrap();

        assert_eq!(config.assistant_name, "Sol");
        assert_eq!(config.model, "qwen2.5:3b");
        assert_eq!(
            config.avatar_file,
            PathBuf::from("portable").join("img").join("sol.png")
        );
    }

    #[test]
    fn parses_face_sequence_paths_relative_to_config() {
        let config = AgentConfig::parse(
            "avatar_sequence=faces/00-normal.png|faces/01-talking.png|faces/02-tongue.png\n",
            PathBuf::from("portable"),
            PathBuf::from("portable\\config.txt"),
        )
        .unwrap();

        assert_eq!(
            config.avatar_sequence,
            vec![
                PathBuf::from("portable")
                    .join("faces")
                    .join("00-normal.png"),
                PathBuf::from("portable")
                    .join("faces")
                    .join("01-talking.png"),
                PathBuf::from("portable")
                    .join("faces")
                    .join("02-tongue.png"),
            ]
        );
    }

    #[test]
    fn saves_face_sequence_to_config() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("config.txt");
        std::fs::write(&config_path, "assistant_name=Mondo\navatar_file=old.png\n").unwrap();
        let mut config = AgentConfig::parse(
            "assistant_name=Mondo\navatar_file=old.png\n",
            directory.path().to_path_buf(),
            config_path.clone(),
        )
        .unwrap();
        let sequence = vec![
            directory.path().join("00-normal.png"),
            directory.path().join("01-talking.png"),
        ];

        config.save_avatar_sequence(sequence.clone()).unwrap();

        let contents = std::fs::read_to_string(config_path).unwrap();
        assert!(contents.contains("avatar_file=00-normal.png"));
        assert!(contents.contains("avatar_sequence=00-normal.png|01-talking.png"));
        assert_eq!(config.avatar_sequence, sequence);
    }

    #[test]
    fn saves_avatar_size_to_config() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("config.txt");
        std::fs::write(&config_path, "assistant_name=Mondo\navatar_size=320\n").unwrap();
        let mut config = AgentConfig::parse(
            "assistant_name=Mondo\navatar_size=320\n",
            directory.path().to_path_buf(),
            config_path.clone(),
        )
        .unwrap();
        config.avatar_size = 180.0;
        config.avatar_height = 260.0;

        config.save_avatar_size().unwrap();

        let contents = std::fs::read_to_string(config_path).unwrap();
        assert!(contents.contains("avatar_size=180"));
        assert!(contents.contains("avatar_height=260"));
        assert_eq!(contents.matches("avatar_size=").count(), 1);
    }

    #[test]
    fn rejects_unknown_options() {
        assert!(
            AgentConfig::parse(
                "not_a_setting=1\n",
                PathBuf::new(),
                PathBuf::from("config.txt"),
            )
            .unwrap_err()
            .to_string()
            .contains("opción desconocida")
        );
    }

    #[test]
    fn parses_custom_global_hotkeys() {
        let config = AgentConfig::parse(
            "push_to_talk_hotkey=CTRL+F9\navatar_toggle_hotkey=ALT+SHIFT+M\n",
            PathBuf::from("portable"),
            PathBuf::from("portable\\config.txt"),
        )
        .unwrap();

        assert_eq!(config.push_to_talk_hotkey, "CTRL+F9");
        assert_eq!(config.avatar_toggle_hotkey, "ALT+SHIFT+M");
    }

    #[test]
    fn parses_whisper_performance_settings() {
        let config = AgentConfig::parse(
            "whisper_language=auto\nwhisper_prompt=Spotify, Discord\nwhisper_threads=8\nwhisper_beam_size=3\n",
            PathBuf::from("portable"),
            PathBuf::from("portable\\config.txt"),
        )
        .unwrap();

        assert_eq!(config.whisper_language, "auto");
        assert_eq!(config.whisper_prompt, "Spotify, Discord");
        assert_eq!(config.whisper_threads, 8);
        assert_eq!(config.whisper_beam_size, 3);
    }

    #[test]
    fn resolves_configured_piper_voice_path() {
        let config = AgentConfig::parse(
            "piper_model=es_MX-ald-medium.onnx\n",
            PathBuf::from("portable"),
            PathBuf::from("portable\\config.txt"),
        )
        .unwrap();

        assert_eq!(
            config.piper_model,
            PathBuf::from("portable").join("es_MX-ald-medium.onnx")
        );
    }

    #[test]
    fn defaults_use_small_whisper_settings_on_arm() {
        let config = AgentConfig::defaults();

        if cfg!(any(target_arch = "arm", target_arch = "aarch64")) {
            assert_eq!(config.whisper_threads, 2);
            assert_eq!(config.whisper_beam_size, 1);
            assert_eq!(
                config.whisper_model.file_name().unwrap(),
                "ggml-tiny-q5_1.bin"
            );
        } else {
            assert_eq!(config.whisper_threads, 4);
            assert_eq!(config.whisper_beam_size, 3);
            assert_eq!(
                config.whisper_model.file_name().unwrap(),
                "ggml-base-q5_1.bin"
            );
        }
    }

    #[test]
    fn rejects_identical_global_hotkeys() {
        let error = AgentConfig::parse(
            "push_to_talk_hotkey=CTRL+F9\navatar_toggle_hotkey=CTRL+F9\n",
            PathBuf::from("portable"),
            PathBuf::from("portable\\config.txt"),
        )
        .unwrap_err();

        assert!(error.to_string().contains("deben ser diferentes"));
    }

    #[test]
    fn finds_built_whisper_assets_for_a_release_executable() {
        let project = tempfile::tempdir().unwrap();
        let executable_dir = project.path().join("target").join("release");
        let whisper_server_bin = project
            .path()
            .join("src")
            .join("whisper.cpp")
            .join("build")
            .join("bin")
            .join(if cfg!(windows) { "Release" } else { "" })
            .join(if cfg!(windows) {
                "whisper-server.exe"
            } else {
                "whisper-server"
            });
        let whisper_model = project.path().join("ggml-base-q5_1.bin");
        std::fs::create_dir_all(whisper_server_bin.parent().unwrap()).unwrap();
        std::fs::write(&whisper_server_bin, b"test executable").unwrap();
        std::fs::write(&whisper_model, b"test model").unwrap();

        let config = AgentConfig::parse(
            &format!(
                "whisper_server_bin={}\nwhisper_model=whisper/ggml-base-q5_1.bin\n",
                default_whisper_server_config_value()
            ),
            executable_dir,
            project
                .path()
                .join("target")
                .join("release")
                .join("config.txt"),
        )
        .unwrap();

        assert_eq!(config.whisper_server_bin, whisper_server_bin);
        assert_eq!(config.whisper_model, whisper_model);
    }

    #[test]
    fn finds_tiny_whisper_model_for_a_release_executable() {
        let project = tempfile::tempdir().unwrap();
        let executable_dir = project.path().join("target").join("release");
        let whisper_model = project.path().join("ggml-tiny-q5_1.bin");
        std::fs::write(&whisper_model, b"test model").unwrap();

        let config = AgentConfig::parse(
            "whisper_model=whisper/ggml-tiny-q5_1.bin\n",
            executable_dir,
            project
                .path()
                .join("target")
                .join("release")
                .join("config.txt"),
        )
        .unwrap();

        assert_eq!(config.whisper_model, whisper_model);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resolves_linux_whisper_binary_from_a_windows_style_config() {
        let project = tempfile::tempdir().unwrap();
        let whisper_server = project
            .path()
            .join("src/whisper.cpp/build/bin/whisper-server");
        std::fs::create_dir_all(whisper_server.parent().unwrap()).unwrap();
        std::fs::write(&whisper_server, b"test executable").unwrap();

        let resolved = resolve_whisper_path(project.path(), r"whisper\whisper-server.exe", true);

        assert_eq!(resolved, whisper_server);
    }

    #[test]
    fn loads_project_config_when_running_from_cargo_release_folder() {
        let project = tempfile::tempdir().unwrap();
        let release_dir = project.path().join("target\\release");
        std::fs::create_dir_all(&release_dir).unwrap();
        let project_config = project.path().join("config.txt");
        std::fs::write(&project_config, "assistant_name=Proyecto\n").unwrap();
        std::fs::write(release_dir.join("config.txt"), "assistant_name=Viejo\n").unwrap();

        let selected = config_path_for_executable(&release_dir);
        let config = AgentConfig::parse(
            &std::fs::read_to_string(&selected).unwrap(),
            selected.parent().unwrap().to_path_buf(),
            selected.clone(),
        )
        .unwrap();

        assert_eq!(selected, project_config);
        assert_eq!(config.assistant_name, "Proyecto");
        assert_eq!(config.avatar_file, project.path().join("avatar.png"));
    }
}
