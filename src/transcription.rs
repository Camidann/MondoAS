//broa
use std::{fs, net::TcpListener, process::Stdio, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::{Client, multipart};
use serde::Deserialize;
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

use crate::config::AgentConfig;

struct WhisperServer {
    child: Child,
    url: String,
}

#[derive(Clone)]
pub struct WhisperTranscriber {
    config: AgentConfig,
    client: Client,
    server: Arc<Mutex<Option<WhisperServer>>>,
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

impl WhisperTranscriber {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            client: Client::new(),
            server: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn transcribe(&self, audio_path: &str) -> Result<String> {
        let url = self.ensure_server().await?;
        let audio = fs::read(audio_path)
            .with_context(|| format!("No se pudo leer el audio {}", audio_path))?;
        let file = multipart::Part::bytes(audio)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .context("No se pudo preparar el WAV para Whisper")?;
        let beam_size = self.config.whisper_beam_size;
        let form = multipart::Form::new()
            .part("file", file)
            .text("language", self.config.whisper_language.clone())
            .text("prompt", self.config.whisper_prompt.clone())
            .text("response_format", "json")
            .text("beam_size", beam_size.to_string())
            .text("best_of", beam_size.to_string());
        let response = self
            .client
            .post(format!("{url}/inference"))
            .multipart(form)
            .send()
            .await
            .context("No se pudo enviar el audio al servidor Whisper")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "No se pudo leer la respuesta".to_string());
            bail!("Error del servidor Whisper ({}): {}", status, body.trim());
        }

        let transcript: TranscriptionResponse = response
            .json()
            .await
            .context("No se pudo leer la transcripción del servidor Whisper")?;
        let text = transcript.text.trim().to_string();
        if text.is_empty() {
            bail!("whisper.cpp no devolvió una transcripción");
        }
        Ok(text)
    }

    async fn ensure_server(&self) -> Result<String> {
        let mut server = self.server.lock().await;
        if let Some(running) = server.as_mut() {
            if running
                .child
                .try_wait()
                .context("No se pudo comprobar el servidor Whisper")?
                .is_none()
            {
                return Ok(running.url.clone());
            }
            *server = None;
        }

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .context("No se pudo reservar un puerto local para Whisper")?;
        let port = listener
            .local_addr()
            .context("No se pudo obtener el puerto local de Whisper")?
            .port();
        drop(listener);

        let beam_size = self.config.whisper_beam_size;
        let mut child = Command::new(&self.config.whisper_server_bin)
            .args(["-m"])
            .arg(&self.config.whisper_model)
            .args(["-t"])
            .arg(self.config.whisper_threads.to_string())
            .args(["-bs"])
            .arg(beam_size.to_string())
            .args(["-bo"])
            .arg(beam_size.to_string())
            .args([
                "-l",
                &self.config.whisper_language,
                "-nt",
                "--host",
                "127.0.0.1",
                "--port",
            ])
            .arg(port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "No se pudo iniciar whisper-server: {}",
                    self.config.whisper_server_bin.display()
                )
            })?;
        let url = format!("http://127.0.0.1:{port}");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

        loop {
            if let Some(status) = child
                .try_wait()
                .context("No se pudo comprobar el inicio del servidor Whisper")?
            {
                bail!("whisper-server terminó durante el inicio con {status}");
            }
            if self.client.get(format!("{url}/")).send().await.is_ok() {
                *server = Some(WhisperServer {
                    child,
                    url: url.clone(),
                });
                return Ok(url);
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("whisper-server no estuvo listo dentro de 60 segundos");
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WhisperTranscriber;
    use crate::config::AgentConfig;

    #[tokio::test]
    async fn persistent_server_transcribes_audio_without_reloading_model() {
        let Some(project_dir) = std::env::current_exe().ok().and_then(|exe| {
            exe.ancestors()
                .find(|dir| {
                    dir.join(r"src\whisper.cpp\build\bin\Release\whisper-server.exe")
                        .is_file()
                        && dir.join(r"src\whisper.cpp\samples\jfk.wav").is_file()
                })
                .map(|dir| dir.to_path_buf())
        }) else {
            return;
        };

        let config = AgentConfig::load().unwrap();
        if !config.whisper_server_bin.is_file() || !config.whisper_model.is_file() {
            return;
        }
        let sample = project_dir.join(r"src\whisper.cpp\samples\jfk.wav");
        let transcriber = WhisperTranscriber::new(config);
        let first = transcriber
            .transcribe(&sample.to_string_lossy())
            .await
            .unwrap();
        let server_url = transcriber
            .server
            .lock()
            .await
            .as_ref()
            .unwrap()
            .url
            .clone();
        assert_eq!(transcriber.ensure_server().await.unwrap(), server_url);

        assert!(!first.is_empty());
        assert_eq!(
            transcriber.server.lock().await.as_ref().unwrap().url,
            server_url
        );
    }
}
