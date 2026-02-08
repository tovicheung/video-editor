use eframe::egui;
use rfd::FileDialog;
use std::process::Command;
use std::path::PathBuf;
use std::time::Instant;
mod player;
use player::{PlayerCommand, VideoPlayer, PREVIEW_WIDTH, PREVIEW_HEIGHT};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(egui::Vec2::new(800.0, 600.0)),
        ..Default::default()
    };
    eframe::run_native(
        "Video Editor",
        options,
        Box::new(|cc| Ok(Box::new(VideoEditorApp::new(cc.egui_ctx.clone())))),
    )
}

#[derive(Clone)]
struct VideoClip {
    path: PathBuf,
    name: String,
    duration: u32,
    timeline_start: u32,
    trim_start: u32,
    trim_end: u32,
}

struct VideoEditorApp {
    clips: Vec<VideoClip>,
    total_timeline_duration: u32,
    playhead: u32,
    is_exporting: bool,
    status_message: String,

    video_player: VideoPlayer,
    current_preview_texture: Option<egui::TextureHandle>,
    last_requested_playhead_ms: u32,
    last_playhead_update_time: Instant,
    current_active_clip_id: Option<usize>,

    is_playing: bool,
    last_play_update_time: Instant,
    
    pending_clip_transition: bool,

    clip_drag_init: u32,
    selected_clip: Option<usize>, // index
}

impl VideoEditorApp {
    fn new(ctx: egui::Context) -> Self {
        Self {
            clips: Vec::new(),
            total_timeline_duration: 30 * 1000,
            playhead: 0,
            is_exporting: false,
            status_message: String::new(),
            video_player: VideoPlayer::new(ctx),
            current_preview_texture: None,
            last_requested_playhead_ms: 0,
            last_playhead_update_time: Instant::now(),
            current_active_clip_id: None,
            is_playing: false,
            last_play_update_time: Instant::now(),
            pending_clip_transition: false,
            clip_drag_init: 0,
            selected_clip: None,
        }
    }
}

impl Drop for VideoEditorApp {
    fn drop(&mut self) {
        self.video_player.send_command(PlayerCommand::Stop);
    }
}

const MIN_CLIP_DURATION: u32 = 100;

fn get_video_duration(path: &PathBuf) -> Result<u32, &str> {
    let output = Command::new("ffprobe")
        .args(&[
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .map_err(|_| "Error running ffprobe")?;

    let duration_str = String::from_utf8(output.stdout)
        .map_err(|_| "Error reading duration from ffprobe result")?
        .trim()
        .to_string();

    let duration_secs: f32 = duration_str.parse().map_err(|_| "Error parsing duration from ffprobe result")?;
    Ok((duration_secs * 1000.0) as u32)
}

impl eframe::App for VideoEditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("test");

            ui.horizontal(|ui| { // toolbar
                if ui.button("Import").clicked() {
                    if let Some(path) = FileDialog::new()
                        .add_filter("Video", &["mp4", "mkv", "mov"])
                        .pick_file() 
                    {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        
                        let duration = match get_video_duration(&path) {
                            Ok(dur) => dur,
                            Err(err) => {
                                self.set_status(err);
                                10000
                            },
                        };
                        
                        let offset = self.clips.iter().map(|c| c.timeline_start + (c.trim_end - c.trim_start)).fold(0, u32::max);

                        self.clips.push(VideoClip {
                            path,
                            name,
                            duration,
                            timeline_start: offset,
                            trim_start: 0,
                            trim_end: duration,
                        });
                        self.set_status("Clip added to timeline.");
                    }
                }

                if !self.clips.is_empty() {
                    if ui.button("Export All").clicked() {
                        if let Some(output) = FileDialog::new()
                            .add_filter("MP4", &["mp4"])
                            .save_file() 
                        {
                            self.export_sequence(output);
                        }
                    }
                    if ui.button("Clear").clicked() {
                        self.clips.clear();
                        // self.clips.clear();
                        self.playhead = 0;
                        self.video_player.send_command(PlayerCommand::StopPlayback);
                        self.is_playing = false;
                    }
                }

                ui.separator();

                if ui.button(if self.is_playing { "⏸ Pause" } else { "▶ Play" }).clicked() {
                    self.is_playing = !self.is_playing;
                    self.last_play_update_time = Instant::now();

                    let active_clip_idx = self.clips.iter().position(|c| {
                        let clip_timeline_end = c.timeline_start + (c.trim_end - c.trim_start);
                        self.playhead >= c.timeline_start && self.playhead < clip_timeline_end
                    });

                    if let Some(idx) = active_clip_idx {
                        if self.is_playing {
                            let active_clip = &self.clips[idx];
                            let clip_playhead_offset_ms = self.playhead - active_clip.timeline_start;
                            
                            // very unoptimized (temp)
                            self.video_player.send_command(PlayerCommand::LoadClip {
                                path: active_clip.path.clone(),
                                trim_start_ms: active_clip.trim_start,
                                trim_end_ms: active_clip.trim_end,
                            });

                            self.video_player.send_command(PlayerCommand::StartPlayback { 
                                timestamp_ms: clip_playhead_offset_ms 
                            });
                        } else {
                            self.video_player.send_command(PlayerCommand::StopPlayback);
                        }
                    }
                    
                    ctx.request_repaint();
                }

                if ui.button("⏪ 5s").clicked() {
                    self.playhead = self.playhead.saturating_sub(5000);
                    self.last_play_update_time = Instant::now();
                    self.last_requested_playhead_ms = u32::MAX;
                    
                    // Stop playback if currently playing
                    if self.is_playing {
                        self.is_playing = false;
                        self.video_player.send_command(PlayerCommand::StopPlayback);
                    }
                    
                    ctx.request_repaint();
                }
                if ui.button("⏩ 5s").clicked() {
                    self.playhead = (self.playhead + 5000).min(self.total_timeline_duration);
                    self.last_play_update_time = Instant::now();
                    self.last_requested_playhead_ms = u32::MAX;
                    
                    // Stop playback if currently playing
                    if self.is_playing {
                        self.is_playing = false;
                        self.video_player.send_command(PlayerCommand::StopPlayback);
                    }
                    
                    ctx.request_repaint();
                }
            });

            ui.separator();

            // move playhead through time
            if self.is_playing {
                let elapsed_ms = self.last_play_update_time.elapsed().as_millis() as u32;
                if elapsed_ms > 0 {
                    self.playhead = (self.playhead + elapsed_ms).min(self.total_timeline_duration);
                    self.last_play_update_time = Instant::now();
                }   

                // reached  end of timeline
                if self.playhead >= self.total_timeline_duration {
                    self.playhead = self.total_timeline_duration;
                    self.is_playing = false;
                    self.video_player.send_command(PlayerCommand::StopPlayback);
                }
            }

            // preview display
            let preview_rect_size = egui::vec2(PREVIEW_WIDTH as f32, PREVIEW_HEIGHT as f32);
            let (preview_resp, painter) = ui.allocate_painter(
                preview_rect_size,
                egui::Sense::hover(),
            );
            painter.rect_filled(preview_resp.rect, 0.0, egui::Color32::from_black_alpha(200));

            if let Some(texture) = &self.current_preview_texture {
                // have a frame
                ui.painter().image(
                    texture.id(),
                    preview_resp.rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.text(
                    preview_resp.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No preview",
                    egui::FontId::proportional(20.0),
                    egui::Color32::WHITE,
                );
            }

            // read new frame from thread
            while let Ok(decoded_frame) = self.video_player.frame_receiver.try_recv() {
                self.current_preview_texture = Some(ctx.load_texture(
                    "video_preview_frame",
                    decoded_frame.image,
                    egui::TextureOptions::LINEAR,
                ));
            }

            // if false && self.is_playing && self.pending_clip_transition {
            //     self.pending_clip_transition = false;
            //     
            //     let current_idx = self.current_active_clip_id.unwrap_or(0);
            //     
            //     if let Some(next_clip) = self.clips.get(current_idx + 1) {
            //         self.playhead = next_clip.timeline_start;
            //         // TODO: handle gap betwen clips
            //         self.video_player.send_command(PlayerCommand::LoadClip {
            //             path: next_clip.path.clone(),
            //             trim_start_ms: next_clip.trim_start,
            //             trim_end_ms: next_clip.trim_end,
            //         });
            //         
            //         self.video_player.send_command(PlayerCommand::StartPlayback {
            //             timestamp_ms: 0,
            //         });
            //         
            //         self.current_active_clip_id = Some(current_idx + 1);
            //         self.last_requested_playhead_ms = 0;
            //         ctx.request_repaint();
            //     } else {
            //         self.is_playing = false;
            //         self.video_player.send_command(PlayerCommand::StopPlayback);
            //     }
            // }

            while let Ok(_) = self.video_player.playback_ended_receiver.try_recv() {
                if self.is_playing {
                    self.pending_clip_transition = true;
                    ctx.request_repaint();
                }
            }

            // request new clip to load
            const MIN_FRAME_REQUEST_INTERVAL_MS_SCRUBBING: u32 = 300;

            let active_clip_idx = self.clips.iter().position(|c| {
                let clip_timeline_end = c.timeline_start + (c.trim_end - c.trim_start);
                self.playhead >= c.timeline_start && self.playhead < clip_timeline_end
            });

            if let Some(clip_idx) = active_clip_idx {
                let mut should_request_new_frame = false;

                let active_clip = &self.clips[clip_idx];
                let clip_playhead_offset_ms = self.playhead - active_clip.timeline_start;

                if self.current_active_clip_id != Some(clip_idx) {
                    // load new clip
                    self.current_active_clip_id = Some(clip_idx);
                    let active_clip = &self.clips[clip_idx];
                    self.video_player.send_command(PlayerCommand::LoadClip {
                        path: active_clip.path.clone(),
                        trim_start_ms: active_clip.trim_start,
                        trim_end_ms: active_clip.trim_end,
                    });
                    should_request_new_frame = true;
                    self.last_requested_playhead_ms = u32::MAX;

                    if self.is_playing {
                        self.video_player.send_command(PlayerCommand::StartPlayback {
                            timestamp_ms: clip_playhead_offset_ms,
                        });
                    }
                }

                if !self.is_playing { // scrubbing
                    let time_since_last_request = self.last_playhead_update_time.elapsed().as_millis() as u32;

                    if should_request_new_frame ||
                        (clip_playhead_offset_ms != self.last_requested_playhead_ms &&
                        time_since_last_request >= MIN_FRAME_REQUEST_INTERVAL_MS_SCRUBBING) {
                        
                        self.video_player.send_command(PlayerCommand::Seek {
                            timestamp_ms: clip_playhead_offset_ms,
                        });
                        self.last_requested_playhead_ms = clip_playhead_offset_ms;
                        self.last_playhead_update_time = Instant::now();
                    }
                }
            } else {
                self.current_preview_texture = Some(ctx.load_texture(
                    "video_preview_frame",
                    egui::ColorImage::filled([PREVIEW_WIDTH as usize, PREVIEW_HEIGHT as usize], egui::Color32::BLACK),
                    egui::TextureOptions::LINEAR,
                ));
            }

            if self.is_playing {
                ctx.request_repaint();
            }

            ui.add_space(30.0);

            // timeline
            ui.label("Timeline");
            let timeline_height = 60.0;
            let (timeline_rect, _resp) = ui.allocate_at_least(egui::vec2(ui.available_width(), timeline_height), egui::Sense::hover());
            ui.painter().rect_filled(timeline_rect, 4.0, egui::Color32::from_gray(40));

            let time_to_x = |t: u32| timeline_rect.left() + (t as f32 / self.total_timeline_duration as f32) * timeline_rect.width();
            let x_to_time = |x: f32| (((x - timeline_rect.left()) / timeline_rect.width()) * self.total_timeline_duration as f32).round() as u32;

            let mut clip_to_update = None;

            for (idx, clip) in self.clips.iter().enumerate() {
                let is_selected = self.selected_clip == Some(idx);
                let clip_duration = clip.trim_end - clip.trim_start;

                let start_x = time_to_x(clip.timeline_start);
                let end_x = time_to_x(clip.timeline_start + clip_duration);
                
                let clip_rect = egui::Rect::from_x_y_ranges(start_x..=end_x, timeline_rect.top()..=timeline_rect.bottom());
                ui.painter().rect_filled(clip_rect, 2.0, if is_selected { egui::Color32::from_rgb(60, 60, 200) } else { egui::Color32::from_rgb(60, 120, 180) });
                ui.painter().rect_stroke(clip_rect, 2.0, egui::Stroke::new(1.0, egui::Color32::WHITE), egui::StrokeKind::Inside);

                let handle_w = 10.0;

                let middle_drag_rect = egui::Rect::from_x_y_ranges(
                    (start_x + handle_w)..=(end_x - handle_w),
                    timeline_rect.top()..=timeline_rect.bottom(),
                );
                let l_handle = egui::Rect::from_x_y_ranges(start_x..=(start_x + handle_w), timeline_rect.top()..=timeline_rect.bottom());
                let r_handle = egui::Rect::from_x_y_ranges((end_x - handle_w)..=end_x, timeline_rect.top()..=timeline_rect.bottom());

                let l_res = ui.interact(l_handle, egui::Id::new((idx, "l")), egui::Sense::drag());
                let r_res = ui.interact(r_handle, egui::Id::new((idx, "r")), egui::Sense::drag());

                let middle_res = ui.interact(middle_drag_rect, egui::Id::new((idx, "middle")), egui::Sense::drag());

                if l_res.hovered() || r_res.hovered() || l_res.dragged() || r_res.dragged() {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                } else if middle_res.hovered() || middle_res.dragged() {
                    ctx.set_cursor_icon(if middle_res.dragged() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Grab
                    });
                }

                if l_res.dragged() {
                    let timeline_end = clip.timeline_start + clip.trim_end - clip.trim_start;
                    let pointer_x = ctx.input(|i| i.pointer.latest_pos().unwrap_or_default()).x;
                    let new_timeline_start = x_to_time(pointer_x)
                        .clamp(0, self.total_timeline_duration - MIN_CLIP_DURATION)
                        .clamp(clip.timeline_start - clip.trim_start, timeline_end - MIN_CLIP_DURATION);

                    let new_trim_start = clip.trim_end - (timeline_end - new_timeline_start);
                    
                    clip_to_update = Some((idx, new_timeline_start, new_trim_start, clip.trim_end));
                }
                if r_res.dragged() {
                    let pointer_x = ctx.input(|i| i.pointer.latest_pos().unwrap_or_default()).x;
                    let new_timeline_end = x_to_time(pointer_x)
                        .clamp(clip.timeline_start + MIN_CLIP_DURATION, self.total_timeline_duration);
                    let new_trim_end = (clip.trim_start + (new_timeline_end - clip.timeline_start))
                        .clamp(clip.trim_start + MIN_CLIP_DURATION, clip.duration);
                    clip_to_update = Some((idx, clip.timeline_start, clip.trim_start, new_trim_end));
                }
                
                if middle_res.drag_started() {
                    println!("dragstart");
                    self.clip_drag_init = clip.timeline_start;
                    self.selected_clip = Some(idx);
                }

                if middle_res.dragged() {
                    let pointer_pos = ctx.input(|i| i.pointer.press_origin()).unwrap_or_default();
                    let current_pos = ctx.input(|i| i.pointer.latest_pos().unwrap_or_default());
                    // println!("{} {}", pointer_pos, current_pos);

                    let prev = self.clips.iter()
                        .map(|c| { c.timeline_start + c.trim_end - c.trim_start })
                        .filter(|timeline_end| { *timeline_end <= clip.timeline_start })
                        .max()
                        .unwrap_or(0);

                    let timeline_end = clip.timeline_start + clip.trim_end - clip.trim_start;

                    let next = self.clips.iter()
                        .map(|c| { c.timeline_start })
                        .filter(|timeline_start| { *timeline_start >= timeline_end })
                        .min()
                        .unwrap_or(self.total_timeline_duration)
                         - clip_duration;

                    // println!("{} {}   {}", prev, next, x_to_time(time_to_x(self.clip_drag_init) + current_pos.x - pointer_pos.x));
                    let new_timeline_start = x_to_time(time_to_x(self.clip_drag_init) + current_pos.x - pointer_pos.x)
                        .clamp(prev, next.max(0));
                    
                    clip_to_update = Some((idx, new_timeline_start, clip.trim_start, clip.trim_end));
                }

                if middle_res.drag_stopped() {
                    self.clip_drag_init = 0;
                }

                ui.painter().rect_filled(l_handle, 2.0, egui::Color32::LIGHT_GREEN);
                ui.painter().rect_filled(r_handle, 2.0, egui::Color32::LIGHT_GREEN);

                ui.painter().text(clip_rect.left_top() + egui::vec2(5.0, 15.0), egui::Align2::LEFT_TOP, &clip.name, egui::FontId::proportional(12.0), egui::Color32::WHITE);
            }

            if let Some((idx, new_timeline_start, new_start, new_end)) = clip_to_update {
                // stop playback when editing
                if self.is_playing {
                    self.is_playing = false;
                    self.video_player.send_command(PlayerCommand::StopPlayback);
                }
                
                self.clips[idx].timeline_start = new_timeline_start;
                self.clips[idx].trim_start = new_start;
                self.clips[idx].trim_end = new_end;
            }

            let ph_x = time_to_x(self.playhead);

            
            let ph_rect = egui::Rect::from_x_y_ranges(ph_x-1.0..=ph_x+1.0, timeline_rect.top()-20.0..=timeline_rect.bottom());
            ui.painter().rect_filled(ph_rect, 2.0, egui::Color32::RED);

            let ph_jump_rect = egui::Rect::from_min_max(egui::pos2(timeline_rect.min.x, timeline_rect.min.y - 20.0), timeline_rect.max);
            
            let ph_jump_res = ui.interact(ph_jump_rect, egui::Id::new("ph_jump"), egui::Sense::drag());

            if ph_jump_res.dragged() {
                let pointer_x = ctx.input(|i| i.pointer.latest_pos().unwrap_or_default()).x;
                self.playhead = x_to_time(pointer_x);
            }


            // ui.painter().line_segment(
            //     [egui::pos2(ph_x, rect.top() - 30.0), egui::pos2(ph_x, rect.bottom())],
            //     egui::Stroke::new(3.0, egui::Color32::RED),
            // );

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Status: {}", self.status_message));
                    if self.is_exporting { ui.add(egui::Spinner::new()); }
                });
            });
        });
    }
}

impl VideoEditorApp {
    fn set_status(&mut self, status: &str) {
        self.status_message = status.to_string();
    }

    fn export_sequence(&mut self, output: PathBuf) {
        self.is_exporting = true;
        self.set_status("Exporting video ...");

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y");

        for clip in &self.clips {
            cmd.arg("-ss").arg(format!("{:.2}", clip.trim_start as f32 / 1000.0))
               .arg("-t").arg(format!("{:.2}", (clip.trim_end - clip.trim_start) as f32 / 1000.0))
               .arg("-i").arg(&clip.path);
        }

        let mut filter_parts = Vec::new();
        for i in 0..self.clips.len() {
            filter_parts.push(format!("[{}:v]scale=w=1920:h=1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2,setsar=1,setdar=16/9[v{}];", i, i));
        }
        
        let mut concat_inputs = String::new();
        for i in 0..self.clips.len() {
            concat_inputs.push_str(&format!("[v{}][{}:a]", i, i));
        }
        
        let filter_complex = format!(
            "{}{}concat=n={}:v=1:a=1[outv][outa]",
            filter_parts.join(""),
            concat_inputs,
            self.clips.len()
        );
        
        cmd.arg("-filter_complex")
           .arg(filter_complex)
           .arg("-map").arg("[outv]")
           .arg("-map").arg("[outa]")
           .arg(output);

        let status = cmd.status();

        match status {
            Ok(s) if s.success() => self.set_status("exported successfully!"),
            _ => self.set_status("export failed!"),
        }
        self.is_exporting = false;
    }
}
