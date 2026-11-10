use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
#[cfg(any(windows, target_os = "linux"))]
use std::{env, process::Command};

use crate::config::AgentConfig;

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    keep_alive: &'static str,
    options: GenerateOptions,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Serialize)]
struct GenerateOptions {
    temperature: f32,
    num_ctx: u32,
    num_predict: u32,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    message: ChatResponseMessage,
}

#[derive(Debug, Default, Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: String,
}

#[derive(Clone)]
pub struct Agent {
    client: Client,
    config: AgentConfig,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .build()
            .context("No se pudo configurar la conexión directa con Ollama")?;
        Ok(Self { client, config })
    }

    pub async fn warm_up(&self) -> Result<()> {
        let url = format!("{}/api/chat", self.config.ollama_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .json(&ChatRequest {
                model: self.config.model.clone(),
                messages: build_messages(&self.config.system_prompt, "Respondé solo: listo."),
                stream: false,
                keep_alive: "30m",
                options: GenerateOptions {
                    temperature: self.config.temperature,
                    num_ctx: self.config.context_size,
                    num_predict: 1,
                },
            })
            .send()
            .await
            .with_context(|| format!("No se pudo conectar con Ollama en {url}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "No se pudo leer la respuesta".to_string());
            anyhow::bail!("Error de Ollama ({}): {}", status, body);
        }

        response
            .json::<ChatResponse>()
            .await
            .context("No se pudo leer la respuesta de calentamiento de Ollama")?;
        Ok(())
    }

    pub async fn respond_streaming(
        &self,
        prompt: &str,
        mut on_chunk: impl FnMut(&str) + Send,
    ) -> Result<String> {
        if let Some((action, app_name)) = application_request(prompt) {
            let response = match action {
                ApplicationAction::Open => open_application(&app_name)?,
                ApplicationAction::Close => close_application(&app_name)?,
            };
            on_chunk(&response);
            return Ok(response);
        }

        let url = format!(
            "{}/api/chat",
            self.config.ollama_url.trim_end_matches('/')
        );

        let mut response = self
            .client
            .post(&url)
            .json(&ChatRequest {
                model: self.config.model.clone(),
                messages: build_messages(&self.config.system_prompt, prompt),
                stream: true,
                keep_alive: "30m",
                options: GenerateOptions {
                    temperature: self.config.temperature,
                    num_ctx: self.config.context_size,
                    num_predict: 128,
                },
            })
            .send()
            .await
            .with_context(|| format!("No se pudo conectar con Ollama en {url}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "No se pudo leer la respuesta".to_string());
            anyhow::bail!("Error de Ollama ({}): {}", status, body);
        }

        let mut buffer = Vec::new();
        let mut answer = String::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("No se pudo leer el flujo de respuesta de Ollama")?
        {
            buffer.extend_from_slice(&chunk);
            while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                let line = buffer.drain(..=newline).collect::<Vec<_>>();
                append_stream_response(&line, &mut answer, &mut on_chunk)?;
            }
        }
        if !buffer.is_empty() {
            append_stream_response(&buffer, &mut answer, &mut on_chunk)?;
        }

        Ok(answer.trim().to_string())
    }
}

fn build_messages(system_prompt: &str, user_prompt: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: "system",
            content: format!(
                "{system_prompt}\n\n\
                 Regla prioritaria de idioma: respondé siempre y exclusivamente en español \
                 rioplatense. No uses portugués, aunque el usuario escriba o hable en \
                 portugués. Si no entendés el mensaje, pedí una aclaración en español.\n\n\
                 Priorizá coherencia, exactitud y claridad. Contestá directo y breve, en \
                 una a tres frases salvo que te pidan más detalle. No inventes datos ni \
                 uses humor absurdo si confunde la respuesta."
            ),
        },
        ChatMessage {
            role: "user",
            content: user_prompt.to_string(),
        },
    ]
}

fn append_stream_response(
    line: &[u8],
    answer: &mut String,
    on_chunk: &mut impl FnMut(&str),
) -> Result<()> {
    let line = std::str::from_utf8(line).context("Ollama devolvió una respuesta no válida")?;
    let line = line.trim();
    let data = line.strip_prefix("data:").unwrap_or(line).trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }

    let response: ChatResponse =
        serde_json::from_str(data).context("No se pudo parsear un fragmento de Ollama")?;
    if !response.message.content.is_empty() {
        answer.push_str(&response.message.content);
        on_chunk(&response.message.content);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApplicationAction {
    Open,
    Close,
}

fn application_request(prompt: &str) -> Option<(ApplicationAction, String)> {
    let normalized = normalize_name(prompt);
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    let verbs = [
        (
            ApplicationAction::Open,
            &[
                "abre", "abri", "abrir", "abrime", "abreme", "lanza", "lanzar", "pone", "poneme",
                "ejecuta", "ejecutar", "inicia", "iniciar", "open", "start",
            ][..],
        ),
        (
            ApplicationAction::Close,
            &[
                "cierra",
                "cerra",
                "cerrar",
                "cerrame",
                "cierre",
                "cierres",
                "termina",
                "terminar",
                "finaliza",
                "finalizar",
                "sali",
                "salir",
                "close",
                "quit",
                "stop",
            ][..],
        ),
    ];

    let mut found_verb = None;
    for (index, word) in words.iter().take(5).enumerate() {
        for (action, forms) in verbs {
            if forms.contains(word) || is_misspelled_verb(word, forms) {
                found_verb = Some((action, index, 1));
                break;
            }
        }
        if found_verb.is_some() {
            break;
        }
        if index + 1 < words.len()
            && matches!(
                *word,
                "abre" | "abri" | "abrir" | "cierra" | "cerra" | "cerrar"
            )
            && words[index + 1] == "me"
        {
            let action = if matches!(*word, "cierra" | "cerra" | "cerrar") {
                ApplicationAction::Close
            } else {
                ApplicationAction::Open
            };
            found_verb = Some((action, index, 2));
            break;
        }
    }
    let (action, verb_index, verb_len) = found_verb?;
    let mut app_words = words[verb_index + verb_len..].to_vec();
    if app_words
        .first()
        .is_some_and(|word| matches!(*word, "la" | "el"))
        && app_words.len() > 1
        && matches!(app_words[1], "app" | "aplicacion" | "programa")
    {
        app_words.drain(..2);
    } else if app_words
        .first()
        .is_some_and(|word| matches!(*word, "app" | "aplicacion" | "programa"))
    {
        app_words.remove(0);
    }
    if app_words.first() == Some(&"me") {
        app_words.remove(0);
    }
    if app_words.ends_with(&["por", "favor"]) {
        app_words.truncate(app_words.len() - 2);
    } else if app_words
        .last()
        .is_some_and(|word| matches!(*word, "porfa" | "please"))
    {
        app_words.pop();
    }
    if app_words.starts_with(&["la", "aplicacion"]) || app_words.starts_with(&["el", "programa"]) {
        app_words.drain(..2);
    }
    let app_name = app_words.join(" ");
    (!app_name.is_empty()).then_some((action, app_name))
}

fn is_misspelled_verb(word: &str, forms: &[&str]) -> bool {
    forms.iter().any(|form| {
        let distance = edit_distance(word, form);
        distance == 1 || (form.len() >= 6 && distance == 2 && word.len() >= 6)
    })
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            character if character.is_alphanumeric() => character,
            _ => ' ',
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

enum ApplicationMatch {
    Found { name: String, path: PathBuf },
    FoundStartMenu { name: String, app_id: String },
    Ambiguous(Vec<String>),
}

fn application_match_score(requested: &str, candidate: &str) -> u8 {
    let requested = normalize_name(requested);
    let candidate = normalize_name(candidate);
    if requested == candidate {
        return 100;
    }

    let requested_words = requested.split_whitespace().collect::<Vec<_>>();
    let candidate_words = candidate.split_whitespace().collect::<Vec<_>>();
    if requested_words
        .iter()
        .all(|word| candidate_words.contains(word))
    {
        return 80;
    }

    if requested_words.len() > 2 {
        return 0;
    }
    let requested_term = requested_words.concat();
    if requested_term.len() < 4 {
        return 0;
    }

    candidate_words
        .iter()
        .filter_map(|word| {
            let distance = edit_distance(&requested_term, word);
            let max_distance = word.len().max(requested_term.len());
            let allowed_distance = if max_distance >= 7 { 2 } else { 1 };
            (distance > 0 && distance <= allowed_distance)
                .then_some(60u8.saturating_sub(distance as u8 * 10))
        })
        .max()
        .unwrap_or_default()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.chars().count()).collect::<Vec<_>>();
    let mut current = vec![0; previous.len()];
    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right.chars().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + usize::from(left_char != right_char));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.chars().count()]
}

#[cfg(test)]
fn find_application(requested: &str, roots: &[PathBuf]) -> Result<Option<ApplicationMatch>> {
    let mut matches = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_applications(root, &mut files)?;
        matches.extend(
            files
                .into_iter()
                .map(|(name, path)| (name, Some(path), None)),
        );
    }

    Ok(resolve_application(requested, matches))
}

fn resolve_application(
    requested: &str,
    matches: Vec<(String, Option<PathBuf>, Option<String>)>,
) -> Option<ApplicationMatch> {
    let requested = normalize_name(requested);
    let mut visited_names = HashSet::new();
    let best_score = matches
        .iter()
        .map(|(name, _, _)| application_match_score(&requested, &normalize_name(name)))
        .max()
        .unwrap_or_default();
    if best_score == 0 {
        return None;
    }

    let mut best_matches = matches
        .into_iter()
        .filter(|(name, _, _)| {
            application_match_score(&requested, &normalize_name(name)) == best_score
        })
        .filter(|(name, _, _)| visited_names.insert(normalize_name(name)))
        .collect::<Vec<_>>();
    if best_matches.len() > 1 {
        return Some(ApplicationMatch::Ambiguous(
            best_matches.into_iter().map(|(name, _, _)| name).collect(),
        ));
    }

    best_matches
        .pop()
        .map(|(name, path, app_id)| match (path, app_id) {
            (Some(path), _) => ApplicationMatch::Found { name, path },
            (_, Some(app_id)) => ApplicationMatch::FoundStartMenu { name, app_id },
            (None, None) => unreachable!("application candidate must have a launch target"),
        })
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct StartMenuApp {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "AppID")]
    app_id: String,
}

#[cfg(windows)]
fn installed_start_menu_apps() -> Result<Vec<(String, String)>> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-StartApps | Select-Object Name,AppID | ConvertTo-Json -Compress",
        ])
        .output()
        .context("No se pudieron enumerar las aplicaciones registradas en Windows")?;
    if !output.status.success() {
        bail!(
            "Windows no pudo enumerar las aplicaciones instaladas: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let json = String::from_utf8_lossy(&output.stdout);
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(&json)
        .context("Windows devolvió una lista de aplicaciones inválida")?;
    let apps = match value {
        serde_json::Value::Array(apps) => apps,
        app => vec![app],
    };
    apps.into_iter()
        .map(|app| {
            let app: StartMenuApp = serde_json::from_value(app)
                .context("Windows devolvió una aplicación sin nombre o identificador")?;
            Ok((app.name, app.app_id))
        })
        .collect()
}

fn collect_applications(root: &Path, matches: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("No se pudo buscar aplicaciones en {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("No se pudo leer {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("No se pudo comprobar {}", path.display()))?;

        if file_type.is_dir() {
            collect_applications(&path, matches)?;
        } else if file_type.is_file()
            && matches!(
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("lnk" | "exe")
            )
            && let Some(name) = path.file_stem().and_then(|stem| stem.to_str())
        {
            matches.push((name.to_string(), path));
        }
    }
    Ok(())
}

// abir aplicaciones / open apps

#[cfg(windows)]
fn open_application(requested: &str) -> Result<String> {
    let roots = [
        env::var_os("APPDATA")
            .map(|path| PathBuf::from(path).join(r"Microsoft\Windows\Start Menu\Programs")),
        env::var_os("PROGRAMDATA")
            .map(|path| PathBuf::from(path).join(r"Microsoft\Windows\Start Menu\Programs")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    let mut candidates = Vec::new();
    for (name, app_id) in installed_start_menu_apps()? {
        candidates.push((name, None, Some(app_id)));
    }
    for root in &roots {
        if !root.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_applications(root, &mut files)?;
        candidates.extend(
            files
                .into_iter()
                .map(|(name, path)| (name, Some(path), None)),
        );
    }
    let Some(application) = resolve_application(requested, candidates) else {
        return Ok(format!(
            "No encontré «{requested}» en las aplicaciones instaladas ni en los accesos del menú Inicio."
        ));
    };

    match application {
        ApplicationMatch::Found { name, path } => {
            Command::new("explorer.exe")
                .arg(&path)
                .spawn()
                .with_context(|| format!("No se pudo abrir {} ({})", name, path.display()))?;
            Ok(format!("Abriendo {name}."))
        }
        ApplicationMatch::FoundStartMenu { name, app_id } => {
            let target = format!("shell:AppsFolder\\{app_id}");
            Command::new("explorer.exe")
                .arg(&target)
                .spawn()
                .with_context(|| format!("No se pudo iniciar {name}"))?;
            Ok(format!("Abriendo {name}."))
        }
        ApplicationMatch::Ambiguous(names) => Ok(format!(
            "Encontré varias aplicaciones que coinciden: {}. Decime cuál querés abrir.",
            names.join(", ")
        )),
    }
}

#[cfg(target_os = "linux")]
fn open_application(requested: &str) -> Result<String> {
    let mut roots = Vec::new();
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        roots.push(PathBuf::from(data_home).join("applications"));
    } else if let Some(home) = env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".local/share/applications"));
    }
    roots.extend([
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/usr/share/applications"),
    ]);

    let mut candidates = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        collect_desktop_applications(&root, &mut candidates)?;
    }
    let Some(application) = resolve_application(requested, candidates) else {
        return Ok(format!(
            "No encontré «{requested}» en las aplicaciones instaladas del entorno gráfico."
        ));
    };

    match application {
        ApplicationMatch::Found { name, path } => {
            let app_id = path
                .file_stem()
                .and_then(|name| name.to_str())
                .with_context(|| format!("Nombre de aplicación inválido: {}", path.display()))?;
            match Command::new("gtk-launch").arg(app_id).spawn() {
                Ok(_) => Ok(format!("Abriendo {name}.")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Command::new("gio")
                        .arg("launch")
                        .arg(&path)
                        .spawn()
                        .with_context(|| {
                            format!(
                                "No se pudo iniciar {name}. Instala gtk-launch o GIO para abrir aplicaciones."
                            )
                        })?;
                    Ok(format!("Abriendo {name}."))
                }
                Err(error) => Err(error).with_context(|| format!("No se pudo iniciar {name}")),
            }
        }
        ApplicationMatch::Ambiguous(names) => Ok(format!(
            "Encontré varias aplicaciones que coinciden: {}. Decime cuál querés abrir.",
            names.join(", ")
        )),
        ApplicationMatch::FoundStartMenu { .. } => {
            unreachable!("Linux apps use desktop entry paths")
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_application(_requested: &str) -> Result<String> {
    bail!("Abrir aplicaciones desde Mondo no está disponible en este sistema")
}

#[cfg(target_os = "linux")]
fn collect_desktop_applications(
    root: &Path,
    matches: &mut Vec<(String, Option<PathBuf>, Option<String>)>,
) -> Result<()> {
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("No se pudieron buscar aplicaciones en {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("No se pudo leer {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("No se pudo comprobar {}", path.display()))?;
        if file_type.is_dir() {
            collect_desktop_applications(&path, matches)?;
            continue;
        }
        if !file_type.is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("desktop")
        {
            continue;
        }

        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("No se pudo leer {}", path.display()))?;
        let mut in_desktop_entry = false;
        let mut name = None;
        let mut hidden = false;
        let mut is_application = false;
        for line in contents.lines().map(str::trim) {
            if line.starts_with('[') && line.ends_with(']') {
                in_desktop_entry = line == "[Desktop Entry]";
            } else if in_desktop_entry {
                if let Some(value) = line.strip_prefix("Name=") {
                    name = Some(value.to_string());
                } else if let Some(value) = line.strip_prefix("Hidden=") {
                    hidden = value.eq_ignore_ascii_case("true");
                } else if let Some(value) = line.strip_prefix("NoDisplay=") {
                    hidden |= value.eq_ignore_ascii_case("true");
                } else if let Some(value) = line.strip_prefix("Type=") {
                    is_application = value == "Application";
                }
            }
        }
        if !hidden
            && is_application
            && let Some(name) = name.filter(|name| !name.is_empty())
        {
            matches.push((name, Some(path), None));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn close_application(requested: &str) -> Result<String> {
    let output = Command::new("wmctrl")
        .arg("-l")
        .output()
        .context("No se pudo consultar las ventanas. Instala wmctrl en un escritorio X11.")?;
    if !output.status.success() {
        bail!(
            "wmctrl no pudo consultar las ventanas. Esta función requiere una sesión X11; en Wayland, cierra la ventana desde el escritorio."
        );
    }

    let windows = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            columns.next()?;
            columns.next()?;
            columns.next()?;
            let title = columns.collect::<Vec<_>>().join(" ");
            (!title.is_empty()).then_some(title)
        })
        .collect::<Vec<_>>();
    let best_score = windows
        .iter()
        .map(|title| application_match_score(requested, title))
        .max()
        .unwrap_or_default();
    if best_score < 40 {
        return Ok(format!("No encontré una ventana abierta de «{requested}»."));
    }

    let mut matched_titles = windows
        .iter()
        .filter(|title| application_match_score(requested, title) == best_score)
        .cloned()
        .collect::<Vec<_>>();
    matched_titles.sort_unstable_by_key(|title| normalize_name(title));
    matched_titles.dedup_by(|left, right| normalize_name(left) == normalize_name(right));
    if matched_titles.len() > 1 {
        return Ok(format!(
            "Encontré varias ventanas que coinciden: {}. Decime cuál querés cerrar.",
            matched_titles.join(", ")
        ));
    }

    let title = &matched_titles[0];
    let output = Command::new("wmctrl")
        .args(["-c", title])
        .output()
        .context("No se pudo solicitar el cierre normal de la ventana")?;
    if !output.status.success() {
        bail!(
            "No se pudo cerrar normalmente «{title}»: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(format!(
        "Solicité el cierre normal de «{title}». La aplicación puede pedirte confirmación para guardar cambios."
    ))
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct VisibleProcess {
    #[serde(rename = "Id")]
    id: u32,
    #[serde(rename = "ProcessName")]
    process_name: String,
}

#[cfg(windows)]
fn close_application(requested: &str) -> Result<String> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object Id,ProcessName | ConvertTo-Json -Compress",
        ])
        .output()
        .context("No se pudieron consultar las aplicaciones abiertas")?;
    if !output.status.success() {
        bail!(
            "Windows no pudo consultar las aplicaciones abiertas: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let json = String::from_utf8_lossy(&output.stdout);
    if json.trim().is_empty() {
        return Ok(format!("No veo ninguna ventana abierta de «{requested}»."));
    }
    let value: serde_json::Value =
        serde_json::from_str(&json).context("Windows devolvió una lista de procesos inválida")?;
    let processes = match value {
        serde_json::Value::Array(processes) => processes,
        process => vec![process],
    }
    .into_iter()
    .map(serde_json::from_value::<VisibleProcess>)
    .collect::<std::result::Result<Vec<_>, _>>()
    .context("Windows devolvió un proceso sin los datos necesarios")?;

    let best_score = processes
        .iter()
        .map(|process| application_match_score(requested, &process.process_name))
        .max()
        .unwrap_or_default();
    if best_score < 40 {
        return Ok(format!("No encontré una ventana abierta de «{requested}»."));
    }

    let mut matched_names = processes
        .iter()
        .filter(|process| application_match_score(requested, &process.process_name) == best_score)
        .map(|process| process.process_name.clone())
        .collect::<Vec<_>>();
    matched_names.sort_unstable_by_key(|name| normalize_name(name));
    matched_names.dedup_by(|left, right| normalize_name(left) == normalize_name(right));
    if matched_names.len() > 1 {
        return Ok(format!(
            "Encontré varias aplicaciones abiertas que coinciden: {}. Decime cuál querés cerrar.",
            matched_names.join(", ")
        ));
    }

    let process_name = &matched_names[0];
    let process_ids = processes
        .iter()
        .filter(|process| normalize_name(&process.process_name) == normalize_name(process_name))
        .map(|process| process.id.to_string())
        .collect::<Vec<_>>();
    let script = format!(
        "$ids = @({}); foreach ($id in $ids) {{ $p = Get-Process -Id $id -ErrorAction Stop; Write-Output ($p.CloseMainWindow().ToString()) }}",
        process_ids.join(",")
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("No se pudo solicitar el cierre de la aplicación")?;
    if !output.status.success() {
        bail!(
            "No se pudo cerrar {}: {}",
            process_name,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let close_requested = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.trim().eq_ignore_ascii_case("true"))
        .count();
    if close_requested == 0 {
        return Ok(format!(
            "{process_name} no aceptó cerrar su ventana. No forcé el cierre para proteger cualquier trabajo sin guardar."
        ));
    }
    Ok(format!(
        "Solicité el cierre normal de {process_name}. Si tiene cambios sin guardar, puede pedirte confirmación."
    ))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn close_application(_requested: &str) -> Result<String> {
    bail!("Cerrar aplicaciones desde Mondo no está disponible en este sistema")
}

// tests
#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::collect_desktop_applications;
    use super::{
        ApplicationAction, ApplicationMatch, append_stream_response, application_match_score,
        application_request, build_messages, find_application,
    };
    use std::path::PathBuf;

    #[test]
    fn prompt_requires_rioplatense_spanish_even_if_input_looks_portuguese() {
        let messages = build_messages("Personalidad personalizada.", "Olá, tudo bem?");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.starts_with("Personalidad personalizada."));
        assert!(messages[0].content.contains("siempre y exclusivamente en español rioplatense"));
        assert!(messages[0].content.contains("No uses portugués"));
        assert!(
            messages[0]
                .content
                .contains("Priorizá coherencia, exactitud y claridad.")
        );
        assert!(messages[0].content.contains("No inventes datos"));
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].content, "Olá, tudo bem?");
    }

    #[test]
    fn detects_spanish_open_application_commands() {
        assert_eq!(
            application_request("Abrí Google Chrome"),
            Some((ApplicationAction::Open, "google chrome".to_string()))
        );
        assert_eq!(
            application_request("abre calculadora"),
            Some((ApplicationAction::Open, "calculadora".to_string()))
        );
        assert_eq!(
            application_request("abrir Paint!"),
            Some((ApplicationAction::Open, "paint".to_string()))
        );
        assert_eq!(
            application_request("abrí la puerta"),
            Some((ApplicationAction::Open, "la puerta".to_string()))
        );
        assert_eq!(
            application_request("podés abrir Chrome"),
            Some((ApplicationAction::Open, "chrome".to_string()))
        );
        assert_eq!(
            application_request("por favor abrime Spotify"),
            Some((ApplicationAction::Open, "spotify".to_string()))
        );
        assert_eq!(
            application_request("pone Spotify por favor"),
            Some((ApplicationAction::Open, "spotify".to_string()))
        );
        assert_eq!(
            application_request("abrí la aplicación Spotify"),
            Some((ApplicationAction::Open, "spotify".to_string()))
        );
        assert_eq!(
            application_request("abrir me Spotify"),
            Some((ApplicationAction::Open, "spotify".to_string()))
        );
        assert_eq!(
            application_request("abrime el programa Spotify"),
            Some((ApplicationAction::Open, "spotify".to_string()))
        );
        assert_eq!(
            application_request("habrime Spotify"),
            Some((ApplicationAction::Open, "spotify".to_string()))
        );
        assert_eq!(
            application_request("por favor cerrame Spotify"),
            Some((ApplicationAction::Close, "spotify".to_string()))
        );
        assert_eq!(
            application_request("serrame Spotify"),
            Some((ApplicationAction::Close, "spotify".to_string()))
        );
        assert_eq!(
            application_request("cerrá la aplicación Spotify"),
            Some((ApplicationAction::Close, "spotify".to_string()))
        );
        assert_eq!(
            application_request("cierra Spotify por favor"),
            Some((ApplicationAction::Close, "spotify".to_string()))
        );
        assert_eq!(
            application_request("cerrar me Spotify"),
            Some((ApplicationAction::Close, "spotify".to_string()))
        );
        assert_eq!(application_request("abrí"), None);
    }

    #[test]
    fn matches_accented_application_names_and_words() {
        assert_eq!(
            application_match_score("configuracion", "Configuración"),
            100
        );
        assert_eq!(application_match_score("chrome", "Google Chrome"), 80);
        assert!(application_match_score("spoty", "Spotify") > 0);
        assert!(application_match_score("spoty fi", "Spotify") > 0);
        assert!(application_match_score("spotifai", "Spotify") > 0);
        assert_eq!(application_match_score("spoon", "Spotify"), 0);
        assert_eq!(
            application_match_score("microsoft code", "Visual Studio Code"),
            0
        );
    }

    #[test]
    fn finds_application_shortcuts_case_and_accent_insensitively() {
        let directory = tempfile::tempdir().unwrap();
        let shortcut = directory.path().join("Configuración.lnk");
        std::fs::write(&shortcut, b"shortcut").unwrap();

        let found = find_application("configuracion", &[PathBuf::from(directory.path())])
            .unwrap()
            .unwrap();
        let ApplicationMatch::Found { name, path } = found else {
            panic!("Expected to find one application");
        };

        assert_eq!(name, "Configuración");
        assert_eq!(path, shortcut);
    }

    #[test]
    fn reports_ambiguous_application_matches_and_ignores_other_files() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Microsoft Paint.lnk"), b"shortcut").unwrap();
        std::fs::write(directory.path().join("Paint 3D.exe"), b"application").unwrap();
        std::fs::write(directory.path().join("paint-notes.txt"), b"not an app").unwrap();

        let found = find_application("paint", &[PathBuf::from(directory.path())]).unwrap();
        let Some(ApplicationMatch::Ambiguous(names)) = found else {
            panic!("Expected ambiguous application matches");
        };

        assert_eq!(names.len(), 2);
        assert!(names.contains(&"Microsoft Paint".to_string()));
        assert!(names.contains(&"Paint 3D".to_string()));
    }

    #[test]
    fn finds_application_with_a_small_speech_recognition_error() {
        let directory = tempfile::tempdir().unwrap();
        let shortcut = directory.path().join("Spotify.lnk");
        std::fs::write(&shortcut, b"shortcut").unwrap();

        let found = find_application("spoty", &[PathBuf::from(directory.path())])
            .unwrap()
            .unwrap();
        let ApplicationMatch::Found { name, path } = found else {
            panic!("Expected fuzzy matching to find Spotify");
        };

        assert_eq!(name, "Spotify");
        assert_eq!(path, shortcut);
    }

    #[test]
    fn parses_ollama_streamed_json_lines_and_emits_each_chunk() {
        let mut answer = String::new();
        let mut chunks = Vec::new();
        let mut on_chunk = |chunk: &str| chunks.push(chunk.to_string());

        append_stream_response(
            br#"{"message":{"role":"assistant","content":"Hola"},"done":false}"#,
            &mut answer,
            &mut on_chunk,
        )
        .unwrap();
        append_stream_response(
            br#"{"message":{"role":"assistant","content":" mundo"},"done":false}"#,
            &mut answer,
            &mut on_chunk,
        )
        .unwrap();
        append_stream_response(
            br#"{"message":{"role":"assistant","content":""},"done":true}"#,
            &mut answer,
            &mut on_chunk,
        )
        .unwrap();

        assert_eq!(answer, "Hola mundo");
        assert_eq!(chunks, ["Hola", " mundo"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_linux_desktop_apps_and_ignores_hidden_entries() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("mondo-test.desktop"),
            "[Desktop Entry]\nType=Application\nName=Mondo Test\nExec=mondo-test\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("hidden.desktop"),
            "[Desktop Entry]\nType=Application\nName=Hidden Mondo Test\nHidden=true\n",
        )
        .unwrap();
        let mut apps = Vec::new();

        collect_desktop_applications(directory.path(), &mut apps).unwrap();

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].0, "Mondo Test");
        assert_eq!(
            apps[0].1.as_deref(),
            Some(directory.path().join("mondo-test.desktop").as_path())
        );
    }
}
