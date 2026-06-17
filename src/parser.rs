use crate::models::{AskeyFile, Frames};
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use owo_colors::OwoColorize;

pub struct ParsedFrame {
    pub ansi_content: String,
    pub width: u16,
    pub height: u16,
    pub delay: u64,
}

pub fn load_askey_file(path: &Path) -> Result<AskeyFile> {
    let file = File::open(path).context("Failed to open file")?;
    
    let mut gz = GzDecoder::new(&file);
    let mut s = String::new();
    
    if gz.read_to_string(&mut s).is_ok() {
        serde_json::from_str(&s).context("Failed to parse JSON from gzip")
    } else {
        let mut file = File::open(path)?;
        let mut s = String::new();
        file.read_to_string(&mut s)?;
        serde_json::from_str(&s).context("Failed to parse JSON")
    }
}

pub fn parse_hex_rgb(color_str: &str) -> Option<(u8, u8, u8)> {
    if color_str.starts_with('#') && color_str.len() == 7 {
        let r = u8::from_str_radix(&color_str[1..3], 16).ok()?;
        let g = u8::from_str_radix(&color_str[3..5], 16).ok()?;
        let b = u8::from_str_radix(&color_str[5..7], 16).ok()?;
        Some((r, g, b))
    } else {
        None
    }
}

fn parse_single_frame(
    content: &str,
    palette: &HashMap<String, String>,
    delay: u64,
    re: &Regex,
) -> ParsedFrame {
    let mut ansi_content = String::new();
    let mut last_idx = 0;
    let mut max_width = 0;
    let mut current_width = 0;
    let mut lines = 0;

    for cap in re.captures_iter(content) {
        let full_match = cap.get(0).unwrap();
        let start = full_match.start();
        let end = full_match.end();

        if start > last_idx {
            let plain_text = &content[last_idx..start];
            ansi_content.push_str(plain_text);
            for ch in plain_text.chars() {
                if ch == '\n' {
                    if current_width > max_width { max_width = current_width; }
                    current_width = 0;
                    lines += 1;
                } else {
                    current_width += 1;
                }
            }
        }

        let color_key = cap.get(1).unwrap().as_str();
        let inner_text = cap.get(2).unwrap().as_str();

        let mut color_hex = palette.get(color_key).map(|s| s.as_str());
        if color_hex.is_none() && !color_key.starts_with('c') {
            let prefixed = format!("c{}", color_key);
            color_hex = palette.get(&prefixed).map(|s| s.as_str());
        }
        let color_hex = color_hex.unwrap_or(color_key);

        if let Some((r, g, b)) = parse_hex_rgb(color_hex) {
            let colored = inner_text.truecolor(r, g, b).to_string();
            ansi_content.push_str(&colored);
        } else {
            ansi_content.push_str(inner_text);
        }

        for ch in inner_text.chars() {
            if ch == '\n' {
                if current_width > max_width { max_width = current_width; }
                current_width = 0;
                lines += 1;
            } else {
                current_width += 1;
            }
        }

        last_idx = end;
    }

    if last_idx < content.len() {
        let plain_text = &content[last_idx..];
        ansi_content.push_str(plain_text);
        for ch in plain_text.chars() {
            if ch == '\n' {
                if current_width > max_width { max_width = current_width; }
                current_width = 0;
                lines += 1;
            } else {
                current_width += 1;
            }
        }
    }

    if current_width > max_width { max_width = current_width; }
    if lines > 0 || max_width > 0 {
        lines += 1;
    }

    ParsedFrame {
        ansi_content,
        width: max_width as u16,
        height: lines as u16,
        delay,
    }
}

pub fn parse_frames(askey: &AskeyFile) -> Vec<ParsedFrame> {
    let (contents, delays): (Vec<String>, Vec<u64>) = match &askey.fr {
        Frames::Simple(list) => {
            let d = askey.d.unwrap_or(100);
            (list.clone(), vec![d; list.len()])
        }
        Frames::Detailed(list) => {
            let c = list.iter().map(|f| f.c.clone()).collect();
            let d = list.iter().map(|f| f.d).collect();
            (c, d)
        }
    };

    let is_v21 = askey.v == "2.1.0" || askey.v == "1.1" || (!contents.is_empty() && !contents[0].contains("<s c="));
    let re = if is_v21 {
        Regex::new(r#"(?s)\{([^:]+):(.*?)\}"#).unwrap()
    } else {
        Regex::new(r#"(?s)<s c="([^"]+)">(.*?)</s>"#).unwrap()
    };

    contents
        .iter()
        .zip(delays)
        .map(|(content, delay)| parse_single_frame(content, &askey.p, delay, &re))
        .collect()
}

pub fn parse_first_frame(askey: &AskeyFile) -> Option<ParsedFrame> {
    let (content, delay) = match &askey.fr {
        Frames::Simple(list) => {
            if list.is_empty() { return None; }
            (list[0].clone(), askey.d.unwrap_or(100))
        }
        Frames::Detailed(list) => {
            if list.is_empty() { return None; }
            (list[0].c.clone(), list[0].d)
        }
    };

    let is_v21 = askey.v == "2.1.0" || askey.v == "1.1" || !content.contains("<s c=");
    let re = if is_v21 {
        Regex::new(r#"(?s)\{([^:]+):(.*?)\}"#).unwrap()
    } else {
        Regex::new(r#"(?s)<s c="([^"]+)">(.*?)</s>"#).unwrap()
    };

    Some(parse_single_frame(&content, &askey.p, delay, &re))
}
