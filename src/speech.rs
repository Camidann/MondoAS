//bro
use anyhow::{Context, Result, bail};
#[cfg(target_os = "windows")]
use base64::{Engine as _, engine::general_purpose::STANDARD};
#[cfg(target_os = "windows")]
use std::sync::OnceLock;
#[cfg(target_os = "linux")]
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;
#[cfg(target_os = "windows")]
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    sync::Mutex,
};

#[cfg(target_os = "windows")]
struct WindowsSpeech {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[cfg(target_os = "windows")]
impl WindowsSpeech {
    async fn start(voice_name: &str) -> Result<Self> {
        let encoded_voice = STANDARD.encode(voice_name.as_bytes());
        let script = format!(
            "$ErrorActionPreference = 'Stop'\n\
             try {{\n\
                 Add-Type -AssemblyName System.Speech\n\
                 $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer\n\
                 $voiceName = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{encoded_voice}'))\n\
                 $voices = $synth.GetInstalledVoices()\n\
                 if ($voiceName) {{\n\
                     $voice = $voices | Where-Object {{ $_.VoiceInfo.Name -eq $voiceName }} | Select-Object -First 1\n\
                     if (-not $voice) {{ throw \"No está instalada la voz configurada: $voiceName\" }}\n\
                     $synth.SelectVoice($voice.VoiceInfo.Name)\n\
                 }} else {{\n\
                     $voice = $voices | Where-Object {{ $_.VoiceInfo.Culture.Name -like 'es-*' }} | Select-Object -First 1\n\
                     if ($voice) {{ $synth.SelectVoice($voice.VoiceInfo.Name) }}\n\
                 }}\n\
                 [Console]::Out.WriteLine('READY')\n\
                 [Console]::Out.Flush()\n\
                 while (($line = [Console]::In.ReadLine()) -ne $null) {{\n\
                     try {{\n\
                         $text = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($line))\n\
                         $synth.Speak($text)\n\
                         [Console]::Out.WriteLine('OK')\n\
                     }} catch {{\n\
                         $errorText = [System.Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($_.Exception.Message))\n\
                         [Console]::Out.WriteLine(\"ERROR:$errorText\")\n\
                     }}\n\
                     [Console]::Out.Flush()\n\
                 }}\n\
             }} catch {{\n\
                 $errorText = [System.Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($_.Exception.Message))\n\
                 [Console]::Out.WriteLine(\"START_ERROR:$errorText\")\n\
                 [Console]::Out.Flush()\n\
             }}",
        );
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("No se pudo iniciar la síntesis de voz de Windows")?;
        let stdin = child
            .stdin
            .take()
            .context("No se pudo enviar texto al sintetizador de Windows")?;
        let stdout = child
            .stdout
            .take()
            .context("No se pudo recibir la respuesta del sintetizador de Windows")?;
        let mut speech = Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let mut response = String::new();
        if speech
            .stdout
            .read_line(&mut response)
            .await
            .context("No se pudo inicializar la voz de Windows")?
            == 0
        {
            bail!("El sintetizador de Windows se cerró durante el inicio");
        }
        if response.trim() != "READY" {
            bail!(
                "No se pudo inicializar la voz de Windows: {}",
                decode_speech_error(response.trim(), "START_ERROR:")
            );
        }
        Ok(speech)
    }

    async fn speak(&mut self, text: &str) -> Result<()> {
        let encoded_text = STANDARD.encode(text.as_bytes());
        self.stdin
            .write_all(encoded_text.as_bytes())
            .await
            .context("No se pudo enviar texto al sintetizador de Windows")?;
        self.stdin
            .write_all(b"\n")
            .await
            .context("No se pudo enviar texto al sintetizador de Windows")?;
        self.stdin
            .flush()
            .await
            .context("No se pudo enviar texto al sintetizador de Windows")?;

        let mut response = String::new();
        if self
            .stdout
            .read_line(&mut response)
            .await
            .context("No se pudo esperar la síntesis de voz de Windows")?
            == 0
        {
            bail!("El sintetizador de Windows se cerró antes de completar la voz");
        }
        match response.trim() {
            "OK" => Ok(()),
            response => bail!(
                "La síntesis de voz falló: {}",
                decode_speech_error(response, "ERROR:")
            ),
        }
    }
}

#[cfg(target_os = "windows")]
fn decode_speech_error(response: &str, prefix: &str) -> String {
    response
        .strip_prefix(prefix)
        .and_then(|encoded| STANDARD.decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| response.to_string())
}

#[cfg(target_os = "windows")]
static WINDOWS_SPEECH: OnceLock<Mutex<Option<WindowsSpeech>>> = OnceLock::new();

#[cfg(target_os = "windows")]
async fn windows_speech(text: &str, voice_name: &str) -> Result<()> {
    let speech = WINDOWS_SPEECH.get_or_init(|| Mutex::new(None));
    let mut speech = speech.lock().await;
    if speech.is_none() {
        *speech = Some(WindowsSpeech::start(voice_name).await?);
    }
    let result = match speech.as_mut() {
        Some(speech) => speech.speak(text).await,
        None => bail!("No se pudo inicializar la voz de Windows"),
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            *speech = None;
            Err(error)
        }
    }
}

#[cfg(target_os = "windows")]
pub async fn warm_up(voice_name: &str) -> Result<()> {
    let speech = WINDOWS_SPEECH.get_or_init(|| Mutex::new(None));
    let mut speech = speech.lock().await;
    if speech.is_none() {
        *speech = Some(WindowsSpeech::start(voice_name).await?);
    }
    Ok(())
}

pub async fn speak(text: &str, voice_name: &str, piper_model: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = piper_model;
        windows_speech(text, voice_name).await
    }

    #[cfg(target_os = "linux")]
    {
        let selected_voice = voice_name.trim();
        if selected_voice.is_empty() {
            return speak_with_piper(text, piper_model).await;
        }
        let mut command = Command::new("bash");
        let mut args = vec!["-lc".to_string()];
        let mut script = format!(
            "if command -v espeak-ng >/dev/null 2>&1; then espeak-ng -s 165 -v {} \"$TEXT\"; elif command -v espeak >/dev/null 2>&1; then espeak -s 165 -v {} \"$TEXT\"; else echo 'No hay un sintetizador de voz instalado. Instala espeak-ng o espeak.' >&2; exit 127; fi",
            escape_for_shell(selected_voice),
            escape_for_shell(selected_voice)
        );
        script = script.replace("$TEXT", &escape_for_shell(text));
        args.push(script);
        let output = command
            .args(args)
            .output()
            .await
            .context("No se pudo iniciar la síntesis de voz en Linux")?;
        if !output.status.success() {
            bail!(
                "La síntesis de voz en Linux falló: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let _ = piper_model;
        let selected_voice = voice_name.trim();
        let mut command = Command::new("say");
        if !selected_voice.is_empty() {
            command.arg("-v").arg(selected_voice);
        }
        command.arg(text);
        let output = command
            .output()
            .await
            .context("No se pudo iniciar la síntesis de voz en macOS")?;
        if !output.status.success() {
            bail!(
                "La síntesis de voz en macOS falló: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        let _ = text;
        let _ = voice_name;
        let _ = piper_model;
        bail!("La respuesta por voz no está implementada para este sistema operativo");
    }
}

#[cfg(target_os = "linux")]
async fn speak_with_piper(text: &str, model: &Path) -> Result<()> {
    let mut model_config = model.as_os_str().to_os_string();
    model_config.push(".json");
    let model_config = PathBuf::from(model_config);
    if !model.is_file() {
        bail!(
            "No se encontró el modelo de voz Piper: {}. `piper_model` se resuelve respecto de la carpeta de config.txt.",
            model.display()
        );
    }
    if !model_config.is_file() {
        bail!(
            "Falta el archivo de configuración de la voz Piper: {}. Copia junto al modelo su archivo `.onnx.json`.",
            model_config.display()
        );
    }

    let output_file = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .context("No se pudo crear el archivo temporal de Piper")?;
    let piper_binary = piper_binary();
    let mut child = Command::new(&piper_binary)
        .args(["--model"])
        .arg(model)
        .args(["--output_file"])
        .arg(output_file.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!(
                "No se pudo iniciar Piper (`{}`). Instálalo y verifica que esté en PATH o en `~/.local/bin/piper`.",
                piper_binary.display()
            )
        })?;
    let mut stdin = child
        .stdin
        .take()
        .context("No se pudo enviar el texto a Piper")?;
    let text_with_newline = format!("{text}\n");
    tokio::io::AsyncWriteExt::write_all(&mut stdin, text_with_newline.as_bytes())
        .await
        .context("No se pudo enviar el texto a Piper")?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .await
        .context("No se pudo esperar la síntesis de Piper")?;
    if !output.status.success() {
        bail!(
            "Piper no pudo sintetizar la respuesta: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let playback = Command::new("aplay")
        .arg("-q")
        .arg(output_file.path())
        .output()
        .await
        .context("No se pudo iniciar aplay. Instala alsa-utils para reproducir la voz de Piper.")?;
    if !playback.status.success() {
        bail!(
            "No se pudo reproducir la voz de Piper: {}",
            String::from_utf8_lossy(&playback.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn piper_binary() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let pipx_binary = PathBuf::from(home).join(".local").join("bin").join("piper");
        if pipx_binary.is_file() {
            return pipx_binary;
        }
    }
    PathBuf::from("piper")
}

#[cfg(target_os = "linux")]
fn escape_for_shell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
}
