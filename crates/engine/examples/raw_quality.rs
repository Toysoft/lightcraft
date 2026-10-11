//! Compare the current and legacy detail processing on a locally supplied RAW.
//! `cargo run -p lightcraft-engine --example raw_quality -- IN.NEF OUT_DIR [full]`
//! Outputs are scratch diagnostics; no photographs are included in the repository.
use lightcraft_develop::{DevelopSettings, ProcessVersion};
use lightcraft_engine::export::{ExportFormat, ExportOptions, encode_image};
use lightcraft_pipeline::{RenderRequest, render};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let input = args.get(1).ok_or("expected input RAW path")?;
    let output = std::path::Path::new(args.get(2).ok_or("expected output directory")?);
    std::fs::create_dir_all(output)?;
    let full = args.get(3).is_some_and(|arg| arg == "full");
    let edge = if full { usize::MAX } else { 2560 };
    let (source, info) = lightcraft_engine::files::load_vec(std::fs::read(input)?, edge)?;
    let request = if full { RenderRequest::fit(source.width, source.height) } else { RenderRequest::fit(1400, 1400) };
    for process in [1, 2, 3] {
        let mut settings = DevelopSettings { process: ProcessVersion(process), ..Default::default() };
        settings.detail.sharpen_amount = 40.0;
        settings.detail.nr_color = 25.0;
        if process >= 3 {
            settings.detail.nr_luminance = 25.0;
        }
        let rendered = render(&source, &info, &settings, &request);
        let options = ExportOptions { format: ExportFormat::Png, ..Default::default() };
        let destination = output.join(format!("process-{process}.png"));
        let encoded = encode_image(&rendered.image, &options)?;
        std::fs::OpenOptions::new().write(true).create_new(true).open(&destination)?.write_all(&encoded)?;
        eprintln!("{}: {} × {}", destination.display(), rendered.image.width, rendered.image.height);
    }
    Ok(())
}
