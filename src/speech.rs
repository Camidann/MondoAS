use anyhow::{Context, Result, bail};
#[cfg(target_os = "windows")]
use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::process::Command;

pub async fn speak(text: &str, voice_name: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
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
        let mut command = Command::new("bash");
        let mut args = vec!["-lc".to_string()];
        let mut script = if selected_voice.is_empty() {
            String::from(
                "if command -v espeak-ng >/dev/null 2>&1; then espeak-ng -s 165 \"$TEXT\"; elif command -v espeak >/dev/null 2>&1; then espeak -s 165 \"$TEXT\"; else echo 'No hay un sintetizador de voz instalado. Instala espeak-ng o espeak.' >&2; exit 127; fi",
            )
        } else {
            format!(
                "if command -v espeak-ng >/dev/null 2>&1; then espeak-ng -s 165 -v {} \"$TEXT\"; elif command -v espeak >/dev/null 2>&1; then espeak -s 165 -v {} \"$TEXT\"; else echo 'No hay un sintetizador de voz instalado. Instala espeak-ng o espeak.' >&2; exit 127; fi",
                escape_for_shell(selected_voice),
                escape_for_shell(selected_voice)
            )
        };
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
        bail!("La respuesta por voz no está implementada para este sistema operativo");
    }
}

#[cfg(target_os = "linux")]
fn escape_for_shell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
}
