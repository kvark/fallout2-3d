//! Launch Arroyo, play one fixed session, and write one PNG per corner.
//!
//! `cargo run --release -- --output DIR` walks, talks, and takes one shot, then
//! writes `corner0.png` .. `corner3.png` of that pose.
//! `--window` presents the current corner and accepts the same commands from keys.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use fallout2_3d::render::{self, FRAME_HEIGHT, FRAME_WIDTH};
use fallout2_3d::window;
use fallout2_3d::{session_commands, Game};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let mut output: Option<PathBuf> = None;
    let mut show_window = false;
    let mut window_secs = 8u64;
    while let Some(arg) = args.next() {
        if arg == "--output" {
            output = args.next().map(PathBuf::from);
        } else if arg == "--window" {
            show_window = true;
        } else if arg == "--window-secs" {
            let Some(value) = args.next() else {
                eprintln!("--window-secs needs a number of seconds");
                return ExitCode::from(2);
            };
            match value.parse::<u64>() {
                Ok(secs) => window_secs = secs,
                Err(_) => {
                    eprintln!("--window-secs {value} is not a whole number");
                    return ExitCode::from(2);
                }
            }
        } else if arg == "--help" {
            eprintln!("fallout2-3d --output DIR [--window] [--window-secs N]");
            eprintln!("plays one step, one dialogue choice, and one shot; window keys repeat those commands");
            return ExitCode::SUCCESS;
        } else {
            eprintln!("unknown argument {arg}");
            return ExitCode::from(2);
        }
    }
    if output.is_none() && !show_window {
        eprintln!("pass --output DIR");
        return ExitCode::from(2);
    }
    let mut game = match Game::load() {
        Ok(game) => game,
        Err(err) => {
            eprintln!("load failed: {err}");
            return ExitCode::from(1);
        }
    };
    let commands = match session_commands(&mut game) {
        Ok(commands) => commands,
        Err(err) => {
            eprintln!("session failed: {err}");
            return ExitCode::from(1);
        }
    };
    if show_window {
        if let Err(err) = window::play(
            &mut game,
            commands,
            output.as_deref(),
            Duration::from_secs(window_secs),
        ) {
            eprintln!("window failed: {err}");
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }
    if let Err(err) = fallout2_3d::play::run_commands(&mut game, commands) {
        eprintln!("play failed: {err}");
        return ExitCode::from(1);
    }
    let rendered = match render::render_corners(&mut game) {
        Ok(rendered) => rendered,
        Err(err) => {
            eprintln!("render failed: {err}");
            return ExitCode::from(1);
        }
    };
    println!("blade context: {}", rendered.device_name);
    println!("texture readback: {FRAME_WIDTH}x{FRAME_HEIGHT}");
    if let Some(output) = &output {
        if let Err(err) = write_frames(output, &rendered) {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

fn write_frames(output: &std::path::Path, rendered: &fallout2_3d::render::Rendered) -> Result<(), String> {
    for (index, frame) in rendered.frames.iter().enumerate() {
        let path = output.join(format!("corner{index}.png"));
        render::write_png(&path, frame).map_err(|err| format!("write {} failed: {err}", path.display()))?;
    }
    Ok(())
}
