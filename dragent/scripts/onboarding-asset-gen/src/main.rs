//! Generate onboarding preview PNGs for the features and theme slides.

use std::path::{Path, PathBuf};

use image::{Rgba, RgbaImage};

const W: u32 = 960;
const H: u32 = 640;

#[derive(Clone, Copy)]
struct Palette {
    id: &'static str,
    background: Rgba<u8>,
    foreground: Rgba<u8>,
    accent: Rgba<u8>,
    sidebar: Rgba<u8>,
    surface: Rgba<u8>,
    border: Rgba<u8>,
    green: Rgba<u8>,
    red: Rgba<u8>,
    yellow: Rgba<u8>,
    blue: Rgba<u8>,
}

const PALETTES: &[Palette] = &[
    Palette {
        id: "dark",
        background: Rgba([0, 0, 0, 255]),
        foreground: Rgba([255, 255, 255, 255]),
        accent: Rgba([25, 170, 216, 255]),
        sidebar: Rgba([13, 13, 13, 255]),
        surface: Rgba([20, 20, 20, 255]),
        border: Rgba([46, 46, 46, 255]),
        green: Rgba([180, 250, 114, 255]),
        red: Rgba([255, 130, 114, 255]),
        yellow: Rgba([229, 160, 26, 255]),
        blue: Rgba([165, 213, 254, 255]),
    },
    Palette {
        id: "light",
        background: Rgba([245, 245, 247, 255]),
        foreground: Rgba([28, 28, 30, 255]),
        accent: Rgba([25, 130, 196, 255]),
        sidebar: Rgba([235, 235, 238, 255]),
        surface: Rgba([255, 255, 255, 255]),
        border: Rgba([200, 200, 205, 255]),
        green: Rgba([52, 140, 72, 255]),
        red: Rgba([196, 54, 42, 255]),
        yellow: Rgba([196, 132, 18, 255]),
        blue: Rgba([36, 110, 196, 255]),
    },
    Palette {
        id: "dracula",
        background: Rgba([40, 42, 54, 255]),
        foreground: Rgba([248, 248, 242, 255]),
        accent: Rgba([189, 147, 249, 255]),
        sidebar: Rgba([33, 34, 44, 255]),
        surface: Rgba([48, 50, 62, 255]),
        border: Rgba([68, 71, 90, 255]),
        green: Rgba([80, 250, 123, 255]),
        red: Rgba([255, 85, 85, 255]),
        yellow: Rgba([241, 250, 140, 255]),
        blue: Rgba([139, 233, 253, 255]),
    },
    Palette {
        id: "solarized_dark",
        background: Rgba([0, 43, 54, 255]),
        foreground: Rgba([131, 148, 150, 255]),
        accent: Rgba([38, 139, 210, 255]),
        sidebar: Rgba([0, 36, 46, 255]),
        surface: Rgba([7, 54, 66, 255]),
        border: Rgba([38, 70, 78, 255]),
        green: Rgba([133, 153, 0, 255]),
        red: Rgba([220, 50, 47, 255]),
        yellow: Rgba([181, 137, 0, 255]),
        blue: Rgba([38, 139, 210, 255]),
    },
    Palette {
        id: "monokai",
        background: Rgba([39, 40, 34, 255]),
        foreground: Rgba([248, 248, 242, 255]),
        accent: Rgba([166, 226, 46, 255]),
        sidebar: Rgba([30, 31, 26, 255]),
        surface: Rgba([46, 46, 40, 255]),
        border: Rgba([73, 72, 62, 255]),
        green: Rgba([166, 226, 46, 255]),
        red: Rgba([249, 38, 114, 255]),
        yellow: Rgba([230, 219, 116, 255]),
        blue: Rgba([102, 217, 239, 255]),
    },
    Palette {
        id: "nord",
        background: Rgba([46, 52, 64, 255]),
        foreground: Rgba([216, 222, 233, 255]),
        accent: Rgba([136, 192, 208, 255]),
        sidebar: Rgba([40, 45, 56, 255]),
        surface: Rgba([59, 66, 82, 255]),
        border: Rgba([76, 86, 106, 255]),
        green: Rgba([163, 190, 140, 255]),
        red: Rgba([191, 97, 106, 255]),
        yellow: Rgba([235, 203, 139, 255]),
        blue: Rgba([129, 161, 193, 255]),
    },
];

fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba<u8> {
    Rgba([r, g, b, a])
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../app/assets/onboarding");
    std::fs::create_dir_all(&root).expect("create onboarding dir");

    let dark = PALETTES[0];
    write_png(
        &root.join("feature-workspace.png"),
        render_features_workspace(dark),
    );
    write_png(
        &root.join("theme-workspace.png"),
        render_theme_terminal(dark),
    );

    println!("Wrote onboarding PNGs under {}", root.display());
}

fn write_png(path: &Path, img: RgbaImage) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    img.save(path).unwrap_or_else(|err| panic!("save {}: {err}", path.display()));
}

fn fill(img: &mut RgbaImage, color: Rgba<u8>) {
    for px in img.pixels_mut() {
        *px = color;
    }
}

fn rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: Rgba<u8>) {
    for py in y..y.saturating_add(h).min(H) {
        for px in x..x.saturating_add(w).min(W) {
            img.put_pixel(px, py, color);
        }
    }
}

fn line_h(img: &mut RgbaImage, x: u32, y: u32, w: u32, color: Rgba<u8>) {
    rect(img, x, y, w, 1, color);
}

fn pill(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: Rgba<u8>) {
    rect(img, x, y, w, h, color);
}

fn render_window_chrome(img: &mut RgbaImage, p: Palette) {
    rect(img, 0, 0, W, 38, p.surface);
    line_h(img, 0, 38, W, p.border);
    pill(img, 16, 14, 12, 12, Rgba([255, 95, 86, 255]));
    pill(img, 34, 14, 12, 12, Rgba([255, 189, 46, 255]));
    pill(img, 52, 14, 12, 12, Rgba([39, 201, 63, 255]));
}

fn render_sidebar(img: &mut RgbaImage, p: Palette) {
    rect(img, 0, 39, 220, H - 39, p.sidebar);
    line_h(img, 220, 39, 1, p.border);
    rect(img, 12, 56, 196, 28, p.surface);
    rect(img, 12, 96, 196, 36, blend(p.accent, p.surface, 0.25));
    rect(img, 12, 140, 196, 36, p.surface);
    rect(img, 12, 184, 196, 36, p.surface);
}

fn render_features_workspace(p: Palette) -> RgbaImage {
    let mut img = RgbaImage::new(W, H);
    fill(&mut img, p.background);
    render_window_chrome(&mut img, p);
    render_sidebar(&mut img, p);

    let tx = 236;
    rect(&mut img, tx, 39, W - tx, H - 39, p.background);
    line_h(&mut img, tx, 74, W - tx, p.border);

    pill(&mut img, tx + 16, 90, 8, 18, p.accent);
    rect(&mut img, tx + 32, 92, 220, 10, with_alpha(p.foreground, 180));

    rect(&mut img, tx + 16, 124, 420, 10, with_alpha(p.red, 220));
    rect(&mut img, tx + 16, 146, 280, 8, with_alpha(p.foreground, 100));

    let card_x = tx + 16;
    let card_y = 180;
    let card_w = 520;
    let card_h = 150;
    rect(&mut img, card_x, card_y, card_w, card_h, p.surface);
    rect(&mut img, card_x, card_y, card_w, 1, p.border);
    rect(&mut img, card_x, card_y + card_h - 1, card_w, 1, p.border);
    rect(&mut img, card_x, card_y, 1, card_h, p.border);
    rect(&mut img, card_x + card_w - 1, card_y, 1, card_h, p.border);

    pill(&mut img, card_x + 16, card_y + 16, 24, 24, p.accent);
    rect(&mut img, card_x + 48, card_y + 22, 180, 10, p.accent);
    rect(&mut img, card_x + 16, card_y + 52, 360, 8, with_alpha(p.foreground, 200));
    rect(&mut img, card_x + 16, card_y + 72, 280, 8, with_alpha(p.foreground, 120));

    rect(&mut img, card_x + 16, card_y + 96, card_w - 32, 40, p.background);
    rect(&mut img, card_x + 24, card_y + 108, 12, 8, p.green);
    rect(&mut img, card_x + 42, card_y + 108, 260, 8, with_alpha(p.foreground, 200));
    rect(&mut img, card_x + card_w - 72, card_y + 108, 48, 8, p.accent);

    pill(&mut img, tx + 16, card_y + card_h + 24, 8, 18, p.accent);
    rect(&mut img, tx + 32, card_y + card_h + 28, 4, 12, p.foreground);

    img
}

fn render_theme_terminal(p: Palette) -> RgbaImage {
    let mut img = RgbaImage::new(W, H);
    fill(&mut img, p.background);
    render_window_chrome(&mut img, p);

    rect(&mut img, 0, 39, W, H - 39, p.background);
    let tx = 24;
    let ty = 72;

    pill(&mut img, tx, ty, 8, 18, p.accent);
    rect(&mut img, tx + 16, ty + 4, 120, 10, with_alpha(p.foreground, 220));

    rect(&mut img, tx, ty + 36, 80, 8, p.yellow);
    rect(&mut img, tx + 88, ty + 36, 140, 8, p.blue);
    rect(&mut img, tx, ty + 56, 260, 8, with_alpha(p.foreground, 120));
    rect(&mut img, tx, ty + 76, 72, 8, p.green);
    rect(&mut img, tx + 80, ty + 76, 160, 8, p.green);
    rect(&mut img, tx, ty + 96, 64, 8, p.red);
    rect(&mut img, tx + 72, ty + 96, 120, 8, p.red);

    pill(&mut img, tx, ty + 128, 8, 18, p.accent);
    rect(&mut img, tx + 16, ty + 132, 4, 12, p.foreground);

    img
}

fn with_alpha(color: Rgba<u8>, alpha: u8) -> Rgba<u8> {
    Rgba([color[0], color[1], color[2], alpha])
}

fn blend(a: Rgba<u8>, b: Rgba<u8>, t: f32) -> Rgba<u8> {
    let t = t.clamp(0.0, 1.0);
    Rgba([
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
        255,
    ])
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}
