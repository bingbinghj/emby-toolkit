use crate::{
    error::{ApiError, Result},
    state::string,
};
use ab_glyph::{Font, FontArc, ScaleFont};
use image::{
    DynamicImage, ImageFormat, Rgba, RgbaImage,
    imageops::{self, FilterType},
};
use imageproc::{
    drawing::{draw_text_mut, text_size},
    geometric_transformations::{Interpolation, rotate_about_center},
    rect::Rect,
};
use serde_json::Value;
use std::io::Cursor;

fn ratio(config: &Value, key: &str, default: f32) -> f32 {
    config[key]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n as f32)
        .unwrap_or(default)
}
fn fit(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    DynamicImage::ImageRgba8(image.clone())
        .resize_to_fill(width, height, FilterType::Lanczos3)
        .to_rgba8()
}
fn hsv(color: [u8; 3]) -> [f64; 3] {
    let [r, g, b] = color.map(|c| c as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let hue = if d == 0.0 {
        0.0
    } else if max == r {
        (g - b) / d
    } else if max == g {
        2.0 + (b - r) / d
    } else {
        4.0 + (r - g) / d
    };
    [
        (hue / 6.0).rem_euclid(1.0),
        if max == 0.0 { 0.0 } else { d / max },
        max,
    ]
}
fn rgb([h, s, v]: [f64; 3]) -> [u8; 3] {
    let i = (h * 6.0).floor() as usize;
    let f = h * 6.0 - i as f64;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let color = match i % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    };
    color.map(|c| (c * 255.0) as u8)
}
fn distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    let [h1, s1, v1] = hsv(a);
    let [h2, s2, v2] = hsv(b);
    let d = (h1 - h2).abs();
    d.min(1.0 - d) * 5.0 + (s1 - s2).abs() + (v1 - v2).abs()
}
fn common_colors(image: &RgbaImage, size: u32, filter: impl Fn(&Rgba<u8>) -> bool) -> Vec<[u8; 3]> {
    let sample = if image.width() <= size && image.height() <= size {
        image.clone()
    } else {
        DynamicImage::ImageRgba8(image.clone())
            .resize(size, size, FilterType::CatmullRom)
            .to_rgba8()
    };
    let mut positions = std::collections::HashMap::new();
    let mut colors = Vec::<([u8; 3], usize)>::new();
    for pixel in sample.pixels().filter(|p| filter(p)) {
        let c = [pixel[0], pixel[1], pixel[2]];
        let index = *positions.entry(c).or_insert_with(|| {
            colors.push((c, 0));
            colors.len() - 1
        });
        colors[index].1 += 1;
    }
    colors.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    colors.into_iter().map(|(c, _)| c).collect()
}
fn colorful(p: &Rgba<u8>) -> bool {
    let [r, g, b, _] = p.0;
    !((r < 20 && g < 20 && b < 20)
        || (r > 235 && g > 235 && b > 235)
        || ((r as i32 - g as i32).abs() < 10
            && (g as i32 - b as i32).abs() < 10
            && (r as i32 - b as i32).abs() < 10))
}
pub fn palette(image: &RgbaImage, count: usize, vibrant: bool) -> Vec<[u8; 3]> {
    let mut selected = vec![];
    let mut seen_hues = Vec::<i32>::new();
    for color in common_colors(image, if vibrant { 100 } else { 150 }, colorful)
        .into_iter()
        .take(count * if vibrant { 3 } else { 5 })
    {
        let [h, s, v] = hsv(color);
        let adjusted = rgb([
            h,
            s.clamp(if vibrant { 0.2 } else { 0.3 }, 0.7),
            v.clamp(if vibrant { 0.55 } else { 0.6 }, 0.85),
        ]);
        let duplicate = if vibrant {
            seen_hues
                .iter()
                .any(|&seen| ((h * 360.0) as i32 - seen).abs() < 15)
                || selected.contains(&adjusted)
        } else {
            selected.iter().any(|&c| distance(adjusted, c) < 0.15)
        };
        if !duplicate {
            selected.push(adjusted);
            seen_hues.push((h * 360.0) as i32);
            if selected.len() >= count {
                break;
            }
        }
    }
    selected
}
const SOFT: [[u8; 3]; 6] = [
    [237, 159, 77],
    [255, 183, 197],
    [186, 225, 255],
    [255, 223, 186],
    [202, 231, 200],
    [245, 203, 255],
];
fn colors(image: &RgbaImage, single: bool) -> Vec<[u8; 3]> {
    use rand::seq::SliceRandom;
    let mut colors = palette(image, if single { 6 } else { 5 }, !single);
    let mut rng = rand::thread_rng();
    if !single {
        return vec![
            colors
                .first()
                .copied()
                .unwrap_or_else(|| *SOFT.choose(&mut rng).unwrap()),
        ];
    }
    colors.shuffle(&mut rng);
    let soft = [SOFT[0], SOFT[2], SOFT[3], SOFT[4]];
    while colors.len() < 6 {
        let color = if colors.is_empty() {
            *soft.choose(&mut rng).unwrap()
        } else {
            soft.iter()
                .copied()
                .enumerate()
                .max_by(|(ai, a), (bi, b)| {
                    let score = |c| {
                        colors
                            .iter()
                            .map(|&e| distance(c, e))
                            .fold(f64::INFINITY, f64::min)
                    };
                    score(*a).total_cmp(&score(*b)).then_with(|| bi.cmp(ai))
                })
                .unwrap()
                .1
        };
        colors.push(color);
    }
    colors
}
fn grain(image: &mut RgbaImage, intensity: f64, alpha: bool) {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut spare = None;
    for p in image.pixels_mut() {
        for channel in 0..if alpha { 4 } else { 3 } {
            let noise = spare.take().unwrap_or_else(|| {
                let radius = (-2.0 * rng.gen_range(f64::MIN_POSITIVE..1.0).ln()).sqrt();
                let angle = rng.gen_range(0.0..std::f64::consts::TAU);
                spare = Some(radius * angle.sin());
                radius * angle.cos()
            });
            p[channel] = (p[channel] as f64 + noise * intensity * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}
fn gradient(image: &RgbaImage) -> RgbaImage {
    use rand::Rng;
    let sample = imageops::resize(image, 100, 150, FilterType::Lanczos3);
    let mut colors = common_colors(&sample, 150, |p| {
        p[3] > 200
            && !(p[0] < 30 && p[1] < 30 && p[2] < 30)
            && !(p[0] > 220 && p[1] > 220 && p[2] > 220)
    });
    if colors.is_empty() {
        colors = common_colors(&sample, 150, |p| p[3] > 100);
    }
    let color = colors
        .into_iter()
        .take(10)
        .find(|c| {
            let max = *c.iter().max().unwrap() as f64 / 255.0;
            let min = *c.iter().min().unwrap() as f64 / 255.0;
            (0.3..=0.7).contains(&((max + min) / 2.0))
        })
        .unwrap_or_else(|| {
            let mut rng = rand::thread_rng();
            let h = rng.gen_range(0.0..1.0);
            let l: f64 = rng.gen_range(0.5..0.8);
            let s = rng.gen_range(0.5..1.0);
            let v = l + s * l.min(1.0 - l);
            rgb([h, 2.0 * (1.0 - l / v), v])
        });
    let left = color.map(|c| (c as f64 * 0.65) as u8);
    let right = left.map(|c| (c as f64 * 1.9).min(255.0) as u8);
    RgbaImage::from_fn(1920, 1080, |x, _| {
        let mask = (255.0 * (x as f64 / 1920.0).powf(0.7)) as u32;
        let mut p = [0, 0, 0, 255];
        for c in 0..3 {
            p[c] = ((left[c] as u32 * (255 - mask) + right[c] as u32 * mask + 127) / 255) as u8;
        }
        Rgba(p)
    })
}
fn right_image(image: &RgbaImage) -> RgbaImage {
    let width = (image.width() as f64 * 1080.0 / image.height() as f64) as u32;
    let mut canvas = RgbaImage::from_pixel(1920, 1080, Rgba([0, 0, 0, 255]));
    if width < 1296 {
        imageops::overlay(&mut canvas, &fit(image, 1296, 1080), 624, 0);
    } else {
        let image = imageops::resize(image, width, 1080, FilterType::Lanczos3);
        let left = (width - 1296) / 2;
        let crop = imageops::crop_imm(&image, left, 0, 1296, 1080).to_image();
        imageops::overlay(&mut canvas, &crop, 768, 0);
    }
    canvas
}
fn blend(image: &RgbaImage, color: [u8; 3], mix: f32, blur: f32) -> RgbaImage {
    let mut image = imageops::fast_blur(image, blur.max(0.0));
    for p in image.pixels_mut() {
        for (channel, c) in color.iter().enumerate() {
            p[channel] = (p[channel] as f32 * (1.0 - mix) + *c as f32 * mix) as u8;
        }
        p[3] = 255;
    }
    image
}
fn rounded(image: &mut RgbaImage, radius: f32) {
    let width = image.width() as f32;
    let height = image.height() as f32;
    for (x, y, p) in image.enumerate_pixels_mut() {
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        let cx = px.clamp(radius, width - radius);
        let cy = py.clamp(radius, height - radius);
        let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        if distance > radius {
            p[3] = 0;
        } else if distance > radius - 1.0 {
            p[3] = (p[3] as f32 * (radius - distance)) as u8;
        }
    }
}
fn rotate(image: &RgbaImage, angle: f32) -> RgbaImage {
    imageproc::geometric_transformations::rotate_about_center_no_crop(
        image,
        -angle.to_radians(),
        Interpolation::Bicubic,
        imageproc::geometric_transformations::Border::Constant(Rgba([0, 0, 0, 0])),
    )
}
fn paste_masked(canvas: &mut RgbaImage, image: &RgbaImage, x: i64, y: i64) {
    for (ix, iy, source) in image.enumerate_pixels() {
        let (tx, ty) = (x + ix as i64, y + iy as i64);
        if tx < 0 || ty < 0 || tx >= canvas.width() as i64 || ty >= canvas.height() as i64 {
            continue;
        }
        let destination = canvas.get_pixel_mut(tx as u32, ty as u32);
        let mask = source[3] as u32;
        for c in 0..4 {
            destination[c] =
                ((source[c] as u32 * mask + destination[c] as u32 * (255 - mask) + 127) / 255)
                    as u8;
        }
    }
}
fn card(
    canvas: &mut RgbaImage,
    image: &RgbaImage,
    angle: f32,
    center: (i64, i64),
    blur: f32,
    opacity: u8,
    offset: (i64, i64),
) {
    let padding = (blur as u32 * 4).max(100);
    let mut shadow = RgbaImage::new(image.width() + padding * 2, image.height() + padding * 2);
    for (x, y, p) in image.enumerate_pixels() {
        shadow.put_pixel(
            x + padding,
            y + padding,
            Rgba([0, 0, 0, (p[3] as u32 * opacity as u32 / 255) as u8]),
        );
    }
    let shadow = rotate(&imageops::fast_blur(&shadow, blur), angle);
    paste_masked(
        canvas,
        &shadow,
        center.0 - shadow.width() as i64 / 2 + offset.0,
        center.1 - shadow.height() as i64 / 2 + offset.1,
    );
    let image = rotate(image, angle);
    paste_masked(
        canvas,
        &image,
        center.0 - image.width() as i64 / 2,
        center.1 - image.height() as i64 / 2,
    );
}
fn poster_shadow(image: &RgbaImage) -> RgbaImage {
    let mut shadow = RgbaImage::new(image.width() + 60, image.height() + 60);
    let background = RgbaImage::from_pixel(image.width(), image.height(), Rgba([0, 0, 0, 216]));
    imageops::overlay(&mut shadow, &background, 40, 40);
    let mut shadow = imageops::fast_blur(&shadow, 20.0);
    let mut result = RgbaImage::new(shadow.width(), shadow.height());
    paste_masked(&mut result, image, 20, 20);
    imageops::overlay(&mut shadow, &result, 0, 0);
    shadow
}
fn bounds(font: &FontArc, size: f32, text: &str) -> (u32, u32, f32) {
    let scaled = font.as_scaled(size);
    let mut position = 0.0f32;
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for c in text.chars() {
        let id = font.glyph_id(c);
        let glyph = id.with_scale_and_position(size, ab_glyph::point(position, scaled.ascent()));
        if let Some(outline) = font.outline_glyph(glyph) {
            let b = outline.px_bounds();
            min = min.min(b.min.y);
            max = max.max(b.max.y);
        }
        position += scaled.h_advance(id);
    }
    (
        position.ceil() as u32,
        if min.is_finite() {
            (max - min).ceil() as u32
        } else {
            0
        },
        if min.is_finite() { min } else { 0.0 },
    )
}
fn text(
    canvas: &mut RgbaImage,
    title: &[String; 2],
    fonts: &[FontArc; 2],
    config: &Value,
    multi: bool,
    color: [u8; 3],
    block: [u8; 3],
) {
    let [zh, en] = title;
    let mut shadow = RgbaImage::new(1920, 1080);
    let mut layer = RgbaImage::new(1920, 1080);
    let white = Rgba([
        255,
        255,
        255,
        if multi && config["multi_1_blur"] != true {
            255
        } else {
            229
        },
    ]);
    let dark = Rgba([
        (color[0] as f32 * 0.8) as u8,
        (color[1] as f32 * 0.8) as u8,
        (color[2] as f32 * 0.8) as u8,
        75,
    ]);
    if multi {
        let zh_size =
            163.0 * ratio(config, "zh_font_size_multi_1", 1.0) * fonts[0].height_unscaled()
                / fonts[0].units_per_em().unwrap_or(1000.0);
        draw_text_mut(&mut layer, white, 73, 427, zh_size, &fonts[0], zh);
        if config["multi_1_blur"] == true {
            for offset in (3..=10).step_by(2) {
                draw_text_mut(
                    &mut shadow,
                    dark,
                    73 + offset,
                    427 + offset,
                    zh_size,
                    &fonts[0],
                    zh,
                );
            }
        }
        if !en.is_empty() {
            let words = en.split_whitespace().collect::<Vec<_>>();
            let longest = words.iter().map(|s| s.len()).max().unwrap_or(0);
            let base = 50.0 * ratio(config, "en_font_size_multi_1", 1.0);
            let size = if longest > 10 || words.len() > 3 {
                (base * (10.0 / (longest.max(words.len() * 3)) as f32).powf(0.8)).max(30.0)
            } else {
                base
            };
            let pixels = size as u32 as f32;
            let size =
                pixels * fonts[1].height_unscaled() / fonts[1].units_per_em().unwrap_or(1000.0);
            for (index, line) in words.iter().enumerate() {
                let y = 624 + (index as f32 * (pixels + base * 0.1)) as i32;
                draw_text_mut(&mut layer, white, 124, y, size, &fonts[1], line);
                if config["multi_1_blur"] == true {
                    for offset in 2..=4 {
                        draw_text_mut(
                            &mut shadow,
                            dark,
                            124 + offset,
                            y + offset,
                            size,
                            &fonts[1],
                            line,
                        );
                    }
                }
            }
            imageproc::drawing::draw_filled_rect_mut(
                &mut layer,
                Rect::at(84, 620).of_size(
                    22,
                    (base * 1.1 + (words.len().saturating_sub(1) as f32) * (pixels + base * 0.1))
                        as u32,
                ),
                Rgba([block[0], block[1], block[2], 255]),
            );
        }
    } else {
        let zh_pixel = (183.6 * ratio(config, "zh_font_size", 1.0)) as u32 as f32;
        let en_pixel = (75.6 * ratio(config, "en_font_size", 1.0)) as u32 as f32;
        let zh_size =
            zh_pixel * fonts[0].height_unscaled() / fonts[0].units_per_em().unwrap_or(1000.0);
        let en_size =
            en_pixel * fonts[1].height_unscaled() / fonts[1].units_per_em().unwrap_or(1000.0);
        let (width, height, _) = bounds(&fonts[0], zh_size, zh);
        let x = 480 - width as i32 / 2;
        let y = 540 - height as i32 - en_pixel as i32 / 2 - 5;
        for offset in (3..=12).step_by(2) {
            draw_text_mut(
                &mut shadow,
                dark,
                x + offset,
                y + offset,
                zh_size,
                &fonts[0],
                zh,
            );
        }
        draw_text_mut(&mut layer, white, x, y, zh_size, &fonts[0], zh);
        if !en.is_empty() {
            let (width, _) = text_size(en_size, &fonts[1], en);
            let x = 480 - width as i32 / 2;
            let y = y + height as i32 + en_pixel as i32;
            for offset in 2..=6 {
                draw_text_mut(
                    &mut shadow,
                    dark,
                    x + offset,
                    y + offset,
                    en_size,
                    &fonts[1],
                    en,
                );
            }
            draw_text_mut(&mut layer, white, x, y, en_size, &fonts[1], en);
        }
    }
    let shadow = imageops::fast_blur(&shadow, if multi { 10.0 } else { 12.0 });
    imageops::overlay(canvas, &shadow, 0, 0);
    imageops::overlay(canvas, &layer, 0, 0);
}
pub fn badge(image: &mut RgbaImage, count: i64, font: &FontArc, style: &str, size: f32) {
    badge_with_color(image, count, font, style, size, None);
}
fn badge_with_color(
    image: &mut RgbaImage,
    count: i64,
    font: &FontArc,
    style: &str,
    size: f32,
    color: Option<[u8; 3]>,
) {
    if count == 0 {
        return;
    }
    let pixel_size = (image.height() as f32 * size) as u32 as f32;
    let font_size = pixel_size * font.height_unscaled() / font.units_per_em().unwrap_or(1000.0);
    let margin = (image.height() as f32 * 0.04) as i32;
    let text = count.to_string();
    let (width, height, offset) = bounds(font, font_size, &text);
    let mut layer = RgbaImage::new(image.width(), image.height());
    if style == "ribbon" {
        let size = (pixel_size * 3.0) as u32;
        for y in 0..size.min(image.height()) {
            for x in 0..size.min(image.width()) {
                if x + y <= size && x + y > size / 2 {
                    layer.put_pixel(x, y, Rgba([250, 222, 135, 250]));
                }
            }
        }
        let text_size = ((width as f32).hypot(height as f32) * 1.5) as u32;
        let mut canvas = RgbaImage::new(text_size.max(1), text_size.max(1));
        let x = ((text_size - width) / 2) as i32;
        let y = ((text_size - height) / 2) as i32 - offset as i32;
        draw_text_mut(
            &mut canvas,
            Rgba([0, 0, 0, 80]),
            x + 2,
            y + 2,
            font_size,
            font,
            &text,
        );
        draw_text_mut(
            &mut canvas,
            Rgba([89, 52, 2, 245]),
            x,
            y,
            font_size,
            font,
            &text,
        );
        let rotated = rotate_about_center(
            &canvas,
            -std::f32::consts::FRAC_PI_4,
            Interpolation::Bicubic,
            imageproc::geometric_transformations::Border::Constant(Rgba([0, 0, 0, 0])),
        );
        imageops::overlay(
            &mut layer,
            &rotated,
            (size as f32 * 0.38) as i64 - text_size as i64 / 2,
            (size as f32 * 0.38) as i64 - text_size as i64 / 2,
        );
    } else if style == "badge" {
        let w = width + (pixel_size * 0.4) as u32 * 2;
        let h = height + (pixel_size * 0.2) as u32 * 2;
        let fill = color.map_or(Rgba([40, 40, 40, 180]), |c| {
            Rgba([
                (c[0] as f32 * 0.3) as u8,
                (c[1] as f32 * 0.3) as u8,
                (c[2] as f32 * 0.3) as u8,
                190,
            ])
        });
        let mut background = RgbaImage::from_pixel(w.max(1), h.max(1), fill);
        rounded(&mut background, h as f32 * 0.3);
        imageops::overlay(&mut layer, &background, margin as i64, margin as i64);
        let x = margin + (w - width) as i32 / 2;
        let y = margin + (h - height) as i32 / 2 - offset as i32;
        draw_text_mut(
            &mut layer,
            Rgba([0, 0, 0, 100]),
            x + 2,
            y + 2,
            font_size,
            font,
            &text,
        );
        draw_text_mut(
            &mut layer,
            Rgba([255, 255, 255, 240]),
            x,
            y,
            font_size,
            font,
            &text,
        );
    }
    imageops::overlay(image, &layer, 0, 0);
}
pub fn render(
    images: &[RgbaImage],
    title: &[String; 2],
    fonts: &[FontArc; 2],
    config: &Value,
    count: i64,
) -> Result<Vec<u8>> {
    let first = images
        .first()
        .ok_or_else(|| ApiError::bad("缺少封面图片"))?;
    let style = string(config, "cover_style");
    let multi = style == "multi_1";
    let foreground = (style == "single_2").then(|| right_image(first));
    let palette = colors(foreground.as_ref().unwrap_or(first), style == "single_1");
    let color = palette[0];
    let dark = color.map(|c| (c as f32 * 0.85) as u8);
    let blur = config[if multi {
        "blur_size_multi_1"
    } else {
        "blur_size"
    }]
    .as_f64()
    .filter(|n| n.is_finite() && *n >= 0.0)
    .unwrap_or(50.0) as u32 as f32;
    let mix = config[if multi {
        "color_ratio_multi_1"
    } else {
        "color_ratio"
    }]
    .as_f64()
    .filter(|n| (0.0..=1.0).contains(n))
    .unwrap_or(0.8) as f32;
    let mut canvas = blend(&fit(first, 1920, 1080), dark, mix, blur);
    if !multi {
        grain(
            &mut canvas,
            if style == "single_1" { 0.03 } else { 0.05 },
            false,
        );
    }
    match style.as_str() {
        "single_1" => {
            let main = fit(first, 756, 756);
            for (angle, opacity, offset, mix, blur, shadow_blur, color) in [
                (36.0, 102, (10, 16), 0.6, 16.0, 12.0, palette[2]),
                (18.0, 128, (15, 22), 0.5, 8.0, 15.0, palette[1]),
                (0.0, 153, (20, 26), 0.0, 0.0, 18.0, color),
            ] {
                let mut image = blend(&main, color, mix, blur);
                rounded(&mut image, 94.0);
                card(
                    &mut canvas,
                    &image,
                    angle,
                    (1380, 540),
                    shadow_blur,
                    opacity,
                    offset,
                );
            }
        }
        "single_2" => {
            let image = right_image(first);
            let mut mask = image::GrayImage::new(1920, 1080);
            for y in 0..1080 {
                let split = (1920.0 * (0.55 - 0.15 * y as f32 / 1080.0)) as u32;
                for x in split - 5..split + 5 {
                    mask.put_pixel(x, y, image::Luma([255]));
                }
            }
            let mask = imageops::fast_blur(&mask, 10.0);
            for y in 0..1080 {
                let split = (1920.0 * (0.55 - (0.15 * y as f32 / 1080.0))) as u32;
                for x in split..1920 {
                    let mut p = *image.get_pixel(x, y);
                    let alpha = mask.get_pixel(x, y)[0] as u32;
                    for c in 0..3 {
                        p[c] = ((p[c] as u32 * (255 - alpha)
                            + (color[c] as f32 * 0.5) as u32 * alpha
                            + 127)
                            / 255) as u8;
                    }
                    canvas.put_pixel(x, y, p);
                }
            }
        }
        "multi_1" => {
            if config["multi_1_blur"] != true {
                canvas = gradient(first);
            } else {
                for (x, _, p) in canvas.enumerate_pixels_mut() {
                    let alpha = (153.0 * x as f32 / 1920.0) as u32;
                    for c in 0..3 {
                        p[c] = ((p[c] as u32 * (255 - alpha) + 255 * alpha + 127) / 255) as u8;
                    }
                }
                grain(&mut canvas, 0.03, true);
            }
            let order = [2, 0, 4, 3, 1, 5, 8, 7, 6];
            for column in 0..3 {
                let mut strip = RgbaImage::new(450, 1914);
                for row in 0..3 {
                    let mut poster = fit(&images[order[column * 3 + row] % images.len()], 410, 610);
                    rounded(&mut poster, 46.1);
                    let poster = poster_shadow(&poster);
                    paste_masked(&mut strip, &poster, 0, (row * 632) as i64);
                }
                let center = (
                    1050 + column as i64 * 100
                        + if column == 1 {
                            360
                        } else if column == 2 {
                            780
                        } else {
                            0
                        },
                    -362 + 937 + if column == 2 { -155 } else { 0 },
                );
                let mut layer = RgbaImage::new(strip.width(), strip.height());
                paste_masked(&mut layer, &strip, 0, 0);
                use imageproc::geometric_transformations::{Border, Projection, warp_into};
                let projection = Projection::translate(center.0 as f32, center.1 as f32)
                    * Projection::rotate(15.8f32.to_radians())
                    * Projection::translate(
                        -(layer.width() as f32) / 2.0,
                        -(layer.height() as f32) / 2.0,
                    );
                let mut rotated = RgbaImage::new(1920, 1080);
                warp_into(
                    &layer,
                    projection,
                    Interpolation::Bicubic,
                    Border::Constant(Rgba([0, 0, 0, 0])),
                    &mut rotated,
                );
                paste_masked(&mut canvas, &rotated, 0, 0);
            }
        }
        _ => return Err(ApiError::bad("不支持的封面样式")),
    }
    use rand::Rng;
    let block_image = &images[2 % images.len()];
    let mut rng = rand::thread_rng();
    let pixel = block_image.get_pixel(
        rng.gen_range(
            block_image.width() / 2..=(block_image.width() * 4 / 5).min(block_image.width() - 1),
        ),
        rng.gen_range(
            block_image.height() / 2..=(block_image.height() * 4 / 5).min(block_image.height() - 1),
        ),
    );
    text(
        &mut canvas,
        title,
        fonts,
        config,
        multi,
        if multi { color } else { dark },
        [pixel[0], pixel[1], pixel[2]],
    );
    if config["show_item_count"] == true {
        badge_with_color(
            &mut canvas,
            count,
            &fonts[0],
            config["badge_style"].as_str().unwrap_or("badge"),
            ratio(config, "badge_size_ratio", 0.12),
            Some(color),
        );
    }
    let mut bytes = Cursor::new(vec![]);
    DynamicImage::ImageRgba8(canvas)
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|_| ApiError::bad("封面PNG编码失败"))?;
    Ok(bytes.into_inner())
}
