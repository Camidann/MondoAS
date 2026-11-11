//bro
mod agent;
mod audio_input;
mod config;
mod desktop;
mod speech;
mod transcription;

use anyhow::{Context, Result};
use config::AgentConfig;
use desktop::DesktopApp;

fn main() {
    if let Err(err) = run() {
        eprintln!("No se pudo iniciar Mondo: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let config = AgentConfig::load()?;
    let mut runtime_builder = tokio::runtime::Builder::new_multi_thread();
    #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
    runtime_builder.worker_threads(2);
    let runtime = runtime_builder
        .enable_all()
        .build()
        .context("No se pudo iniciar el runtime de red y audio")?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([640.0, 480.0])
            .with_min_inner_size([400.0, 320.0]),
        ..Default::default()
    };
    let title = config.assistant_name.clone();

    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            DesktopApp::new(cc, config, runtime)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(|err| {
                    Box::new(std::io::Error::other(format!("{err:#}")))
                        as Box<dyn std::error::Error + Send + Sync>
                })
        }),
    )
    .map_err(|err| anyhow::anyhow!("Falló la ventana de Mondo: {err}"))?;
    Ok(())
}
