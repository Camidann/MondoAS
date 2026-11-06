use anyhow::{Context, Result, bail};
#[cfg(target_os = "windows")]
use base64::{Engine as _, engine::general_purpose::STANDARD};
#[cfg(target_os = "linux")]
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;

pub async fn speak(text: &str, voice_name: &str, piper_model: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = piper_model;
        let encoded_text = STANDARD.encode(text.as_bytes());
        let encoded_voice = STANDARD.encode(voice_name.as_bytes());
        let script = format!(
            // esto esta hecho por ia / this is made by ai
            "$text = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{}')); \
             $voiceName = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{}')); \
             Add-Type -AssemblyName System.Speech; \
             $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
             $voices = $synth.GetInstalledVoices(); \
             if ($voiceName) {{ \
                 $voice = $voices | Where-Object {{ $_.VoiceInfo.Name -eq $voiceName }} | Select-Object -First 1; \
                 if (-not $voice) {{ throw \"No está instalada la voz configurada: $voiceName\" }}; \
                 $synth.SelectVoice($voice.VoiceInfo.Name); \
             }} else {{ \
                 $voice = $voices | Where-Object {{ $_.VoiceInfo.Culture.Name -like 'es-*' }} | Select-Object -First 1; \
                 if ($voice) {{ $synth.SelectVoice($voice.VoiceInfo.Name) }}; \
             }}; \
             $synth.Speak($text);",
            encoded_text, encoded_voice
        ); // hasta aqui es hecho por ia / until here is made by ai
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .await
            .context("No se pudo iniciar la síntesis de voz de Windows")?;
        if !output.status.success() {
            bail!(
                "La síntesis de voz falló: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
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
