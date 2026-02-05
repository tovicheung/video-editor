use std::path::PathBuf;
use std::process::{Command, Stdio, Child};
use std::io::{Read, BufReader};
use std::thread;
use std::sync::mpsc;

pub const PREVIEW_WIDTH: u32 = 640;
pub const PREVIEW_HEIGHT: u32 = 360;


pub enum PlayerCommand {
    LoadClip {
        path: PathBuf,
        trim_start_ms: u32,
        trim_end_ms: u32,
    },
    StartPlayback {
        timestamp_ms: u32, // relative to trimmed clip
    },
    StopPlayback,
    Seek {
        timestamp_ms: u32, // scrubbing
    },
    Stop,
}

pub struct DecodedFrame {
    pub image: egui::ColorImage,
    _timestamp_ms: u32,
}

pub struct PlaybackEnded;


pub struct VideoPlayer {
    command_sender: mpsc::Sender<PlayerCommand>,
    pub frame_receiver: mpsc::Receiver<DecodedFrame>,
    pub playback_ended_receiver: mpsc::Receiver<PlaybackEnded>,
    _thread_handle: thread::JoinHandle<()>,
}

impl VideoPlayer {
    pub fn new(ctx: egui::Context) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        let (frame_sender, frame_receiver) = mpsc::channel();
        let (playback_ended_sender, playback_ended_receiver) = mpsc::channel();
        let egui_ctx_clone = ctx.clone();

        let thread_handle = thread::spawn(move || {
            let mut last_frame_time = std::time::Instant::now();
            const TARGET_FRAME_TIME: std::time::Duration = std::time::Duration::from_millis(33);

            let mut current_clip_path: Option<PathBuf> = None;
            let mut current_clip_trim_start_ms: u32 = 0;
            let mut current_clip_trim_end_ms: u32 = 0;
            
            // ffmpeg subprocess
            let mut playback_process: Option<Child> = None;
            let mut playback_stdout: Option<BufReader<std::process::ChildStdout>> = None;
            let mut is_playing = false;

            loop {
                if let Ok(cmd) = command_receiver.try_recv() {
                    match cmd {
                        PlayerCommand::LoadClip { path, trim_start_ms, trim_end_ms } => {
                            println!("main -> player: LoadClip");
                            current_clip_path = Some(path.clone());
                            current_clip_trim_start_ms = trim_start_ms;
                            current_clip_trim_end_ms = trim_end_ms;
                            
                            if let Some(mut child) = playback_process.take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                            playback_stdout = None;
                            is_playing = false;
                        }
                        PlayerCommand::StartPlayback { timestamp_ms } => {
                            println!("main -> player: StartPlayBack");
                            // dont play twice
                            if !is_playing {
                                if let Some(path) = &current_clip_path {
                                    if let Some(mut child) = playback_process.take() {
                                        // kill existing process
                                        let _ = child.kill();
                                        let _ = child.wait();
                                    }
                                    
                                    let ffmpeg_seek_time_secs = (current_clip_trim_start_ms + timestamp_ms) as f32 / 1000.0;
                                    let mut cmd = Command::new("ffmpeg");
                                    cmd.arg("-ss").arg(format!("{:.3}", ffmpeg_seek_time_secs))
                                        .arg("-to").arg(format!("{:.3}", current_clip_trim_end_ms as f32 / 1000.0))
                                        .arg("-i").arg(path)
                                        .arg("-vf").arg(format!("scale={}:{}", PREVIEW_WIDTH, PREVIEW_HEIGHT))
                                        .arg("-pix_fmt").arg("rgba")
                                        .arg("-f").arg("rawvideo")
                                        .arg("-") // continuous stdout
                                        .stderr(Stdio::null());

                                    println!("player: calling ffmpeg");

                                    match cmd.stdout(Stdio::piped()).spawn() {
                                        Ok(mut child) => {
                                            playback_stdout = child.stdout.take().map(|s| BufReader::new(s));
                                            playback_process = Some(child);
                                            is_playing = true;
                                            println!("player: started persistent playback of clip starting at {:.3}s", ffmpeg_seek_time_secs);
                                        }
                                        Err(e) => eprintln!("player: Failed to start playback: {}", e),
                                    }
                                }
                            }
                        }
                        PlayerCommand::StopPlayback => {
                            if let Some(mut child) = playback_process.take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                            playback_stdout = None;
                            is_playing = false;
                            println!("main -> player: StopPlayback");
                        }
                        PlayerCommand::Seek { timestamp_ms } => {
                            println!("main -> player: Seek");
                            if !is_playing { // scrubbing
                                if let Some(path) = &current_clip_path {
                                    let ffmpeg_seek_time_secs = (current_clip_trim_start_ms + timestamp_ms) as f32 / 1000.0;
                                    
                                    let mut cmd = Command::new("ffmpeg");
                                    cmd.arg("-ss").arg(format!("{:.3}", ffmpeg_seek_time_secs))
                                       .arg("-i").arg(path)
                                       .arg("-frames:v").arg("1")
                                       .arg("-vf").arg(format!("scale={}:{}", PREVIEW_WIDTH, PREVIEW_HEIGHT))
                                       .arg("-pix_fmt").arg("rgba")
                                       .arg("-f").arg("rawvideo")
                                       .arg("-")
                                       .stderr(Stdio::null());

                                    if let Ok(mut child) = cmd.stdout(Stdio::piped()).spawn() {
                                        if let Some(mut stdout) = child.stdout.take() {
                                            let frame_size = (PREVIEW_WIDTH * PREVIEW_HEIGHT * 4) as usize;
                                            let mut buffer = vec![0u8; frame_size];
                                            if stdout.read_exact(&mut buffer).is_ok() {
                                                let image = egui::ColorImage::from_rgba_unmultiplied(
                                                    [PREVIEW_WIDTH as usize, PREVIEW_HEIGHT as usize],
                                                    &buffer,
                                                );
                                                let _ = frame_sender.send(DecodedFrame { 
                                                    image, 
                                                    _timestamp_ms: timestamp_ms 
                                                });
                                                egui_ctx_clone.request_repaint();
                                            }
                                        }
                                        let _ = child.wait();
                                    }
                                }
                            }
                        }
                        PlayerCommand::Stop => {
                            // Clean shutdown
                            if let Some(mut child) = playback_process.take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                            break;
                        }
                    }
                    continue; // skip for this tick
                }

                if is_playing {
                    if let Some(stdout) = &mut playback_stdout {
                        let elapsed = last_frame_time.elapsed();
                        if elapsed < TARGET_FRAME_TIME {
                            thread::sleep(TARGET_FRAME_TIME - elapsed);
                        }
                        last_frame_time = std::time::Instant::now();
                        let frame_size = (PREVIEW_WIDTH * PREVIEW_HEIGHT * 4) as usize;
                        let mut buffer = vec![0u8; frame_size];
                        
                        match stdout.read_exact(&mut buffer) {
                            Ok(_) => {
                                let image = egui::ColorImage::from_rgba_unmultiplied(
                                    [PREVIEW_WIDTH as usize, PREVIEW_HEIGHT as usize],
                                    &buffer,
                                );
                                let _ = frame_sender.send(DecodedFrame { 
                                    image, 
                                    _timestamp_ms: 0
                                });
                                egui_ctx_clone.request_repaint();
                            }
                            Err(_) => { // playback finished
                                if let Some(mut child) = playback_process.take() {
                                    let _ = child.wait();
                                }
                                playback_stdout = None;
                                is_playing = false;
                                println!("player -> main: PlaybackEnded");
                                
                                let _ = frame_sender.send(DecodedFrame { 
                                    image: egui::ColorImage::filled([PREVIEW_WIDTH as usize, PREVIEW_HEIGHT as usize], egui::Color32::BLACK),
                                    _timestamp_ms: 0 
                                });
                                let _ = playback_ended_sender.send(PlaybackEnded);
                            }
                        }
                    }
                }

                if !is_playing {
                    thread::sleep(std::time::Duration::from_millis(10)); // avoid busy waiting
                } else {
                    thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });

        Self {
            command_sender,
            frame_receiver,
            playback_ended_receiver,
            _thread_handle: thread_handle,
        }
    }

    pub fn send_command(&self, command: PlayerCommand) {
        let _ = self.command_sender.send(command);
    }
}
