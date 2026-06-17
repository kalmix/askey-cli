mod converter;
mod models;
mod parser;

use ansi_to_tui::IntoText;
use clap::{Parser, Subcommand};
use crossterm::{
    cursor,
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph},
    Terminal,
};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    author,
    version,
    about = "A terminal ASCII animation player and converter.",
    long_about = "A terminal ASCII animation player and converter. Creates a custom file called \
                  \".askey\", which can be shared and played on any machine with this app or \
                  in the browser-based player at https://askey.vercel.app"
)]
struct Cli {
    /// Action to perform: play (default) or fetch
    #[command(subcommand)]
    command: Option<Commands>,

    /// input .askey file
    #[arg(global = true)]
    file: Option<PathBuf>,

    /// Show the full control panel dashboard layout
    #[arg(short, long, global = true)]
    dashboard: bool,

    /// Force play animations with no-clip enabled by default
    #[arg(short, long, global = true)]
    noclip: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Play the animation
    Play { file: PathBuf },
    /// Show system info alongside animation
    Fetch { file: PathBuf },
    /// Convert an image (PNG, JPEG) or GIF to a .askey file
    Convert {
        /// Input image or GIF file path
        input: PathBuf,
        /// Output .askey file path (defaults to input file name with .askey extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Target width of the ASCII output in characters
        #[arg(short, long, default_value_t = 80)]
        width: u32,
        /// Character preset palette ("blocks", "standard", or custom characters)
        #[arg(short, long, default_value = "blocks")]
        preset: String,
        /// Height scale correction factor (terminal characters are taller than wide)
        #[arg(short, long, default_value_t = 0.5)]
        scale: f32,
        /// Color quantization step (16 = standard, 32 = toon, 48 = posterized, 64 = retro)
        #[arg(short, long, default_value_t = 16)]
        quantize: u8,
    },
}

fn get_app_dir() -> anyhow::Result<PathBuf> {
    let mut path = if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata)
    } else if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        let mut p = PathBuf::from(home);
        if cfg!(windows) {
            p.push("AppData");
            p.push("Roaming");
        } else {
            p.push(".config");
        }
        p
    } else {
        PathBuf::from(".")
    };
    path.push("askey");
    if !path.exists() {
        std::fs::create_dir_all(&path)?;
    }
    Ok(path)
}

fn get_animations_dir() -> anyhow::Result<PathBuf> {
    let app_dir = get_app_dir()?;
    let anim_dir = app_dir.join("animations");
    if !anim_dir.exists() {
        std::fs::create_dir_all(&anim_dir)?;
    }

    Ok(anim_dir)
}

fn get_animations() -> anyhow::Result<Vec<(PathBuf, String)>> {
    let dir = get_animations_dir()?;
    let mut anims = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().map(|s| s == "askey").unwrap_or(false) {
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            anims.push((path, filename));
        }
    }
    anims.sort_by_key(|a| a.1.to_ascii_lowercase());
    Ok(anims)
}

fn load_or_create_config() -> anyhow::Result<models::AppConfig> {
    let dir = get_app_dir()?;
    let path = dir.join("config.json");
    if path.exists() {
        let file = std::fs::File::open(&path)?;
        let config: models::AppConfig = serde_json::from_reader(file).unwrap_or_default();
        Ok(config)
    } else {
        let config = models::AppConfig::default();
        let file = std::fs::File::create(&path)?;
        serde_json::to_writer_pretty(file, &config)?;
        Ok(config)
    }
}

fn import_animation(pasted_text: &str) -> anyhow::Result<PathBuf> {
    let cleaned_path = pasted_text
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim();

    let source_path = PathBuf::from(cleaned_path);
    if !source_path.exists() || !source_path.is_file() {
        return Err(anyhow::anyhow!("File does not exist or is not a file"));
    }

    let ext = source_path
        .extension()
        .map(|s| s.to_ascii_lowercase().to_string_lossy().into_owned())
        .unwrap_or_default();

    let dest_dir = get_animations_dir()?;
    let filename = source_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?;

    if ext == "askey" {
        let _valid_file = parser::load_askey_file(&source_path)?;
        let dest_path = dest_dir.join(filename);
        std::fs::copy(&source_path, &dest_path)?;
        Ok(dest_path)
    } else {
        Err(anyhow::anyhow!(
            "Unsupported file format. Please drop .askey files."
        ))
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut config = load_or_create_config().unwrap_or_default();

    let (file_path, is_fetch) = match cli.command {
        Some(Commands::Play { file }) => (Some(file), false),
        Some(Commands::Fetch { file }) => (Some(file), true),
        Some(Commands::Convert {
            input,
            output,
            width,
            preset,
            scale,
            quantize,
        }) => {
            let preset_enum = models::CharsetPreset::from_str(&preset);
            let quantize_enum = models::QuantizationLevel::from_u8(quantize);
            let askey_file = converter::convert_image_to_askey(
                &input,
                width,
                preset_enum,
                scale,
                quantize_enum,
            )?;
            let (output_path, saved_in_appdata) = match output {
                Some(out_path) => (out_path, false),
                None => {
                    let anims_dir = get_animations_dir()?;
                    let mut filename = PathBuf::from(input.file_name().unwrap_or_default());
                    filename.set_extension("askey");
                    (anims_dir.join(filename), true)
                }
            };

            use flate2::write::GzEncoder;
            use flate2::Compression;
            use std::io::Write;

            let json_str = serde_json::to_string(&askey_file)?;
            let file = std::fs::File::create(&output_path)?;
            let mut encoder = GzEncoder::new(file, Compression::best());
            encoder.write_all(json_str.as_bytes())?;
            encoder.finish()?;

            if saved_in_appdata {
                println!(
                    "Successfully converted {} and saved to the AppData animations library:",
                    input.display()
                );
                println!("  {}", output_path.display());
            } else {
                println!(
                    "Successfully converted {} to {}",
                    input.display(),
                    output_path.display()
                );
            }
            println!(
                "Frames: {}, Target Size: {}x{}",
                askey_file.m.f,
                width,
                if askey_file.m.f > 0 {
                    match &askey_file.fr {
                        crate::models::Frames::Simple(v) => v[0].split('\n').count(),
                        crate::models::Frames::Detailed(v) => v[0].c.split('\n').count(),
                    }
                } else {
                    0
                }
            );
            return Ok(());
        }
        None => {
            if let Some(f) = cli.file {
                (Some(f), false)
            } else {
                (None, false)
            }
        }
    };

    // Resolve file_path against the current working directory, 
    // falling back to the AppData library directory
    let file_path = if let Some(ref path) = file_path {
        if path.exists() {
            Some(path.clone())
        } else {
            let appdata_dir = get_animations_dir()?;
            let candidate1 = appdata_dir.join(path);
            if candidate1.exists() {
                Some(candidate1)
            } else {
                let mut path_with_ext = path.clone();
                if path_with_ext.extension().is_none() {
                    path_with_ext.set_extension("askey");
                }
                let candidate2 = appdata_dir.join(&path_with_ext);
                if candidate2.exists() {
                    Some(candidate2)
                } else {
                    Some(path.clone())
                }
            }
        }
    } else {
        None
    };

    let mut initial_askey = None;
    if let Some(ref path) = file_path {
        let is_askey_ext = path
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("askey"))
            .unwrap_or(false);

        if is_askey_ext {
            initial_askey = Some(parser::load_askey_file(path)?);
        } else {
            let askey = converter::convert_image_to_askey(
                path,
                80,
                models::CharsetPreset::Blocks,
                0.5,
                models::QuantizationLevel::Standard,
            )?;
            initial_askey = Some(askey);
        }
    }

    let is_minimal_mode = !(cli.dashboard || is_fetch || config.default_dashboard);
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        cursor::Hide,
        EnableBracketedPaste
    )?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app_dir = get_app_dir()?;
    let flag_file = app_dir.join("font_check_passed.flag");
    if !flag_file.exists() {
        match run_font_check(&mut terminal) {
            Ok(true) => {}
            Ok(false) => {
                terminal::disable_raw_mode()?;
                execute!(
                    io::stdout(),
                    LeaveAlternateScreen,
                    cursor::Show,
                    DisableBracketedPaste
                )?;
                return Ok(());
            }
            Err(e) => {
                terminal::disable_raw_mode()?;
                execute!(
                    io::stdout(),
                    LeaveAlternateScreen,
                    cursor::Show,
                    DisableBracketedPaste
                )?;
                return Err(e);
            }
        }
    }

    let mut play_file = file_path.clone();
    let mut preloaded_askey = initial_askey;

    loop {
        let selected_file = if let Some(ref path) = play_file {
            Some(path.clone())
        } else {
            match run_selector(&mut terminal)? {
                Some((path, askey)) => {
                    preloaded_askey = Some(askey);
                    config = load_or_create_config().unwrap_or_default();
                    Some(path)
                }
                None => break,
            }
        };

        if let Some(path) = selected_file {
            let res = play_animation(
                &mut terminal,
                &path,
                is_fetch,
                is_minimal_mode,
                preloaded_askey.take(),
                cli.noclip || config.default_noclip,
            );
            if res.is_err() && file_path.is_some() {
                res?;
            }
            if file_path.is_some() {
                break;
            }
            play_file = None;
        } else {
            break;
        }
    }

    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    )?;
    terminal.show_cursor()?;

    Ok(())
}

struct SelectedAnimCache {
    filename: String,
    title: String,
    frames_count: usize,
    def_delay: u64,
    file_size_str: String,
    format_version: String,
    first_frame: Option<parser::ParsedFrame>,
}

fn run_selector(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> anyhow::Result<Option<(PathBuf, crate::models::AskeyFile)>> {
    let mut animations = get_animations().unwrap_or_default();
    let mut selected_idx = 0;
    let mut status_message: Option<(String, Instant, bool)> = None;
    let mut paste_buffer = String::new();
    let mut last_key_time = Instant::now();
    let mut show_help_dialog = false;

    let mut show_config_screen = false;
    let mut active_config_item = 0usize;
    let mut temp_config = models::AppConfig::default();

    let mut last_selected_idx: Option<usize> = None;
    let mut selected_cache: Option<SelectedAnimCache> = None;
    let mut preview_loading = false;

    // Channels for background preview loading
    let (req_tx, req_rx) = std::sync::mpsc::channel::<(usize, PathBuf, String)>();
    let (res_tx, res_rx) = std::sync::mpsc::channel::<(usize, Result<SelectedAnimCache, String>)>();

    // Spawn preview loader worker thread
    std::thread::spawn(move || {
        while let Ok(r) = req_rx.recv() {
            let mut req = r;
            while let Ok(next_req) = req_rx.try_recv() {
                req = next_req;
            }
            let (idx, path, filename) = req;
        let res = match parser::load_askey_file(&path) {
            Ok(askey) => {
                let size_on_disk = path.metadata().map(|m| m.len()).unwrap_or(0);
                let file_size_str = format!("{:.2} KB", size_on_disk as f64 / 1024.0);
                let total_frames = match &askey.fr {
                    crate::models::Frames::Simple(v) => v.len(),
                    crate::models::Frames::Detailed(v) => v.len(),
                };
                let def_delay = askey.d.unwrap_or(100);
                let title = askey
                    .n
                    .clone()
                    .unwrap_or_else(|| "Unnamed Animation".to_string());
                let format_version = askey.v.clone();
                let first_frame = parser::parse_first_frame(&askey);

                Ok(SelectedAnimCache {
                    filename,
                    title,
                    frames_count: total_frames,
                    def_delay,
                    file_size_str,
                    format_version,
                    first_frame,
                })
            }
            Err(e) => Err(e.to_string()),
        };
            if res_tx.send((idx, res)).is_err() {
                break;
            }
        }
    });

    // Channel/State for Enter-triggered full loader
    let mut full_loader_rx: Option<
        std::sync::mpsc::Receiver<anyhow::Result<crate::models::AskeyFile>>,
    > = None;
    let mut full_loader_path: Option<PathBuf> = None;
    let mut start_loading_time: Option<Instant> = None;

    loop {
        // Clear expired status message
        if let Some((_, timestamp, _)) = &status_message {
            if timestamp.elapsed() > Duration::from_secs(3) {
                status_message = None;
            }
        }

        // Handle full animation loader updates
        if let Some(ref rx) = full_loader_rx {
            if let Ok(res) = rx.try_recv() {
                full_loader_rx = None;
                let path = full_loader_path.take().unwrap();
                match res {
                    Ok(askey) => return Ok(Some((path, askey))),
                    Err(e) => {
                        status_message = Some((format!("Load error: {}", e), Instant::now(), true));
                    }
                }
            }
        }

        if full_loader_rx.is_none()
            && (last_selected_idx != Some(selected_idx) || selected_cache.is_none())
        {
            last_selected_idx = Some(selected_idx);
            selected_cache = None;
            if !animations.is_empty() && selected_idx < animations.len() {
                let (path, filename) = &animations[selected_idx];
                preview_loading = true;
                let _ = req_tx.send((selected_idx, path.clone(), filename.clone()));
            } else {
                preview_loading = false;
            }
        }
        while let Ok((idx, result)) = res_rx.try_recv() {
            if idx == selected_idx {
                preview_loading = false;
                if let Ok(cache) = result {
                    selected_cache = Some(cache);
                } else {
                    selected_cache = None;
                }
            }
        }

        terminal.draw(|f| {
            draw_selector_screen(
                f,
                SelectorScreenContext {
                    animations: &animations,
                    selected_idx,
                    selected_cache: &selected_cache,
                    preview_loading,
                    status_message: &status_message,
                    show_help_dialog,
                    full_loading_path: &full_loader_path,
                    start_loading_time,
                },
            );
            if show_config_screen {
                draw_config_settings_screen(f, &temp_config, active_config_item);
            }
        })?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                    // Ignore inputs during loading
                    if full_loader_rx.is_some() {
                        continue;
                    }

                    if last_key_time.elapsed() > Duration::from_millis(300) {
                        paste_buffer.clear();
                    }
                    last_key_time = Instant::now();

                    if show_help_dialog {
                        show_help_dialog = false;
                        continue;
                    }

                    if show_config_screen {
                        if key.code == KeyCode::Esc {
                            show_config_screen = false;
                            continue;
                        }
                        if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
                            active_config_item = (active_config_item + 1) % 2;
                            continue;
                        }
                        if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
                            active_config_item = if active_config_item == 0 { 1 } else { 0 };
                            continue;
                        }
                        if key.code == KeyCode::Left
                            || key.code == KeyCode::Right
                            || key.code == KeyCode::Char(' ')
                        {
                            if active_config_item == 0 {
                                temp_config.default_noclip = !temp_config.default_noclip;
                            } else {
                                temp_config.default_dashboard = !temp_config.default_dashboard;
                            }
                            continue;
                        }
                        if key.code == KeyCode::Enter {
                            if let Ok(dir) = get_app_dir() {
                                let path = dir.join("config.json");
                                if let Ok(file) = std::fs::File::create(path) {
                                    let _ = serde_json::to_writer_pretty(file, &temp_config);
                                }
                            }
                            show_config_screen = false;
                            continue;
                        }
                        continue;
                    }

                    if key.code == KeyCode::Char('?') {
                        std::thread::sleep(Duration::from_millis(15));
                        if event::poll(Duration::from_millis(0)).unwrap_or(false) {
                            paste_buffer.push('?');
                            last_key_time = Instant::now();
                            continue;
                        }
                        show_help_dialog = true;
                        continue;
                    }

                    if key.code == KeyCode::Char('c') || key.code == KeyCode::Char('C') {
                        std::thread::sleep(Duration::from_millis(15));
                        if event::poll(Duration::from_millis(0)).unwrap_or(false) {
                            paste_buffer.push(if key.code == KeyCode::Char('c') { 'c' } else { 'C' });
                            last_key_time = Instant::now();
                            continue;
                        }
                        temp_config = load_or_create_config().unwrap_or_default();
                        active_config_item = 0;
                        show_config_screen = true;
                        continue;
                    }

                    if key.code == KeyCode::Esc {
                        return Ok(None);
                    }

                    if !animations.is_empty() {
                        if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
                            selected_idx = (selected_idx + 1) % animations.len();
                        }
                        if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
                            if selected_idx == 0 {
                                selected_idx = animations.len() - 1;
                            } else {
                                selected_idx -= 1;
                            }
                        }
                        if key.code == KeyCode::Enter {
                            let (path, _) = &animations[selected_idx];
                            full_loader_path = Some(path.clone());
                            start_loading_time = Some(Instant::now());
                            let path_clone = path.clone();
                            let (tx, rx) = std::sync::mpsc::channel();
                            std::thread::spawn(move || {
                                let res = parser::load_askey_file(&path_clone);
                                let _ = tx.send(res);
                            });
                            full_loader_rx = Some(rx);
                            continue;
                        }
                        if key.code == KeyCode::Delete {
                            let (path, filename) = &animations[selected_idx];
                            if std::fs::remove_file(path).is_ok() {
                                status_message =
                                    Some((format!("Deleted {}", filename), Instant::now(), false));
                                animations = get_animations().unwrap_or_default();
                                if selected_idx >= animations.len() && !animations.is_empty() {
                                    selected_idx = animations.len() - 1;
                                }
                                last_selected_idx = None;
                            } else {
                                status_message = Some((
                                    format!("Failed to delete {}", filename),
                                    Instant::now(),
                                    true,
                                ));
                            }
                        }
                    }

                    if let KeyCode::Char(ch) = key.code {
                        paste_buffer.push(ch);
                        let cleaned = paste_buffer
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .trim()
                            .to_string();
                        let path = Path::new(&cleaned);
                        if path.is_file() {
                            let ext = path
                                .extension()
                                .map(|s| s.to_ascii_lowercase().to_string_lossy().into_owned())
                                .unwrap_or_default();

                            if ext == "gif"
                                || ext == "png"
                                || ext == "jpg"
                                || ext == "jpeg"
                                || ext == "webp"
                            {
                                paste_buffer.clear();
                                if let Ok(Some(dest_path)) =
                                    run_conversion_preview_screen(terminal, path)
                                {
                                    if let Ok(askey) = parser::load_askey_file(&dest_path) {
                                        return Ok(Some((dest_path, askey)));
                                    }
                                }
                                animations = get_animations().unwrap_or_default();
                                last_selected_idx = None;
                            } else if ext == "askey" {
                                paste_buffer.clear();
                                match import_animation(&cleaned) {
                                    Ok(new_path) => {
                                        let filename = new_path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned();
                                        status_message = Some((
                                            format!("Imported {}", filename),
                                            Instant::now(),
                                            false,
                                        ));
                                        animations = get_animations().unwrap_or_default();
                                        if let Some(pos) =
                                            animations.iter().position(|(p, _)| p == &new_path)
                                        {
                                            selected_idx = pos;
                                        }
                                        last_selected_idx = None;
                                    }
                                    Err(e) => {
                                        status_message = Some((
                                            format!("Import error: {}", e),
                                            Instant::now(),
                                            true,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                Event::Paste(data) => {
                    if full_loader_rx.is_some() {
                        continue;
                    }
                    let cleaned = data.trim().trim_matches('"').trim_matches('\'').trim();
                    let path = Path::new(cleaned);
                    if path.is_file() {
                        let ext = path
                            .extension()
                            .map(|s| s.to_ascii_lowercase().to_string_lossy().into_owned())
                            .unwrap_or_default();

                        if ext == "gif"
                            || ext == "png"
                            || ext == "jpg"
                            || ext == "jpeg"
                            || ext == "webp"
                        {
                            if let Ok(Some(dest_path)) =
                                run_conversion_preview_screen(terminal, path)
                            {
                                if let Ok(askey) = parser::load_askey_file(&dest_path) {
                                    return Ok(Some((dest_path, askey)));
                                }
                            }
                            animations = get_animations().unwrap_or_default();
                            last_selected_idx = None;
                        } else if ext == "askey" {
                            match import_animation(cleaned) {
                                Ok(new_path) => {
                                    let filename = new_path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned();
                                    status_message = Some((
                                        format!("Imported {}", filename),
                                        Instant::now(),
                                        false,
                                    ));
                                    animations = get_animations().unwrap_or_default();
                                    if let Some(pos) =
                                        animations.iter().position(|(p, _)| p == &new_path)
                                    {
                                        selected_idx = pos;
                                    }
                                    last_selected_idx = None;
                                }
                                Err(e) => {
                                    status_message = Some((
                                        format!("Import error: {}", e),
                                        Instant::now(),
                                        true,
                                    ));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn strip_ansi(s: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    re.replace_all(s, "").into_owned()
}

fn run_conversion_preview_screen(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    source_path: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let preview_frames = match converter::load_preview_frames(source_path) {
        Ok(frames) => frames,
        Err(e) => {
            return Err(e);
        }
    };

    let mut width = 60u32;
    let mut preset = models::CharsetPreset::Blocks;
    let mut scale = 0.5f32;
    let mut quantize = models::QuantizationLevel::Standard;
    let mut active_setting = 0usize;

    let mut current_preview_ansi = converter::generate_preview_ansi(
        &preview_frames[0].0,
        width,
        preset.clone(),
        scale,
        quantize,
    );

    loop {
        terminal.draw(|f| {
            draw_conversion_settings_screen(
                f,
                width,
                &preset,
                scale,
                quantize,
                active_setting,
                &current_preview_ansi,
            );
        })?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    if key.code == KeyCode::Esc {
                        return Ok(None);
                    }

                    if key.code == KeyCode::Up {
                        if active_setting == 0 {
                            active_setting = 3;
                        } else {
                            active_setting -= 1;
                        }
                    }
                    if key.code == KeyCode::Down {
                        active_setting = (active_setting + 1) % 4;
                    }

                    let mut changed = false;

                    if key.code == KeyCode::Left {
                        match active_setting {
                            0 => {
                                if width > 20 {
                                    width -= 5;
                                    changed = true;
                                }
                            }
                            1 => {
                                preset = preset.prev();
                                changed = true;
                            }
                            2 => {
                                if scale > 0.15 {
                                    scale -= 0.05;
                                    changed = true;
                                }
                            }
                            3 => {
                                quantize = quantize.prev();
                                changed = true;
                            }
                            _ => {}
                        }
                    }

                    if key.code == KeyCode::Right {
                        match active_setting {
                            0 => {
                                if width < 150 {
                                    width += 5;
                                    changed = true;
                                }
                            }
                            1 => {
                                preset = preset.next();
                                changed = true;
                            }
                            2 => {
                                if scale < 1.0 {
                                    scale += 0.05;
                                    changed = true;
                                }
                            }
                            3 => {
                                quantize = quantize.next();
                                changed = true;
                            }
                            _ => {}
                        }
                    }

                    if changed {
                        current_preview_ansi = converter::generate_preview_ansi(
                            &preview_frames[0].0,
                            width,
                            preset.clone(),
                            scale,
                            quantize,
                        );
                    }

                    if key.code == KeyCode::Enter {
                        let askey_file = converter::convert_image_to_askey(
                            source_path,
                            width,
                            preset.clone(),
                            scale,
                            quantize,
                        )?;
                        let filename = source_path
                            .file_name()
                            .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?;
                        let mut dest_filename = PathBuf::from(filename);
                        dest_filename.set_extension("askey");
                        let dest_dir = get_animations_dir()?;
                        let dest_path = dest_dir.join(dest_filename);

                        use flate2::write::GzEncoder;
                        use flate2::Compression;
                        use std::io::Write;

                        let json_str = serde_json::to_string(&askey_file)?;
                        let file = std::fs::File::create(&dest_path)?;
                        let mut encoder = GzEncoder::new(file, Compression::best());
                        encoder.write_all(json_str.as_bytes())?;
                        encoder.finish()?;

                        return Ok(Some(dest_path));
                    }
                }
            }
        }
    }
}

fn play_animation(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    file_path: &Path,
    is_fetch: bool,
    is_minimal_mode: bool,
    loaded_askey: Option<crate::models::AskeyFile>,
    default_noclip: bool,
) -> anyhow::Result<()> {
    let askey = match loaded_askey {
        Some(a) => a,
        None => parser::load_askey_file(file_path)?,
    };
    let parsed_frames = parser::parse_frames(&askey);

    if parsed_frames.is_empty() {
        return Err(anyhow::anyhow!("No frames found."));
    }

    let mut current_frame_idx = 0;
    let mut is_playing = true;
    let mut speed_multiplier = 1.0f64;
    let mut show_system_info = is_fetch;
    let mut loop_is_minimal = is_minimal_mode;
    let mut last_frame_time = Instant::now();
    let start_playback_time = Instant::now();
    let mut loop_count = 0;
    let mut loop_is_noclip = default_noclip;
    let mut noclip_x_offset: i16 = 0;
    let mut noclip_y_offset: i16 = 0;
    let mut show_help_dialog = false;

    let sys_user = std::env::var("USERNAME")
        .or(std::env::var("USER"))
        .unwrap_or_else(|_| "User".into());
    let sys_host = std::env::var("COMPUTERNAME")
        .or(std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Localhost".into());
    let sys_os = std::env::consts::OS;
    let sys_arch = std::env::consts::ARCH;

    loop {
        terminal.draw(|f| {
            draw_player_screen(
                f,
                PlayerRenderContext {
                    file_path,
                    askey: &askey,
                    parsed_frames: &parsed_frames,
                    current_frame_idx,
                    is_playing,
                    speed_multiplier,
                    show_system_info,
                    loop_is_minimal,
                    loop_is_noclip,
                    noclip_x_offset,
                    noclip_y_offset,
                    show_help_dialog,
                    sys_user: &sys_user,
                    sys_host: &sys_host,
                    sys_os,
                    sys_arch,
                    start_playback_time,
                    loop_count,
                },
            );
        })?;

        // 10ms poll for inputs
        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    if show_help_dialog {
                        show_help_dialog = false;
                        continue;
                    }

                    if key.code == KeyCode::Char('?') {
                        show_help_dialog = true;
                        continue;
                    }

                    if (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                        || key.code == KeyCode::Esc
                        || key.code == KeyCode::Char('q')
                        || key.code == KeyCode::Char('Q')
                    {
                        break;
                    }

                    if key.code == KeyCode::Char(' ') {
                        is_playing = !is_playing;
                    }

                    if key.code == KeyCode::Char('r') || key.code == KeyCode::Char('R') {
                        current_frame_idx = 0;
                        loop_count = 0;
                        last_frame_time = Instant::now();
                    }

                    if key.code == KeyCode::Char('i') || key.code == KeyCode::Char('I') {
                        show_system_info = !show_system_info;
                    }

                    if key.code == KeyCode::Char('m') || key.code == KeyCode::Char('M') {
                        loop_is_minimal = !loop_is_minimal;
                    }

                    if key.code == KeyCode::Char('n') || key.code == KeyCode::Char('N') {
                        loop_is_noclip = !loop_is_noclip;
                        if !loop_is_noclip {
                            noclip_x_offset = 0;
                            noclip_y_offset = 0;
                        }
                    }

                    if loop_is_noclip {
                        if key.code == KeyCode::Up {
                            noclip_y_offset -= 1;
                        }
                        if key.code == KeyCode::Down {
                            noclip_y_offset += 1;
                        }
                        if key.code == KeyCode::Left {
                            noclip_x_offset -= 1;
                        }
                        if key.code == KeyCode::Right {
                            noclip_x_offset += 1;
                        }
                    } else {
                        if !is_playing {
                            if key.code == KeyCode::Right {
                                current_frame_idx = (current_frame_idx + 1) % parsed_frames.len();
                            }
                            if key.code == KeyCode::Left {
                                if current_frame_idx == 0 {
                                    current_frame_idx = parsed_frames.len() - 1;
                                } else {
                                    current_frame_idx -= 1;
                                }
                            }
                        }
                    }

                    if key.code == KeyCode::Char('s') || key.code == KeyCode::Char('S') {
                        speed_multiplier = (speed_multiplier + 0.1).min(4.0);
                    }
                    if key.code == KeyCode::Char('f') || key.code == KeyCode::Char('F') {
                        speed_multiplier = (speed_multiplier - 0.1).max(0.1);
                    }
                }
            }
        }

        if is_playing {
            let frame = &parsed_frames[current_frame_idx];
            let actual_delay =
                Duration::from_millis((frame.delay as f64 / speed_multiplier) as u64);
            if last_frame_time.elapsed() >= actual_delay {
                let next_idx = current_frame_idx + 1;
                if next_idx >= parsed_frames.len() {
                    current_frame_idx = 0;
                    loop_count += 1;
                } else {
                    current_frame_idx = next_idx;
                }
                last_frame_time = Instant::now();
            }
        }
    }

    Ok(())
}

fn draw_font_check_screen(f: &mut ratatui::Frame) {
    let size = f.area();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White))
        .title(Span::styled(
            " asꄗ nerd font check ",
            Style::default().add_modifier(Modifier::BOLD),
        ));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Welcome to asꄗ!",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  This application uses Nerd Font icons to render its user interface panels,"),
        Line::from("  timeline, stats, and guides. Please check if you can see the icons below:"),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  [ 󰉋 library ]  ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  [ 󰐊 play ]  ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  [  warning ]  ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  [ 󰄬 success ]  ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from("  If you see blank boxes, question marks, or broken symbols instead of the"),
        Line::from(
            "  folder, play triangle, warning sign, or checkmark icons above, your terminal",
        ),
        Line::from("  is not configured with a Nerd Font."),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Please download and install a Nerd Font from: "),
            Span::styled(
                "https://www.nerdfonts.com/font-downloads",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::UNDERLINED),
            ),
        ]),
        Line::from(
            "  and configure your terminal emulator to use it (e.g. JetBrainsMono Nerd Font).",
        ),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  Press [Enter] to continue to the library...",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  Press [Esc] or [q] to exit...",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(text).block(block);

    let area = centered_rect(80, 65, size);
    f.render_widget(Clear, area);
    f.render_widget(paragraph, area);
}

fn run_font_check(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<bool> {
    loop {
        terminal.draw(|f| {
            draw_font_check_screen(f);
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') {
                        let dir = get_app_dir()?;
                        let flag = dir.join("font_check_passed.flag");
                        std::fs::File::create(flag)?;
                        return Ok(true);
                    }
                    if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                        return Ok(false);
                    }
                }
            }
        }
    }
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn centered_rect_fixed(
    width: u16,
    height: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(r.height.saturating_sub(height) / 2),
            Constraint::Length(height.min(r.height)),
            Constraint::Length(r.height.saturating_sub(height) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(r.width.saturating_sub(width) / 2),
            Constraint::Length(width.min(r.width)),
            Constraint::Length(r.width.saturating_sub(width) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[derive(Clone, Copy)]
struct SelectorScreenContext<'a> {
    animations: &'a [(PathBuf, String)],
    selected_idx: usize,
    selected_cache: &'a Option<SelectedAnimCache>,
    preview_loading: bool,
    status_message: &'a Option<(String, Instant, bool)>,
    show_help_dialog: bool,
    full_loading_path: &'a Option<PathBuf>,
    start_loading_time: Option<Instant>,
}

fn draw_selector_screen(
    f: &mut ratatui::Frame,
    ctx: SelectorScreenContext,
) {
    let SelectorScreenContext {
        animations,
        selected_idx,
        selected_cache,
        preview_loading,
        status_message,
        show_help_dialog,
        full_loading_path,
        start_loading_time,
    } = ctx;
    let size = f.area();

    let layout_constraints = if status_message.is_some() {
        vec![
            Constraint::Length(3), // Header
            Constraint::Min(5),    // Middle Panel
            Constraint::Length(3), // Drag & Drop Zone / Status Bar
        ]
    } else {
        vec![
            Constraint::Length(3), // Header
            Constraint::Min(5),    // Middle Panel
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(layout_constraints)
        .split(size);

    let header_text = vec![Line::from(vec![
        Span::styled(
            " asꄗ library ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │  Select an animation to play, or drag & drop files here"),
    ])];
    let header_widget = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(header_widget, chunks[0]);

    let is_small = size.width < 100 || size.height < 24;

    let max_len = animations
        .iter()
        .map(|(_, name)| name.chars().count())
        .max()
        .unwrap_or(0);
    let max_pct = if is_small { 30 } else { 50 };
    let max_allowed = ((size.width as usize * max_pct) / 100)
        .max(20)
        .min(size.width as usize);
    let list_width = ((max_len + 8).max(20).min(max_allowed)) as u16;

    let middle_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(list_width), Constraint::Min(20)])
        .split(chunks[1]);

    let list_items: Vec<ListItem> = if animations.is_empty() {
        vec![ListItem::new(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                "No animations found. Drag & drop a .askey file here!",
                Style::default().fg(Color::DarkGray),
            ),
        ]))]
    } else {
        animations
            .iter()
            .enumerate()
            .map(|(idx, (_, filename))| {
                if idx == selected_idx {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            "  ",
                            Style::default()
                                .fg(Color::Black)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            filename.clone(),
                            Style::default()
                                .fg(Color::Black)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]))
                    .style(Style::default().bg(Color::White).fg(Color::Black))
                } else {
                    ListItem::new(Line::from(vec![
                        Span::raw("   "),
                        Span::raw(filename.clone()),
                    ]))
                }
            })
            .collect()
    };

    let list_widget = List::new(list_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" saved animations ")
            .border_style(Style::default().fg(Color::White)),
    );
    f.render_widget(list_widget, middle_chunks[0]);

    let show_details = size.height > 44;

    let right_pane_chunks = if show_details {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(14)])
            .split(middle_chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5)])
            .split(middle_chunks[1])
    };

    if show_details {
        let details_widget = if let Some(ref cache) = selected_cache {
            let lines = vec![
                Line::from(vec![
                    Span::styled("󰈔 File:       ", Style::default().fg(Color::White)),
                    Span::raw(&cache.filename),
                ]),
                Line::from(vec![
                    Span::styled("󰓎 Title:      ", Style::default().fg(Color::White)),
                    Span::styled(&cache.title, Style::default().add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    Span::styled("󰕧 Frames:     ", Style::default().fg(Color::White)),
                    Span::raw(format!("{}", cache.frames_count)),
                ]),
                Line::from(vec![
                    Span::styled("󰔚 Def Delay:  ", Style::default().fg(Color::White)),
                    Span::raw(format!("{} ms", cache.def_delay)),
                ]),
                Line::from(vec![
                    Span::styled("󰗄 File Size:  ", Style::default().fg(Color::White)),
                    Span::raw(&cache.file_size_str),
                ]),
                Line::from(vec![
                    Span::styled("󰗀 Format v:   ", Style::default().fg(Color::White)),
                    Span::raw(&cache.format_version),
                ]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "󰌌 Controls:",
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
                Line::from(vec![
                    Span::styled(
                        "  [Enter]    ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("󰐊 Play animation"),
                ]),
                Line::from(vec![
                    Span::styled(
                        "  [Delete]   ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("󰆴 Delete animation"),
                ]),
                Line::from(vec![
                    Span::styled(
                        "  [C]        ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("󰘳 Configure Defaults"),
                ]),
                Line::from(vec![
                    Span::styled(
                        "  [?]        ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("󰘥 Help Guide"),
                ]),
                Line::from(vec![
                    Span::styled(
                        "  [Esc]      ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("󰈆 Exit Player"),
                ]),
            ];
            Paragraph::new(lines)
        } else {
            Paragraph::new(vec![
                Line::from(""),
                Line::from("  No animation selected."),
                Line::from("  Use Drag & Drop to import .askey files!"),
            ])
        };

        let details_block = Block::default()
            .borders(Borders::ALL)
            .title(" animation details ")
            .border_style(Style::default().fg(Color::DarkGray));
        f.render_widget(details_widget.block(details_block), right_pane_chunks[1]);
    }

    let preview_block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " preview (first frame) ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::DarkGray));

    if preview_loading {
        let msg = "\n\n  Loading preview...";
        let paragraph = Paragraph::new(msg).block(preview_block);
        f.render_widget(paragraph, right_pane_chunks[0]);
    } else if let Some(ref cache) = selected_cache {
        if let Some(ref frame) = cache.first_frame {
            let frame_w = frame.width;
            let frame_h = frame.height;

            let preview_inner = right_pane_chunks[0].inner(Margin {
                vertical: 1,
                horizontal: 1,
            });
            let preview_w = preview_inner.width;
            let preview_h = preview_inner.height;

            let pad_x = if preview_w > frame_w {
                (preview_w - frame_w) / 2
            } else {
                0
            };
            let pad_y = if preview_h > frame_h {
                (preview_h - frame_h) / 2
            } else {
                0
            };

            let mut centered_ansi = String::new();
            for _ in 0..pad_y {
                centered_ansi.push('\n');
            }
            let pad_spaces = " ".repeat(pad_x as usize);
            let mut lines_iter = frame.ansi_content.split('\n').peekable();
            while let Some(line) = lines_iter.next() {
                centered_ansi.push_str(&pad_spaces);
                centered_ansi.push_str(line);
                if lines_iter.peek().is_some() {
                    centered_ansi.push('\n');
                }
            }

            if let Ok(tui_text) = centered_ansi.as_bytes().into_text() {
                let paragraph = Paragraph::new(tui_text).block(preview_block);
                f.render_widget(paragraph, right_pane_chunks[0]);
            } else {
                let paragraph = Paragraph::new(centered_ansi).block(preview_block);
                f.render_widget(paragraph, right_pane_chunks[0]);
            }
        } else {
            f.render_widget(
                Paragraph::new("No frames to preview").block(preview_block),
                right_pane_chunks[0],
            );
        }
    } else {
        f.render_widget(
            Paragraph::new("No animation loaded").block(preview_block),
            right_pane_chunks[0],
        );
    }

    if let Some((msg, _, is_error)) = &status_message {
        let color = Color::White;
        let modifier = if *is_error {
            Modifier::REVERSED
        } else {
            Modifier::BOLD
        };
        let status_text = format!(" {}", msg);
        let status_style = Style::default().fg(color).add_modifier(modifier);

        let status_widget = Paragraph::new(status_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .style(status_style);
        f.render_widget(status_widget, chunks[2]);
    }

    if show_help_dialog {
        let block = Block::default()
            .title(Span::styled(
                " Keyboard Shortcuts Help ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::White));

        let help_text = vec![
            Line::from(vec![
                Span::styled(
                    "  [▲/▼] or [j/k] ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("Navigate the animations list"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [Enter]        ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰐊 Play the selected animation"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [Delete]       ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰆴 Delete animation from disk"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [Esc]          ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰈆 Exit program / selector"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [C]            ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘳 Configure default playback modes"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [?]            ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘥 Toggle this help dialog"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "  Drag & Drop:   ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("Drop any .askey, GIF, PNG, or JPG file"),
            ]),
            Line::from(vec![
                Span::raw("                 "),
                Span::raw("anywhere on screen to import & convert!"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "  GitHub:        ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("https://github.com/kalmix/askey-cli"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "  Press any key to close  ",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
        ];

        let paragraph = Paragraph::new(help_text).block(block);
        let area = centered_rect(65, 65, size);
        f.render_widget(Clear, area);
        f.render_widget(paragraph, area);
    }

    if let Some(ref path) = full_loading_path {
        let popup_area = centered_rect(50, 25, size);
        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Loading Animation ")
            .border_style(Style::default().fg(Color::White));

        let elapsed = start_loading_time
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(0);
        let frame_tick = (elapsed / 100) % 4;
        let dots = match frame_tick {
            0 => ".  ",
            1 => ".. ",
            2 => "...",
            _ => "   ",
        };
        let file_stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let msg = format!("\n  Loading \"{}\"{}\n  Please wait...", file_stem, dots);
        let paragraph = Paragraph::new(msg)
            .block(block)
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(paragraph, popup_area);
    }
}

fn draw_config_settings_screen(
    f: &mut ratatui::Frame,
    config: &models::AppConfig,
    active_item: usize,
) {
    let size = f.area();

    // Get paths
    let config_path_str = get_app_dir()
        .map(|d| d.join("config.json").display().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());
    let anims_path_str = get_animations_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    // Calculate dynamic popup width based on paths
    let max_path_len = config_path_str.len().max(anims_path_str.len());
    let needed_width = (max_path_len as u16 + 22).max(80);
    let popup_width = needed_width.min(size.width);
    let popup_height = 18.min(size.height);

    let popup_area = centered_rect_fixed(popup_width, popup_height, size);
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White))
        .title(Span::styled(
            " asꄗ default playback configuration ",
            Style::default().add_modifier(Modifier::BOLD),
        ));

    let mut lines = vec![
        Line::from(""),
        Line::from("  Adjust default playback options. These take effect when playing animations:"),
        Line::from(""),
    ];

    let noclip_style = if active_item == 0 {
        Style::default()
            .bg(Color::White)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    lines.push(Line::from(vec![
        Span::styled(if active_item == 0 { "  " } else { "   " }, noclip_style),
        Span::styled("Default No-Clip Mode:  ", noclip_style),
        Span::styled(
            if config.default_noclip {
                "[ ON ]"
            } else {
                "[ OFF ]"
            },
            Style::default()
                .fg(if config.default_noclip {
                    Color::White
                } else {
                    Color::DarkGray
                })
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(""));

    let dash_style = if active_item == 1 {
        Style::default()
            .bg(Color::White)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    lines.push(Line::from(vec![
        Span::styled(if active_item == 1 { "  " } else { "   " }, dash_style),
        Span::styled("Default Dashboard:    ", dash_style),
        Span::styled(
            if config.default_dashboard {
                "[ ON ]"
            } else {
                "[ OFF ]"
            },
            Style::default()
                .fg(if config.default_dashboard {
                    Color::White
                } else {
                    Color::DarkGray
                })
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  📁 Paths (click to open):",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::raw("    Config File: "),
        Span::styled(
            &config_path_str,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    Animations:  "),
        Span::styled(
            &anims_path_str,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Controls:",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from("    [▲/▼]      Navigate settings"));
    lines.push(Line::from("    [◀/▶/Space] Toggle value"));
    lines.push(Line::from(vec![
        Span::styled(
            "    [Enter]    ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Save config to disk & close"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            "    [Esc]      ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Cancel and discard changes"),
    ]));

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, popup_area);
}

fn draw_conversion_settings_screen(
    f: &mut ratatui::Frame,
    width: u32,
    preset: &models::CharsetPreset,
    scale: f32,
    quantize: models::QuantizationLevel,
    active_setting: usize,
    current_preview_ansi: &str,
) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(size);

    let header_text = vec![Line::from(vec![
        Span::styled(
            " asꄗ import settings ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │  Configure conversion options and see live preview"),
    ])];
    let header_widget = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(header_widget, chunks[0]);

    let middle_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(35), Constraint::Min(20)])
        .split(chunks[1]);

    let mut settings_lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Conversion Settings:",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
    ];

    let width_style = if active_setting == 0 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let width_marker = if active_setting == 0 { " ❯ " } else { "   " };
    settings_lines.push(Line::from(vec![
        Span::styled(width_marker, width_style),
        Span::styled("󰃶 Width:        ", width_style),
        Span::raw(" ◀ "),
        Span::styled(
            format!("{:^3}", width),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ▶ "),
    ]));
    settings_lines.push(Line::from(""));

    let charset_style = if active_setting == 1 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let charset_marker = if active_setting == 1 { " ❯ " } else { "   " };
    settings_lines.push(Line::from(vec![
        Span::styled(charset_marker, charset_style),
        Span::styled("󰺣 Charset:      ", charset_style),
        Span::raw(" ◀ "),
        Span::styled(
            format!("{:^8}", preset.as_str()),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ▶ "),
    ]));
    settings_lines.push(Line::from(""));

    let scale_style = if active_setting == 2 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let scale_marker = if active_setting == 2 { " ❯ " } else { "   " };
    settings_lines.push(Line::from(vec![
        Span::styled(scale_marker, scale_style),
        Span::styled("󰹅 Aspect Scale: ", scale_style),
        Span::raw(" ◀ "),
        Span::styled(
            format!("{:^4.2}", scale),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ▶ "),
    ]));
    settings_lines.push(Line::from(""));

    let quantize_style = if active_setting == 3 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let quantize_marker = if active_setting == 3 { " ❯ " } else { "   " };
    settings_lines.push(Line::from(vec![
        Span::styled(quantize_marker, quantize_style),
        Span::styled("󰓎 Quantize:      ", quantize_style),
        Span::raw(" ◀ "),
        Span::styled(
            format!("{:^15}", quantize.description()),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ▶ "),
    ]));
    settings_lines.push(Line::from(""));
    settings_lines.push(Line::from(""));

    settings_lines.push(Line::from(vec![Span::styled(
        "  󰌌 Navigation:",
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    settings_lines.push(Line::from(vec![
        Span::styled(
            "    [▲/▼] ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("󰌌 Select Setting"),
    ]));
    settings_lines.push(Line::from(vec![
        Span::styled(
            "    [◀/▶] ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("󰘳 Adjust Value"),
    ]));
    settings_lines.push(Line::from(""));
    settings_lines.push(Line::from(vec![
        Span::styled(
            "    [Enter]  ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("󰄬 Save & Play Minimal"),
    ]));
    settings_lines.push(Line::from(vec![
        Span::styled(
            "    [Esc]    ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("󰅖 Cancel Import"),
    ]));

    let settings_widget = Paragraph::new(settings_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" settings ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(settings_widget, middle_chunks[0]);

    let preview_block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " live preview (first frame) ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::DarkGray));

    let preview_inner = middle_chunks[1].inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let preview_w = preview_inner.width;
    let preview_h = preview_inner.height;

    let first_line = current_preview_ansi.split('\n').next().unwrap_or("");
    let plain_first_line = strip_ansi(first_line);
    let frame_w = plain_first_line.chars().count() as u16;
    let frame_h = current_preview_ansi.split('\n').count() as u16;

    if preview_w < frame_w || preview_h < frame_h {
        let mut warning_ansi = String::new();
        let warn_text = " Preview area too small! Increase terminal size or reduce width.";
        let pad_y = if preview_h > 1 {
            (preview_h - 1) / 2
        } else {
            0
        };
        for _ in 0..pad_y {
            warning_ansi.push('\n');
        }
        let warn_len = warn_text.chars().count() as u16;
        let pad_x = if preview_w > warn_len {
            (preview_w - warn_len) / 2
        } else {
            0
        };
        warning_ansi.push_str(&" ".repeat(pad_x as usize));
        warning_ansi.push_str(warn_text);

        let paragraph = Paragraph::new(Span::styled(
            warning_ansi,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))
        .block(preview_block);
        f.render_widget(paragraph, middle_chunks[1]);
    } else {
        let pad_x = (preview_w - frame_w) / 2;
        let pad_y = (preview_h - frame_h) / 2;

        let mut centered_ansi = String::new();
        for _ in 0..pad_y {
            centered_ansi.push('\n');
        }
        let pad_spaces = " ".repeat(pad_x as usize);
        let mut lines_iter = current_preview_ansi.split('\n').peekable();
        while let Some(line) = lines_iter.next() {
            centered_ansi.push_str(&pad_spaces);
            centered_ansi.push_str(line);
            if lines_iter.peek().is_some() {
                centered_ansi.push('\n');
            }
        }

        if let Ok(tui_text) = centered_ansi.as_bytes().into_text() {
            let paragraph = Paragraph::new(tui_text).block(preview_block);
            f.render_widget(paragraph, middle_chunks[1]);
        } else {
            let paragraph = Paragraph::new(centered_ansi).block(preview_block);
            f.render_widget(paragraph, middle_chunks[1]);
        }
    }
}

#[derive(Clone, Copy)]
struct PlayerRenderContext<'a> {
    file_path: &'a Path,
    askey: &'a crate::models::AskeyFile,
    parsed_frames: &'a [parser::ParsedFrame],
    current_frame_idx: usize,
    is_playing: bool,
    speed_multiplier: f64,
    show_system_info: bool,
    loop_is_minimal: bool,
    loop_is_noclip: bool,
    noclip_x_offset: i16,
    noclip_y_offset: i16,
    show_help_dialog: bool,
    sys_user: &'a str,
    sys_host: &'a str,
    sys_os: &'a str,
    sys_arch: &'a str,
    start_playback_time: Instant,
    loop_count: usize,
}

fn draw_player_screen(
    f: &mut ratatui::Frame,
    ctx: PlayerRenderContext,
) {
    let PlayerRenderContext {
        file_path,
        askey,
        parsed_frames,
        current_frame_idx,
        is_playing,
        speed_multiplier,
        show_system_info,
        loop_is_minimal,
        loop_is_noclip,
        noclip_x_offset,
        noclip_y_offset,
        show_help_dialog,
        sys_user,
        sys_host,
        sys_os,
        sys_arch,
        start_playback_time,
        loop_count,
    } = ctx;
    let size = f.area();

    if loop_is_minimal {
        let canvas_area = size;
        let canvas_w = canvas_area.width;
        let canvas_h = canvas_area.height;
        let frame = &parsed_frames[current_frame_idx];

        let canvas_block = Block::default();

        if !loop_is_noclip && (canvas_w < frame.width || canvas_h < frame.height) {
            let mut warning_ansi = String::new();
            let warn_text = " Terminal size too small! Please zoom out (Ctrl -) or increase terminal size. Or press N to enable no-clip mode.";
            let pad_y = if canvas_h > 1 { (canvas_h - 1) / 2 } else { 0 };
            for _ in 0..pad_y {
                warning_ansi.push('\n');
            }
            let warn_len = warn_text.chars().count() as u16;
            let pad_x = if canvas_w > warn_len {
                (canvas_w - warn_len) / 2
            } else {
                0
            };
            warning_ansi.push_str(&" ".repeat(pad_x as usize));
            warning_ansi.push_str(warn_text);

            let paragraph = Paragraph::new(Span::styled(
                warning_ansi,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ))
            .block(canvas_block);
            f.render_widget(paragraph, canvas_area);
        } else {
            let base_offset_x = (canvas_w as i16 - frame.width as i16) / 2;
            let base_offset_y = (canvas_h as i16 - frame.height as i16) / 2;

            let h_offset = base_offset_x + noclip_x_offset;
            let v_offset = base_offset_y + noclip_y_offset;

            let (pad_x, scroll_x) = if h_offset >= 0 {
                (h_offset as u16, 0u16)
            } else {
                (0u16, h_offset.unsigned_abs())
            };

            let (pad_y, scroll_y) = if v_offset >= 0 {
                (v_offset as u16, 0u16)
            } else {
                (0u16, v_offset.unsigned_abs())
            };

            let mut centered_ansi = String::new();
            for _ in 0..pad_y {
                centered_ansi.push('\n');
            }
            let pad_spaces = " ".repeat(pad_x as usize);
            let mut lines_iter = frame.ansi_content.split('\n').peekable();
            while let Some(line) = lines_iter.next() {
                centered_ansi.push_str(&pad_spaces);
                centered_ansi.push_str(line);
                if lines_iter.peek().is_some() {
                    centered_ansi.push('\n');
                }
            }

            if let Ok(tui_text) = centered_ansi.as_bytes().into_text() {
                let paragraph = Paragraph::new(tui_text)
                    .block(canvas_block)
                    .scroll((scroll_y, scroll_x));
                f.render_widget(paragraph, canvas_area);
            } else {
                let paragraph = Paragraph::new(centered_ansi)
                    .block(canvas_block)
                    .scroll((scroll_y, scroll_x));
                f.render_widget(paragraph, canvas_area);
            }
        }
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(3),
            ])
            .split(size);

        let header_area = chunks[0];
        let middle_area = chunks[1];
        let footer_area = chunks[2];

        let anim_name = askey.n.as_deref().unwrap_or("Unnamed Animation");
        let anim_name_no_ext = Path::new(anim_name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| anim_name.to_string());

        let formatted_file_path = if let Ok(anim_dir) = get_animations_dir() {
            if file_path.starts_with(&anim_dir) {
                let filename = file_path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let filename_no_ext = Path::new(&filename)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or(filename);
                format!(r"%appdata%\askey\animations\{}", filename_no_ext)
            } else {
                let filename = file_path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let filename_no_ext = Path::new(&filename)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or(filename);
                if let Some(parent) = file_path.parent() {
                    parent
                        .join(filename_no_ext)
                        .display()
                        .to_string()
                        .replace('\\', "/")
                } else {
                    filename_no_ext
                }
            }
        } else {
            file_path.display().to_string().replace('\\', "/")
        };

        let header_text = vec![Line::from(vec![
            Span::styled(
                " asꄗ player ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" │  󰈔 File: "),
            Span::styled(formatted_file_path, Style::default().fg(Color::White)),
            Span::raw("  │  󰐌 Animation: "),
            Span::styled(
                anim_name_no_ext,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ])];
        let header_widget = Paragraph::new(header_text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        f.render_widget(header_widget, header_area);

        let middle_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(middle_area);

        let canvas_area = middle_chunks[0];
        let sidebar_area = middle_chunks[1];

        let frame = &parsed_frames[current_frame_idx];

        let canvas_inner = canvas_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let canvas_w = canvas_inner.width;
        let canvas_h = canvas_inner.height;

        let canvas_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " 󰐌 canvas ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));

        if !loop_is_noclip && (canvas_w < frame.width || canvas_h < frame.height) {
            let mut warning_ansi = String::new();
            let warn_text = " Terminal size too small! Please zoom out (Ctrl -) or increase terminal size. Or press N to enable no-clip mode.";
            let pad_y = if canvas_h > 1 { (canvas_h - 1) / 2 } else { 0 };
            for _ in 0..pad_y {
                warning_ansi.push('\n');
            }
            let warn_len = warn_text.chars().count() as u16;
            let pad_x = if canvas_w > warn_len {
                (canvas_w - warn_len) / 2
            } else {
                0
            };
            warning_ansi.push_str(&" ".repeat(pad_x as usize));
            warning_ansi.push_str(warn_text);

            let paragraph = Paragraph::new(Span::styled(
                warning_ansi,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ))
            .block(canvas_block);
            f.render_widget(paragraph, canvas_area);
        } else {
            let base_offset_x = (canvas_w as i16 - frame.width as i16) / 2;
            let base_offset_y = (canvas_h as i16 - frame.height as i16) / 2;

            let h_offset = base_offset_x + noclip_x_offset;
            let v_offset = base_offset_y + noclip_y_offset;

            let (pad_x, scroll_x) = if h_offset >= 0 {
                (h_offset as u16, 0u16)
            } else {
                (0u16, h_offset.unsigned_abs())
            };

            let (pad_y, scroll_y) = if v_offset >= 0 {
                (v_offset as u16, 0u16)
            } else {
                (0u16, v_offset.unsigned_abs())
            };

            let mut centered_ansi = String::new();
            for _ in 0..pad_y {
                centered_ansi.push('\n');
            }
            let pad_spaces = " ".repeat(pad_x as usize);
            let mut lines_iter = frame.ansi_content.split('\n').peekable();
            while let Some(line) = lines_iter.next() {
                centered_ansi.push_str(&pad_spaces);
                centered_ansi.push_str(line);
                if lines_iter.peek().is_some() {
                    centered_ansi.push('\n');
                }
            }

            if let Ok(tui_text) = centered_ansi.as_bytes().into_text() {
                let paragraph = Paragraph::new(tui_text)
                    .block(canvas_block)
                    .scroll((scroll_y, scroll_x));
                f.render_widget(paragraph, canvas_area);
            } else {
                let paragraph = Paragraph::new(centered_ansi)
                    .block(canvas_block)
                    .scroll((scroll_y, scroll_x));
                f.render_widget(paragraph, canvas_area);
            }
        }

        let show_player_details = size.height > 44;
        let mut sidebar_layout_constraints = vec![Constraint::Length(8)];
        if show_player_details {
            sidebar_layout_constraints.push(Constraint::Length(6));
        }
        if show_system_info {
            sidebar_layout_constraints.push(Constraint::Length(5));
        }
        sidebar_layout_constraints.push(Constraint::Min(5));

        let sidebar_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(sidebar_layout_constraints)
            .split(sidebar_area);

        let mut chunk_idx = 0;

        let elapsed_sec = start_playback_time.elapsed().as_secs();
        let elapsed_str = format!("{:02}:{:02}", elapsed_sec / 60, elapsed_sec % 60);
        let status_span = if is_playing {
            Span::styled(
                " Playing",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(" Paused", Style::default().fg(Color::DarkGray))
        };

        let playback_stats = vec![
            Line::from(vec![Span::raw("󰐊 Status:  "), status_span]),
            Line::from(vec![
                Span::raw("󰓅 Speed:   "),
                Span::styled(
                    format!("{:.2}x", speed_multiplier),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("󰕧 Frame:   "),
                Span::styled(
                    format!("{}/{}", current_frame_idx + 1, parsed_frames.len()),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("󰘳 No-Clip: "),
                Span::styled(
                    if loop_is_noclip {
                        "󰄬 On"
                    } else {
                        "󰅖 Off"
                    },
                    Style::default()
                        .fg(if loop_is_noclip {
                            Color::White
                        } else {
                            Color::DarkGray
                        })
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("󰔚 Time:    "),
                Span::styled(elapsed_str, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::raw("󰑓 Loops:   "),
                Span::styled(
                    format!("#{}", loop_count),
                    Style::default().fg(Color::White),
                ),
            ]),
        ];
        let stats_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " 󰓅 playback info ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));
        f.render_widget(
            Paragraph::new(playback_stats).block(stats_block),
            sidebar_chunks[chunk_idx],
        );
        chunk_idx += 1;

        if show_player_details {
            let metadata = vec![
                Line::from(vec![
                    Span::raw("Format: 󰗀 "),
                    Span::styled(&askey.v, Style::default().fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::raw("Delay:  󰔚 "),
                    Span::styled(
                        format!("{}ms", askey.d.unwrap_or(100)),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("Res:    󰇽 "),
                    Span::styled(
                        format!("{}x{}", frame.width, frame.height),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("FPS:    󰦺 "),
                    Span::styled(
                        format!("{:.1}", 1000.0 / askey.d.unwrap_or(100) as f64),
                        Style::default().fg(Color::White),
                    ),
                ]),
            ];
            let metadata_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " 󰗀 metadata ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ));
            f.render_widget(
                Paragraph::new(metadata).block(metadata_block),
                sidebar_chunks[chunk_idx],
            );
            chunk_idx += 1;
        }

        if show_system_info {
            let sys_info_lines = vec![
                Line::from(vec![
                    Span::raw("User:   󰭹 "),
                    Span::styled(
                        format!("{}@{}", sys_user, sys_host),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("OS:     󰨡 "),
                    Span::styled(
                        format!("{} / {}", sys_os, sys_arch),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("Term:   󰍹 "),
                    Span::styled(
                        format!("{}x{}", size.width, size.height),
                        Style::default().fg(Color::White),
                    ),
                ]),
            ];
            let sys_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " 󰨡 system fetch ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ));
            f.render_widget(
                Paragraph::new(sys_info_lines).block(sys_block),
                sidebar_chunks[chunk_idx],
            );
            chunk_idx += 1;
        }

        let mut controls_lines = vec![Line::from(vec![
            Span::styled(
                " [Space] ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("󰐊/󰏤 Play/Pause"),
        ])];

        if loop_is_noclip {
            controls_lines.push(Line::from(vec![
                Span::styled(
                    " [▲]/[▼] ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘳 Pan Vertical"),
            ]));
            controls_lines.push(Line::from(vec![
                Span::styled(
                    " [◀]/[▶] ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘳 Pan Horizontal"),
            ]));
        } else {
            controls_lines.push(Line::from(vec![
                Span::styled(
                    " [◀]/[▶] ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰕇 Step (Paused)"),
            ]));
        }

        controls_lines.extend(vec![
            Line::from(vec![
                Span::styled(
                    " [S]/[F] ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰓅 Faster / Slower"),
            ]),
            Line::from(vec![
                Span::styled(
                    " [I]     ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰭹 Toggle SysInfo"),
            ]),
            Line::from(vec![
                Span::styled(
                    " [M]     ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘳 Toggle Layout"),
            ]),
            Line::from(vec![
                Span::styled(
                    " [N]     ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰈈 Toggle No-Clip"),
            ]),
            Line::from(vec![
                Span::styled(
                    " [R]     ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰑓 Restart"),
            ]),
            Line::from(vec![
                Span::styled(
                    " [Esc]   ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰈆 Quit"),
            ]),
            Line::from(vec![
                Span::styled(
                    " [?]     ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘥 Help Guide"),
            ]),
        ]);
        let controls_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " 󰌌 controls ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));
        f.render_widget(
            Paragraph::new(controls_lines).block(controls_block),
            sidebar_chunks[chunk_idx],
        );

        let ratio = current_frame_idx as f64 / (parsed_frames.len() - 1).max(1) as f64;
        let current_delay = (frame.delay as f64 / speed_multiplier) as u64;
        let gauge_widget = Gauge::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(Span::styled(
                        format!(
                            " timeline (frame {}/{}) ",
                            current_frame_idx + 1,
                            parsed_frames.len()
                        ),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )),
            )
            .gauge_style(Style::default().fg(Color::White).bg(Color::DarkGray))
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!(
                "{:.0}% (Frame delay: {}ms)",
                ratio * 100.0,
                current_delay
            ));
        f.render_widget(gauge_widget, footer_area);
    }

    if show_help_dialog {
        let block = Block::default()
            .title(Span::styled(
                " keyboard shortcuts help ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::White));

        let mut help_text = vec![Line::from(vec![
            Span::styled(
                "  [Space]      ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("󰐊/󰏤 Play or pause playback"),
        ])];

        if loop_is_noclip {
            help_text.push(Line::from(vec![
                Span::styled(
                    "  [Arrow Keys] ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘳 Pan view on canvas (No-Clip Active)"),
            ]));
        } else {
            help_text.push(Line::from(vec![
                Span::styled(
                    "  [◀] / [▶]    ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰕇 Step frames backward/forward (paused)"),
            ]));
        }

        help_text.extend(vec![
            Line::from(vec![
                Span::styled(
                    "  [S] / [F]    ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰓅 Increase / decrease speed multiplier"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [I]          ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰭹 Toggle System Fetch stats sidebar"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [M]          ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘳 Toggle Minimal mode / Dashboard mode"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [N]          ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "󰈈 Toggle No-Clip mode (Currently: {})",
                    if loop_is_noclip { "ON" } else { "OFF" }
                )),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [R]          ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰑓 Restart animation from frame 0"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [Q] / [Esc]  ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰈆 Quit playback & exit to library"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  [?]          ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("󰘥 Close this help dialog"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "  GitHub:      ",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("https://github.com/kalmix/askey-cli"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "  Press any key to close  ",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
        ]);

        let paragraph = Paragraph::new(help_text).block(block);
        let area = centered_rect(65, 65, size);
        f.render_widget(Clear, area);
        f.render_widget(paragraph, area);
    }
}
