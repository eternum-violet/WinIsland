use crate::core::config::{PADDING, TOP_OFFSET};
use crate::core::hook_server::ClaudeState;
use crate::core::smtc::MediaInfo;
use crate::ui::expanded::main_view::{
    draw_main_page, draw_text_cached, draw_visualizer, get_cached_media_image, get_media_palette,
};
use crate::ui::expanded::widget_view::{draw_approval_page, draw_widget_page};
use crate::utils::glass::get_glass_background;
use crate::window::app::ClaudeForegroundApproval;
use skia_safe::{
    ClipOp, Color, FilterMode, ISize, MipmapMode, Paint, RRect, Rect, SamplingOptions,
    Surface as SkSurface, image_filters, surfaces,
};
use softbuffer::Surface;
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use winit::window::Window;

thread_local! {
    static SK_SURFACE: RefCell<Option<SkSurface>> = RefCell::new(None);
    static LAST_CLAUDE_RENDER_LOG: RefCell<Option<String>> = const { RefCell::new(None) };
    static LAST_CLAUDE_PULSE_LOG: RefCell<Option<String>> = const { RefCell::new(None) };
    static LAST_CLAUDE_PULSE_BUCKET: RefCell<Option<u64>> = const { RefCell::new(None) };
}

fn pulse_debug_log_path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push(".winisland");
        let _ = std::fs::create_dir_all(&path);
        path.push("claude-pulse-debug.log");
        path
    })
}

fn append_pulse_debug_line(line: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(pulse_debug_log_path())
    {
        let _ = writeln!(file, "{}", line);
    }
}

fn claude_state_label(state: &ClaudeState) -> &'static str {
    match state {
        ClaudeState::Idle => "Idle",
        ClaudeState::Thinking { .. } => "Thinking",
        ClaudeState::Working { .. } => "Working",
        ClaudeState::Done { .. } => "Done",
        ClaudeState::Attention { .. } => "Attention",
        ClaudeState::ApprovalRequired { .. } => "ApprovalRequired",
    }
}

fn log_claude_render(signature: String) {
    let ascii_signature: String = signature
        .chars()
        .map(|ch| if ch.is_ascii() { ch } else { '?' })
        .collect();
    LAST_CLAUDE_RENDER_LOG.with(|cell| {
        let mut last = cell.borrow_mut();
        if last.as_ref() != Some(&ascii_signature) {
            println!("CLAUDE_RENDER {}", ascii_signature);
            *last = Some(ascii_signature);
        }
    });
}

fn log_claude_pulse(signature: String) {
    LAST_CLAUDE_PULSE_LOG.with(|cell| {
        let mut last = cell.borrow_mut();
        if last.as_ref() != Some(&signature) {
            println!("CLAUDE_PULSE {}", signature);
            *last = Some(signature);
        }
    });
}

fn log_claude_pulse_bucket(bucket: u64, signature: String) {
    LAST_CLAUDE_PULSE_BUCKET.with(|cell| {
        let mut last = cell.borrow_mut();
        if last.map(|value| value != bucket).unwrap_or(true) {
            let line = format!("CLAUDE_PULSE_FRAME {} {}", bucket, signature);
            println!("{}", line);
            append_pulse_debug_line(&line);
            *last = Some(bucket);
        }
    });
}

pub fn draw_island(
    surface: &mut Surface<Arc<Window>, Arc<Window>>,
    current_w: f32,
    current_h: f32,
    current_r: f32,
    os_w: u32,
    os_h: u32,
    _weights: [f32; 4],
    sigmas: (f32, f32),
    expansion_progress: f32,
    view_offset: f32,
    media: &MediaInfo,
    music_active: bool,
    global_scale: f32,
    current_lyric: &str,
    old_lyric: &str,
    lyric_transition: f32,
    current_claude_text: &str,
    old_claude_text: &str,
    claude_transition: f32,
    claude_waiting_visual: bool,
    use_blur: bool,
    hide_progress: f32,
    lyric_scroll_offset: f32,
    island_style: &str,
    win_x: i32,
    win_y: i32,
    font_size: f32,
    claude_pulse_time: f32,
    claude_entries: &[crate::window::app::ClaudeDisplayEntry],
    foreground_approval: Option<&ClaudeForegroundApproval>,
    selected_claude_session_id: Option<&str>,
    claude_detail_scroll: f32,
    claude_status: &ClaudeState,
) {
    let mut buffer = surface.buffer_mut().unwrap();
    let mut sk_surface = SK_SURFACE.with(|cell| {
        let mut opt = cell.borrow_mut();
        if let Some(ref s) = *opt {
            if s.width() == os_w as i32 && s.height() == os_h as i32 {
                return s.clone();
            }
        }
        let new_surface =
            surfaces::raster_n32_premul(ISize::new(os_w as i32, os_h as i32)).unwrap();
        *opt = Some(new_surface.clone());
        new_surface
    });
    let canvas = sk_surface.canvas();
    canvas.clear(Color::TRANSPARENT);

    let offset_x = (os_w as f32 - current_w) / 2.0;
    let base_y = PADDING / 2.0;
    let hidden_peek_h = (5.0 * global_scale).max(3.0);
    let hide_distance = (current_h - hidden_peek_h + TOP_OFFSET as f32).max(0.0);
    let hide_y_offset = hide_progress * hide_distance;
    let offset_y = base_y - hide_y_offset;

    let rect = Rect::from_xywh(offset_x, offset_y, current_w, current_h);
    let rrect = RRect::new_rect_xy(rect, current_r, current_r);
    let has_blur = sigmas.0 > 0.1 || sigmas.1 > 0.1;
    let blur_filter = if has_blur {
        image_filters::blur(sigmas, None, None, None)
    } else {
        None
    };
    canvas.save();
    canvas.clip_rrect(rrect, ClipOp::Intersect, true);

    if island_style == "glass" {
        let screen_x = win_x + offset_x as i32;
        let screen_y = win_y + offset_y as i32;
        if let Some(bg_img) = get_glass_background(
            screen_x,
            screen_y,
            current_w as u32,
            current_h as u32,
            40.0 * global_scale,
        ) {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
            canvas.draw_image_rect_with_sampling_options(&bg_img, None, rect, sampling, &paint);
        }
        let mut overlay = Paint::default();
        overlay.set_color(Color::from_argb(120, 0, 0, 0));
        overlay.set_anti_alias(true);
        canvas.draw_rrect(rrect, &overlay);
    } else {
        let mut bg_paint = Paint::default();
        bg_paint.set_color(Color::BLACK);
        bg_paint.set_anti_alias(true);
        canvas.draw_rrect(rrect, &bg_paint);
    }

    let expanded_alpha_f = (expansion_progress.powf(2.0)).clamp(0.0, 1.0) * (1.0 - hide_progress);
    let mini_alpha_f = (1.0 - expansion_progress * 1.5).clamp(0.0, 1.0) * (1.0 - hide_progress);

    let viz_h_scale = 0.45 + (1.0 - 0.45) * expansion_progress;

    if expanded_alpha_f > 0.01 {
        let alpha = (expanded_alpha_f * 255.0) as u8;
        let page_shift = view_offset * current_w;

        canvas.save();
        if let Some(ref filter) = blur_filter {
            let mut layer_paint = Paint::default();
            layer_paint.set_image_filter(filter.clone());
            canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default().paint(&layer_paint));
        }

        canvas.save();
        canvas.translate((-page_shift, 0.0));
        draw_main_page(
            canvas,
            offset_x,
            offset_y,
            current_w,
            current_h,
            alpha,
            media,
            music_active,
            view_offset,
            global_scale,
            expansion_progress,
            viz_h_scale * global_scale,
            use_blur,
            font_size,
        );
        canvas.restore();

        if view_offset > 0.98 {
            canvas.save();
            canvas.translate((current_w - page_shift, 0.0));
            if let Some(approval) = foreground_approval {
                draw_approval_page(
                    canvas,
                    offset_x,
                    offset_y,
                    current_w,
                    current_h,
                    alpha,
                    global_scale,
                    approval,
                    claude_detail_scroll,
                );
            } else {
                draw_widget_page(
                    canvas,
                    offset_x,
                    offset_y,
                    current_w,
                    current_h,
                    alpha,
                    global_scale,
                    claude_entries,
                    selected_claude_session_id,
                    claude_detail_scroll,
                );
            }
            canvas.restore();
        }

        if blur_filter.is_some() {
            canvas.restore();
        }
        canvas.restore();
    }
    if mini_alpha_f > 0.01 && current_w > 45.0 * global_scale {
        let alpha = (mini_alpha_f * 255.0) as u8;

        // Determine which page we're on based on view_offset
        let on_page_2 = view_offset > 0.5;
        let claude_visible = !matches!(claude_status, ClaudeState::Idle);
        let lyrics_visible = !current_lyric.is_empty() || !old_lyric.is_empty();

        if claude_visible && !on_page_2 && music_active {
            log_claude_render(format!(
                "draw=false reason=collapsed_music_gate state={} on_page_2={} music_active={} alpha={}",
                claude_state_label(claude_status),
                on_page_2,
                music_active,
                alpha
            ));
        } else if claude_visible {
            log_claude_render(format!(
                "draw=true state={} on_page_2={} music_active={} alpha={}",
                claude_state_label(claude_status),
                on_page_2,
                music_active,
                alpha
            ));
        }

        if on_page_2 {
            draw_claude_pill(
                canvas,
                offset_x,
                offset_y,
                current_w,
                current_h,
                alpha,
                global_scale,
                claude_status,
                current_claude_text,
                old_claude_text,
                claude_transition,
                claude_waiting_visual,
                claude_pulse_time,
                use_blur,
            );
        } else if music_active {
            // Page 1: Show music info in pill
            if let Some(image) = get_cached_media_image(media) {
                let size = 18.0 * global_scale;
                let ix = offset_x + 8.0 * global_scale;
                let iy = offset_y + (current_h - size) / 2.0;
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_alpha_f(alpha as f32 / 255.0);
                canvas.save();
                canvas.clip_rrect(
                    RRect::new_rect_xy(
                        Rect::from_xywh(ix, iy, size, size),
                        5.0 * global_scale,
                        5.0 * global_scale,
                    ),
                    ClipOp::Intersect,
                    true,
                );
                let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear);
                canvas.draw_image_rect_with_sampling_options(
                    &image,
                    None,
                    Rect::from_xywh(ix, iy, size, size),
                    sampling,
                    &paint,
                );
                canvas.restore();
            }
            let palette = get_media_palette(media);
            let viz_x = offset_x + current_w - 17.0 * global_scale;
            let viz_y = offset_y + current_h / 2.0;
            draw_visualizer(
                canvas,
                viz_x,
                viz_y,
                alpha,
                media.is_playing,
                &palette,
                &media.spectrum,
                0.55 * global_scale,
                viz_h_scale * global_scale,
                (0.6, 0.08),
            );
        }

        if !on_page_2 && lyrics_visible {
            let lyric_fade_f = (1.0 - expansion_progress * 2.5).clamp(0.0, 1.0);
            let alpha = (alpha as f32 * lyric_fade_f) as u8;

            if alpha > 0 {
                let lyric_font_sz = if font_size > 0.0 {
                    font_size * 0.8 * global_scale
                } else {
                    12.0 * global_scale
                };
                let space_left = offset_x + 30.0 * global_scale;
                let space_right = offset_x + current_w - 29.0 * global_scale;
                let available_w = space_right - space_left;
                let scrolling = lyric_scroll_offset > 0.0;
                let text_x = if scrolling {
                    space_left - lyric_scroll_offset
                } else {
                    space_left + available_w / 2.0
                };
                let text_centered = !scrolling;
                let text_max_w = if scrolling { 10000.0 } else { available_w };

                canvas.save();
                let clip_rect = Rect::from_xywh(space_left, offset_y, available_w, current_h);
                canvas.clip_rect(clip_rect, ClipOp::Intersect, true);

                if use_blur {
                    if lyric_transition < 1.0 && !old_lyric.is_empty() {
                        let mut text_paint = Paint::default();
                        text_paint.set_anti_alias(true);
                        let fade_alpha = (alpha as f32 * (1.0 - lyric_transition)) as u8;
                        text_paint.set_color(Color::from_argb(fade_alpha, 255, 255, 255));

                        let blur_sigma = lyric_transition * 12.0 * global_scale;
                        if blur_sigma > 0.1 {
                            text_paint.set_image_filter(image_filters::blur(
                                (blur_sigma, 0.0),
                                None,
                                None,
                                None,
                            ));
                        }

                        let text_y = offset_y + current_h / 2.0 + 4.0 * global_scale
                            - (10.0 * global_scale * lyric_transition);
                        draw_text_cached(
                            canvas,
                            old_lyric,
                            (text_x, text_y),
                            lyric_font_sz,
                            skia_safe::FontStyle::normal(),
                            &text_paint,
                            text_centered,
                            text_max_w,
                        );
                    }

                    if !current_lyric.is_empty() {
                        let mut text_paint = Paint::default();
                        text_paint.set_anti_alias(true);
                        let fade_alpha = (alpha as f32 * lyric_transition) as u8;
                        text_paint.set_color(Color::from_argb(fade_alpha, 255, 255, 255));

                        let blur_sigma = (1.0 - lyric_transition) * 12.0 * global_scale;
                        if blur_sigma > 0.1 {
                            text_paint.set_image_filter(image_filters::blur(
                                (blur_sigma, 0.0),
                                None,
                                None,
                                None,
                            ));
                        }

                        let text_y = offset_y
                            + current_h / 2.0
                            + 4.0 * global_scale
                            + (10.0 * global_scale * (1.0 - lyric_transition));
                        draw_text_cached(
                            canvas,
                            current_lyric,
                            (text_x, text_y),
                            lyric_font_sz,
                            skia_safe::FontStyle::normal(),
                            &text_paint,
                            text_centered,
                            text_max_w,
                        );
                    }
                } else {
                    let text_y = offset_y + current_h / 2.0 + 4.0 * global_scale;
                    if lyric_transition < 0.5 && !old_lyric.is_empty() {
                        let mut text_paint = Paint::default();
                        text_paint.set_anti_alias(true);
                        let progress = lyric_transition * 2.0;
                        let fade_alpha = (alpha as f32 * (1.0 - progress)) as u8;
                        text_paint.set_color(Color::from_argb(fade_alpha, 255, 255, 255));
                        draw_text_cached(
                            canvas,
                            old_lyric,
                            (text_x, text_y),
                            lyric_font_sz,
                            skia_safe::FontStyle::normal(),
                            &text_paint,
                            text_centered,
                            text_max_w,
                        );
                    } else if lyric_transition >= 0.5 && !current_lyric.is_empty() {
                        let mut text_paint = Paint::default();
                        text_paint.set_anti_alias(true);
                        let progress = (lyric_transition - 0.5) * 2.0;
                        let fade_alpha = (alpha as f32 * progress) as u8;
                        text_paint.set_color(Color::from_argb(fade_alpha, 255, 255, 255));
                        draw_text_cached(
                            canvas,
                            current_lyric,
                            (text_x, text_y),
                            lyric_font_sz,
                            skia_safe::FontStyle::normal(),
                            &text_paint,
                            text_centered,
                            text_max_w,
                        );
                    }
                }
                canvas.restore();
            }
        }
    }
    canvas.restore();
    let info = skia_safe::ImageInfo::new(
        skia_safe::ISize::new(os_w as i32, os_h as i32),
        skia_safe::ColorType::BGRA8888,
        skia_safe::AlphaType::Premul,
        None,
    );
    let dst_row_bytes = (os_w * 4) as usize;
    let u8_buffer: &mut [u8] = bytemuck::cast_slice_mut(&mut *buffer);
    let _ = sk_surface.read_pixels(&info, u8_buffer, dst_row_bytes, (0, 0));
    buffer.present().unwrap();
}

fn draw_claude_pill(
    canvas: &skia_safe::Canvas,
    offset_x: f32,
    offset_y: f32,
    current_w: f32,
    current_h: f32,
    alpha: u8,
    global_scale: f32,
    claude_state: &ClaudeState,
    current_text: &str,
    old_text: &str,
    text_transition: f32,
    claude_waiting_visual: bool,
    claude_pulse_time: f32,
    use_blur: bool,
) {
    let pulse_x = offset_x + 12.0 * global_scale;
    let pulse_y = offset_y + current_h / 2.0;
    let base_radius = 5.0 * global_scale;

    let time = claude_pulse_time;

    let pulse_phase = if claude_waiting_visual {
        ((time * 3.2).sin() * 0.5 + 0.5).clamp(0.0, 1.0)
    } else {
        0.0
    };
    log_claude_pulse(format!(
        "state={} pulsing={} phase={:.2} alpha={} text={}",
        claude_state_label(claude_state),
        claude_waiting_visual,
        pulse_phase,
        alpha,
        if current_text.is_empty() {
            "<fallback>"
        } else {
            current_text
        }
    ));
    if claude_waiting_visual {
        log_claude_pulse_bucket(
            (claude_pulse_time * 1000.0) as u64,
            format!(
                "phase={:.2} scale={:.2}",
                pulse_phase,
                0.92 + 0.14 * pulse_phase
            ),
        );
    }
    let pulse_scale = if claude_waiting_visual {
        0.92 + 0.14 * pulse_phase
    } else {
        1.0
    };

    let mut pulse_paint = Paint::default();
    pulse_paint.set_anti_alias(true);

    let pulse_color = match claude_state {
        ClaudeState::Idle => Color::from_argb((alpha as f32 * 0.5) as u8, 150, 150, 150),
        ClaudeState::Thinking { .. } => Color::from_argb(alpha, 147, 130, 255),
        ClaudeState::Working { .. } => Color::from_argb(alpha, 147, 130, 255),
        ClaudeState::Done { .. } => Color::from_argb(alpha, 74, 222, 128),
        ClaudeState::Attention { .. } => Color::from_argb(alpha, 255, 159, 50),
        ClaudeState::ApprovalRequired { .. } => Color::from_argb(alpha, 255, 159, 50),
    };

    let glow_alpha = if claude_waiting_visual {
        (alpha as f32 * (0.08 + 0.18 * pulse_phase)) as u8
    } else {
        0
    };
    let mut glow_paint = Paint::default();
    glow_paint.set_anti_alias(true);
    glow_paint.set_color(Color::from_argb(
        glow_alpha,
        pulse_color.r(),
        pulse_color.g(),
        pulse_color.b(),
    ));
    if glow_alpha > 0 {
        canvas.draw_circle(
            (pulse_x, pulse_y),
            base_radius * (1.25 + 0.45 * pulse_phase),
            &glow_paint,
        );
    }

    if claude_waiting_visual {
        let ring_alpha = (alpha as f32 * (0.08 + 0.14 * (1.0 - pulse_phase))) as u8;
        let mut ring_paint = Paint::default();
        ring_paint.set_anti_alias(true);
        ring_paint.set_style(skia_safe::paint::Style::Stroke);
        ring_paint.set_stroke_width((0.7 + 0.25 * (1.0 - pulse_phase)) * global_scale);
        ring_paint.set_color(Color::from_argb(
            ring_alpha,
            pulse_color.r(),
            pulse_color.g(),
            pulse_color.b(),
        ));
        canvas.draw_circle(
            (pulse_x, pulse_y),
            base_radius * (1.0 + 0.35 * pulse_phase),
            &ring_paint,
        );
    }

    let core_alpha = if claude_waiting_visual {
        (alpha as f32 * (0.84 + 0.12 * pulse_phase)) as u8
    } else {
        alpha
    };
    pulse_paint.set_color(Color::from_argb(
        core_alpha,
        pulse_color.r(),
        pulse_color.g(),
        pulse_color.b(),
    ));
    canvas.draw_circle((pulse_x, pulse_y), base_radius * pulse_scale, &pulse_paint);

    let text = if !current_text.is_empty() {
        current_text.to_string()
    } else {
        match claude_state {
            ClaudeState::Idle => "Idle".to_string(),
            ClaudeState::Thinking { word, .. } => word.clone(),
            ClaudeState::Working { project } => project.clone(),
            ClaudeState::Done { message, .. } => message.clone(),
            ClaudeState::Attention { message, .. } => format!("⚠ {}", message),
            ClaudeState::ApprovalRequired { request, .. } => format!("⚠ {}", request.title),
        }
    };

    if !text.is_empty() {
        let text_size = 11.0 * global_scale;
        let text_x = offset_x + 24.0 * global_scale;
        let text_y = offset_y + current_h / 2.0 + 4.0 * global_scale;
        let max_width = current_w - 30.0 * global_scale;

        if !old_text.is_empty() && text_transition < 1.0 {
            canvas.save();
            let clip_rect = Rect::from_xywh(text_x, offset_y, max_width, current_h);
            canvas.clip_rect(clip_rect, ClipOp::Intersect, true);

            if use_blur {
                let mut old_paint = Paint::default();
                old_paint.set_anti_alias(true);
                old_paint.set_color(Color::from_argb(
                    (alpha as f32 * (1.0 - text_transition)) as u8,
                    255,
                    255,
                    255,
                ));
                let old_blur_sigma = text_transition * 12.0 * global_scale;
                if old_blur_sigma > 0.1 {
                    old_paint.set_image_filter(image_filters::blur(
                        (old_blur_sigma, 0.0),
                        None,
                        None,
                        None,
                    ));
                }
                draw_text_cached(
                    canvas,
                    old_text,
                    (text_x, text_y - (10.0 * global_scale * text_transition)),
                    text_size,
                    skia_safe::FontStyle::normal(),
                    &old_paint,
                    false,
                    max_width,
                );

                let mut current_paint = Paint::default();
                current_paint.set_anti_alias(true);
                current_paint.set_color(Color::from_argb(
                    (alpha as f32 * text_transition) as u8,
                    255,
                    255,
                    255,
                ));
                let current_blur_sigma = (1.0 - text_transition) * 12.0 * global_scale;
                if current_blur_sigma > 0.1 {
                    current_paint.set_image_filter(image_filters::blur(
                        (current_blur_sigma, 0.0),
                        None,
                        None,
                        None,
                    ));
                }
                draw_text_cached(
                    canvas,
                    &text,
                    (
                        text_x,
                        text_y + (10.0 * global_scale * (1.0 - text_transition)),
                    ),
                    text_size,
                    skia_safe::FontStyle::normal(),
                    &current_paint,
                    false,
                    max_width,
                );
            } else {
                let mut old_paint = Paint::default();
                old_paint.set_anti_alias(true);
                old_paint.set_color(Color::from_argb(
                    (alpha as f32 * (1.0 - text_transition)) as u8,
                    255,
                    255,
                    255,
                ));
                draw_text_cached(
                    canvas,
                    old_text,
                    (text_x, text_y - (10.0 * global_scale * text_transition)),
                    text_size,
                    skia_safe::FontStyle::normal(),
                    &old_paint,
                    false,
                    max_width,
                );

                let mut current_paint = Paint::default();
                current_paint.set_anti_alias(true);
                current_paint.set_color(Color::from_argb(
                    (alpha as f32 * text_transition) as u8,
                    255,
                    255,
                    255,
                ));
                draw_text_cached(
                    canvas,
                    &text,
                    (
                        text_x,
                        text_y + (10.0 * global_scale * (1.0 - text_transition)),
                    ),
                    text_size,
                    skia_safe::FontStyle::normal(),
                    &current_paint,
                    false,
                    max_width,
                );
            }

            canvas.restore();
        } else {
            let mut text_paint = Paint::default();
            text_paint.set_color(Color::from_argb(alpha, 255, 255, 255));
            text_paint.set_anti_alias(true);
            draw_text_cached(
                canvas,
                &text,
                (text_x, text_y),
                text_size,
                skia_safe::FontStyle::normal(),
                &text_paint,
                false,
                max_width,
            );
        }
    }
}
