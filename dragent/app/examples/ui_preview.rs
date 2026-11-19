//! Draw screens with sample data and save them as PNGs (see
//! `dragent::ui_preview`). `cargo run -p dragent --example ui_preview -- <folder>`

use image::ImageEncoder as _;

fn save_frame(mut frame: warpui::platform::CapturedFrame, path: &std::path::Path) -> Result<(), String> {
    frame.ensure_rgba();
    let file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    image::codecs::png::PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Sub,
    )
        .write_image(
            &frame.data,
            frame.width,
            frame.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| error.to_string())
}


fn save(frame: warpui::platform::CapturedFrame, path: &std::path::Path) -> Result<(), String> {
    if std::env::var_os("DRAGENT_PREVIEW_RECORD").is_none() {
        return save_frame(frame, path);
    }
    // Encoding stays off the render thread so interactions remain responsive.
    type Frame = (warpui::platform::CapturedFrame, std::path::PathBuf);
    static QUEUE: std::sync::OnceLock<std::sync::mpsc::SyncSender<Frame>> = std::sync::OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Frame>(2);
        let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
        for _ in 0..1 {
            let receiver = receiver.clone();
            std::thread::spawn(move || loop {
                let next = receiver.lock().unwrap().recv();
                let Ok((frame, path)) = next else { break };
                if let Err(error) = save_frame(frame, &path) { eprintln!("Frame encoding: {error}"); }
            });
        }
        sender
    });
    // Drop a frame if the encoder is behind; timestamps preserve real playback time.
    let _ = queue.try_send((frame, path.to_path_buf()));
    Ok(())
}

fn main() {
    // App code that starts a background service relaunches the current
    // executable with a `--dragent-…` flag. Here that is this preview, so
    // such a launch must end at once instead of opening another window.
    if std::env::args().skip(1).any(|arg| arg.starts_with("--dragent")) {
        std::process::exit(0);
    }
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ui-preview".into());
    dragent::ui_preview::run(out.into(), save);
}
