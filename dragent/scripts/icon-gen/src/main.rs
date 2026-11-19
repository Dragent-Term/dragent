//! App icon helpers: pad artwork on a square canvas and build Windows .ico files.

use std::env;
use std::fs::File;
use std::io;
use std::path::Path;

use ico::{IconDir, IconImage, ResourceType};
use image::imageops::{overlay, FilterType};
use image::{RgbaImage};

const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
const DEFAULT_CANVAS: u32 = 1024;
const DEFAULT_ARTWORK_SCALE: f32 = 0.80;

fn main() -> io::Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let command = args
        .first()
        .ok_or_else(|| usage_err("missing command (pad or ico)"))?
        .as_str();

    match command {
        "pad" => {
            args.remove(0);
            pad_command(&args)
        }
        "ico" => {
            args.remove(0);
            ico_command(&args)
        }
        other => Err(usage_err(&format!("unknown command: {other}"))),
    }
}

fn pad_command(args: &[String]) -> io::Result<()> {
    let source = arg_at(args, 0, "missing source PNG path")?;
    let dest = arg_at(args, 1, "missing destination PNG path")?;
    let mut scale = DEFAULT_ARTWORK_SCALE;
    let mut canvas = DEFAULT_CANVAS;
    let mut index = 2;

    while index < args.len() {
        match args[index].as_str() {
            "--scale" => {
                index += 1;
                scale = parse_scale(arg_at(args, index, "--scale requires a value")?)?;
            }
            "--canvas" => {
                index += 1;
                canvas = arg_at(args, index, "--canvas requires a value")?
                    .parse()
                    .map_err(|_| usage_err("invalid --canvas value"))?;
            }
            other => return Err(usage_err(&format!("unexpected argument: {other}"))),
        }
        index += 1;
    }

    pad_icon(Path::new(source), Path::new(dest), canvas, scale)
}

fn ico_command(args: &[String]) -> io::Result<()> {
    let source = arg_at(args, 0, "missing source PNG path")?;
    let dest = arg_at(args, 1, "missing destination .ico path")?;
    if args.len() > 2 {
        return Err(usage_err("unexpected extra arguments"));
    }
    write_ico(Path::new(source), Path::new(dest))
}

fn arg_at<'a>(args: &'a [String], index: usize, message: &str) -> io::Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| usage_err(message))
}

fn parse_scale(value: &str) -> io::Result<f32> {
    let scale: f32 = value
        .parse()
        .map_err(|_| usage_err("invalid --scale value"))?;
    if !(0.1..=1.0).contains(&scale) {
        return Err(usage_err("--scale must be between 0.1 and 1.0"));
    }
    Ok(scale)
}

fn usage_err(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "{message}\n\
             usage:\n\
               dragent-icon-gen pad <source.png> <dest.png> [--scale 0.80] [--canvas 1024]\n\
               dragent-icon-gen ico <source.png> <dest.ico>"
        ),
    )
}

fn pad_icon(source: &Path, dest: &Path, canvas: u32, scale: f32) -> io::Result<()> {
    let image = image::open(source).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to read {}: {err}", source.display()),
        )
    })?;

    let inner = ((canvas as f32) * scale).round() as u32;
    let scaled = image
        .resize_exact(inner, inner, FilterType::Lanczos3)
        .to_rgba8();
    let mut canvas_img = RgbaImage::new(canvas, canvas);
    let x_off = ((canvas - inner) / 2) as i64;
    let y_off = ((canvas - inner) / 2) as i64;
    overlay(&mut canvas_img, &scaled, x_off, y_off);

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    canvas_img.save(dest).map_err(|err| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("failed to write {}: {err}", dest.display()),
        )
    })
}

fn write_ico(source: &Path, dest: &Path) -> io::Result<()> {
    let image = image::open(source).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to read {}: {err}", source.display()),
        )
    })?;

    let mut icon_dir = IconDir::new(ResourceType::Icon);
    for size in ICO_SIZES {
        let rgba = image
            .resize_exact(size, size, FilterType::Lanczos3)
            .to_rgba8();
        let icon_image = IconImage::from_rgba_data(size, size, rgba.into_raw());
        let entry = ico::IconDirEntry::encode(&icon_image).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to encode {size}x{size} icon layer: {err}"),
            )
        })?;
        icon_dir.add_entry(entry);
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(dest)?;
    icon_dir.write(file).map_err(|err| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("failed to write {}: {err}", dest.display()),
        )
    })
}
