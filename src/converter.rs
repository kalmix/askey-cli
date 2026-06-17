use crate::models::{AskeyFile, CharsetPreset, FrameObj, Frames, Metadata, QuantizationLevel};
use anyhow::{Context, Result};
use image::codecs::gif::GifDecoder;
use image::AnimationDecoder;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

pub fn convert_image_to_askey(
    input_path: &Path,
    width: u32,
    preset: CharsetPreset,
    scale: f32,
    quantize_step: QuantizationLevel,
) -> Result<AskeyFile> {
    let file = File::open(input_path).context("Failed to open input file")?;
    let is_gif = input_path
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("gif"))
        .unwrap_or(false);

    let target_width = width.max(8);
    let scale = scale.max(0.01);
    let quantize_u8 = quantize_step.to_u8();

    if is_gif {
        if let Ok(decoder) = GifDecoder::new(file) {
            if let Ok(gif_frames) = decoder.into_frames().collect_frames() {
                if !gif_frames.is_empty() {
                    let first_frame = &gif_frames[0];
                    let orig_w = first_frame.buffer().width();
                    let orig_h = first_frame.buffer().height();

                    let target_height =
                        ((orig_h as f32 * (target_width as f32 / orig_w as f32) * scale).round()
                            as u32)
                            .max(1);

                    let mut palette = HashMap::new();
                    let mut color_to_key = HashMap::new();
                    let mut next_color_id = 0;

                    let mut frame_objs = Vec::new();

                    let (num, denom) = gif_frames[0].delay().numer_denom_ms();
                    let default_delay = if denom > 0 {
                        (num as f64 / denom as f64) as u64
                    } else {
                        100
                    };

                    for frame in gif_frames {
                        let (f_num, f_denom) = frame.delay().numer_denom_ms();
                        let f_delay = if f_denom > 0 {
                            (f_num as f64 / f_denom as f64) as u64
                        } else {
                            default_delay
                        };

                        let resized = image::imageops::resize(
                            frame.buffer(),
                            target_width,
                            target_height,
                            image::imageops::FilterType::Triangle,
                        );

                        let frame_str = convert_buffer_to_ascii(
                            &resized,
                            preset.clone(),
                            &mut palette,
                            &mut color_to_key,
                            &mut next_color_id,
                            quantize_u8,
                        );
                        frame_objs.push(FrameObj {
                            c: frame_str,
                            d: f_delay,
                        });
                    }

                    let num_frames = frame_objs.len();
                    return Ok(AskeyFile {
                        v: "2.1.0".to_string(),
                        n: Some(
                            input_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                        ),
                        m: Metadata {
                            f: num_frames,
                            d: default_delay,
                            t: "detailed".to_string(),
                        },
                        p: palette,
                        d: Some(default_delay),
                        fr: Frames::Detailed(frame_objs),
                    });
                }
            }
        }
    }

    let img = image::open(input_path).context("Failed to open static image or GIF")?;
    let orig_w = img.width();
    let orig_h = img.height();

    let target_height =
        ((orig_h as f32 * (target_width as f32 / orig_w as f32) * scale).round() as u32).max(1);

    let mut palette = HashMap::new();
    let mut color_to_key = HashMap::new();
    let mut next_color_id = 0;

    let resized = image::imageops::resize(
        &img.to_rgba8(),
        target_width,
        target_height,
        image::imageops::FilterType::Triangle,
    );

    let frame_str = convert_buffer_to_ascii(
        &resized,
        preset,
        &mut palette,
        &mut color_to_key,
        &mut next_color_id,
        quantize_u8,
    );

    Ok(AskeyFile {
        v: "2.1.0".to_string(),
        n: Some(
            input_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
        m: Metadata {
            f: 1,
            d: 100,
            t: "simple".to_string(),
        },
        p: palette,
        d: Some(100),
        fr: Frames::Simple(vec![frame_str]),
    })
}

fn convert_buffer_to_ascii(
    buffer: &image::RgbaImage,
    preset: CharsetPreset,
    palette: &mut HashMap<String, String>,
    color_to_key: &mut HashMap<String, String>,
    next_color_id: &mut u32,
    quantize_step: u8,
) -> String {
    let chars: Vec<char> = match preset {
        CharsetPreset::Blocks => vec![' ', '░', '▒', '▓', '█'],
        CharsetPreset::Standard => vec![' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'],
        CharsetPreset::Detailed => {
            " .'`^\",:;Il!i><~+_-?][}{1)(|\\/tfjrxnuvczXYUJCLQ0OZmwqpdbkhaogpyLDBWM#*&8%@$"
                .chars()
                .collect()
        }
        CharsetPreset::Custom(ref s) => s.chars().collect(),
    };

    let width = buffer.width();
    let height = buffer.height();
    let mut result = String::new();
    let quantize_step = quantize_step.max(1);

    for y in 0..height {
        let mut current_color: Option<String> = None;
        let mut span_accumulator = String::new();

        for x in 0..width {
            let pixel = buffer.get_pixel(x, y);
            let r = pixel[0];
            let g = pixel[1];
            let b = pixel[2];
            let a = pixel[3];

            if a < 64 {
                if let Some(ref color_key) = current_color {
                    result.push_str(&format!("{{{}:{}}}", color_key, span_accumulator));
                    span_accumulator.clear();
                    current_color = None;
                }
                result.push(' ');
            } else {
                let lum = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
                let char_idx = ((lum / 255.0) * (chars.len() - 1) as f32).round() as usize;
                let ch = chars[char_idx.min(chars.len() - 1)];

                if ch == ' ' {
                    if let Some(ref color_key) = current_color {
                        result.push_str(&format!("{{{}:{}}}", color_key, span_accumulator));
                        span_accumulator.clear();
                        current_color = None;
                    }
                    result.push(' ');
                } else {
                    let q_step = quantize_step as u16;
                    let half = q_step / 2;
                    let qr = (((r as u16 + half) / q_step) * q_step).min(255) as u8;
                    let qg = (((g as u16 + half) / q_step) * q_step).min(255) as u8;
                    let qb = (((b as u16 + half) / q_step) * q_step).min(255) as u8;
                    let hex = format!("#{:02x}{:02x}{:02x}", qr, qg, qb);

                    let color_key = color_to_key
                        .entry(hex.clone())
                        .or_insert_with(|| {
                            let key = format!("{}", next_color_id);
                            *next_color_id += 1;
                            palette.insert(key.clone(), hex);
                            key
                        })
                        .clone();

                    if let Some(ref current_key) = current_color {
                        if current_key == &color_key {
                            span_accumulator.push(ch);
                        } else {
                            result.push_str(&format!("{{{}:{}}}", current_key, span_accumulator));
                            span_accumulator.clear();
                            span_accumulator.push(ch);
                            current_color = Some(color_key);
                        }
                    } else {
                        span_accumulator.push(ch);
                        current_color = Some(color_key);
                    }
                }
            }
        }

        if let Some(ref color_key) = current_color {
            result.push_str(&format!("{{{}:{}}}", color_key, span_accumulator));
        }

        if y < height - 1 {
            result.push('\n');
        }
    }

    result
}

pub fn load_preview_frames(input_path: &Path) -> Result<Vec<(image::RgbaImage, u64)>> {
    let file = File::open(input_path).context("Failed to open input file")?;
    let is_gif = input_path
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("gif"))
        .unwrap_or(false);

    if is_gif {
        if let Ok(decoder) = GifDecoder::new(file) {
            if let Ok(gif_frames) = decoder.into_frames().collect_frames() {
                if !gif_frames.is_empty() {
                    let mut result = Vec::new();
                    let (num, denom) = gif_frames[0].delay().numer_denom_ms();
                    let default_delay = if denom > 0 {
                        (num as f64 / denom as f64) as u64
                    } else {
                        100
                    };

                    for frame in gif_frames {
                        let (f_num, f_denom) = frame.delay().numer_denom_ms();
                        let f_delay = if f_denom > 0 {
                            (f_num as f64 / f_denom as f64) as u64
                        } else {
                            default_delay
                        };
                        result.push((frame.buffer().clone(), f_delay));
                    }
                    return Ok(result);
                }
            }
        }
    }

    let img = image::open(input_path).context("Failed to open static image or GIF")?;
    Ok(vec![(img.into_rgba8(), 100)])
}

pub fn generate_preview_ansi(
    buffer: &image::RgbaImage,
    width: u32,
    preset: CharsetPreset,
    scale: f32,
    quantize_step: QuantizationLevel,
) -> String {
    let target_width = width.max(8);
    let scale = scale.max(0.01);
    let quantize_u8 = quantize_step.to_u8();

    let orig_w = buffer.width();
    let orig_h = buffer.height();
    let target_height =
        ((orig_h as f32 * (target_width as f32 / orig_w as f32) * scale).round() as u32).max(1);

    let mut palette = HashMap::new();
    let mut color_to_key = HashMap::new();
    let mut next_color_id = 0;

    let resized = image::imageops::resize(
        buffer,
        target_width,
        target_height,
        image::imageops::FilterType::Triangle,
    );

    let ascii_str = convert_buffer_to_ascii(
        &resized,
        preset,
        &mut palette,
        &mut color_to_key,
        &mut next_color_id,
        quantize_u8,
    );

    let dummy_askey = AskeyFile {
        v: "2.1.0".to_string(),
        n: None,
        m: Metadata {
            f: 1,
            d: 100,
            t: "simple".to_string(),
        },
        p: palette,
        d: Some(100),
        fr: Frames::Simple(vec![ascii_str]),
    };

    let parsed = crate::parser::parse_frames(&dummy_askey);
    if !parsed.is_empty() {
        parsed[0].ansi_content.clone()
    } else {
        String::new()
    }
}
