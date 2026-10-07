use crate::core::audio::AudioProcessor;
use crate::core::claude_watcher::{ClaudeActionKind, ClaudeSessionSummary, ClaudeWatcher};
use crate::core::config::{AppConfig, PADDING, TOP_OFFSET, WINDOW_TITLE};
use crate::core::hook_server::{ApprovalAction, ApprovalRequest, ClaudeState, HookServer};
use crate::core::persistence::load_config;
use crate::core::render::draw_island;
use crate::core::smtc::SmtcListener;
use crate::ui::expanded::main_view::{
    get_next_btn_rect, get_pause_btn_rect, get_prev_btn_rect, get_progress_bar_rect,
    set_progress_hover, trigger_cover_flip, trigger_next_click, trigger_pause_click,
    trigger_prev_click,
};
use crate::ui::expanded::widget_view::{
    approval_scroll_limit, choice_button_rects, detail_scroll_limit, permission_button_rects,
};
use crate::utils::blur::calculate_blur_sigmas;
use crate::utils::color::get_island_border_weights;
use crate::utils::font::FontManager;
use crate::utils::glass::set_glass_hwnd;
use crate::utils::icon::get_app_icon;
use crate::utils::mouse::{get_global_cursor_pos, is_left_button_pressed, is_point_in_rect};
use crate::utils::physics::Spring;
use crate::window::tray::{TrayAction, TrayManager};
use softbuffer::{Context, Surface};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, CreateRoundRectRgn, HRGN, SetWindowRgn};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_NOACTIVATE, SetWindowLongPtrW,
    SetWindowPos, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX, WS_THICKFRAME,
};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowButtons, WindowId, WindowLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClaudeDisplayStage {
    Ready,
    Running,
    IdleOrOther,
}

#[derive(Debug, Clone)]
pub struct ClaudeDisplayApproval {
    pub request: ApprovalRequest,
    pub pending_action: Option<ApprovalAction>,
}

#[derive(Debug, Clone)]
pub struct ClaudeDisplayEntry {
    pub session_id: String,
    pub project: String,
    pub preview_text: String,
    pub full_text: String,
    pub stage: ClaudeDisplayStage,
    pub last_active_at: u64,
    pub approval: Option<ClaudeDisplayApproval>,
}

#[derive(Debug, Clone)]
pub struct ClaudePermissionActionButton {
    pub label: String,
    pub action: ApprovalAction,
}

#[derive(Debug, Clone)]
pub struct ClaudeChoiceButton {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub enum ClaudeForegroundActionKind {
    PermissionApproval {
        actions: Vec<ClaudePermissionActionButton>,
    },
    ChoicePrompt {
        options: Vec<ClaudeChoiceButton>,
    },
}

#[derive(Debug, Clone)]
pub struct ClaudeForegroundApproval {
    pub session_id: String,
    pub project: String,
    pub reason_title: String,
    pub waiting_for: String,
    pub status: String,
    pub pid: u32,
    pub last_active_at: u64,
    pub detail_text: String,
    pub kind: ClaudeForegroundActionKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct VisibleIslandRegion {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct HiddenHandleRegion {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn log_window_sync(signature: String) {
    thread_local! {
        static LAST_WINDOW_SYNC_LOG: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }

    LAST_WINDOW_SYNC_LOG.with(|cell| {
        let mut last = cell.borrow_mut();
        if last.as_ref() != Some(&signature) {
            println!("WINDOW_SYNC {}", signature);
            *last = Some(signature);
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum NativeHitRegion {
    Visible(VisibleIslandRegion),
    HiddenHandle(HiddenHandleRegion),
    Disabled,
}

const VISIBLE_HIT_PADDING: f64 = 8.0;
const COLLAPSED_ENVELOPE_SYNC_THRESHOLD: f32 = 2.0;

pub struct App {
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    tray: Option<TrayManager>,
    smtc: SmtcListener,
    audio: AudioProcessor,
    config: AppConfig,
    expanded: bool,
    widget_view: bool,
    visible: bool,
    border_weights: [f32; 4],
    target_border_weights: [f32; 4],
    spring_w: Spring,
    spring_h: Spring,
    spring_r: Spring,
    spring_view: Spring,
    os_w: u32,
    os_h: u32,
    win_x: i32,
    win_y: i32,
    native_window_expanded: bool,
    pending_collapse_resize: bool,
    frame_count: u64,
    last_media_title: String,
    last_media_playing: bool,
    last_playing_time: Instant,
    current_lyric_text: String,
    old_lyric_text: String,
    lyric_transition: f32,
    current_claude_text: String,
    old_claude_text: String,
    claude_transition: f32,
    claude_waiting_visual: bool,
    claude_pulse_time: f32,
    selected_claude_session_id: Option<String>,
    claude_detail_scroll: f32,
    had_foreground_approval: bool,
    foreground_approval_request_id: Option<String>,
    stable_claude_session_order: Vec<String>,
    idle_timer: Instant,
    spring_hide: Spring,
    auto_hidden: bool,
    is_dragging: bool,
    drag_start_py: i32,
    drag_start_hide_val: f32,
    manually_hidden: bool,
    drag_has_moved: bool,
    last_frame_time: Instant,
    last_mon_size: (u32, u32),
    last_mon_pos: (i32, i32),
    lyric_scroll_offset: f32,
    lyric_scroll_pause: f32,
    seeking_progress: bool,
    seeking_bar_left: f32,
    seeking_bar_right: f32,
    seeking_duration_ms: u64,
    hook_server: HookServer,
    claude_watcher: ClaudeWatcher,
    thinking_started_at: Option<std::time::Instant>,
    thinking_baseline_content: String,
    last_claude_merge_log: Option<String>,
    last_claude_redraw_log: Option<String>,
    last_native_hit_region: NativeHitRegion,
}

impl Default for App {
    fn default() -> Self {
        let config = load_config();
        Self {
            window: None,
            surface: None,
            tray: None,
            config: config.clone(),
            expanded: false,
            widget_view: false,
            visible: true,
            border_weights: [0.0; 4],
            target_border_weights: [0.0; 4],
            spring_w: Spring::new(config.base_width * config.global_scale),
            spring_h: Spring::new(config.base_height * config.global_scale),
            spring_r: Spring::new((config.base_height * config.global_scale) / 2.0),
            spring_view: Spring::new(0.0),
            smtc: SmtcListener::new(
                config.lyrics_source.clone(),
                config.lyrics_fallback,
                config.smtc_apps.clone(),
            ),
            audio: AudioProcessor::new(),
            os_w: 0,
            os_h: 0,
            win_x: 0,
            win_y: 0,
            native_window_expanded: false,
            pending_collapse_resize: false,
            frame_count: 0,
            last_media_title: String::new(),
            last_media_playing: false,
            last_playing_time: Instant::now(),
            current_lyric_text: String::new(),
            old_lyric_text: String::new(),
            lyric_transition: 1.0,
            current_claude_text: String::new(),
            old_claude_text: String::new(),
            claude_transition: 1.0,
            claude_waiting_visual: false,
            claude_pulse_time: 0.0,
            selected_claude_session_id: None,
            claude_detail_scroll: 0.0,
            had_foreground_approval: false,
            foreground_approval_request_id: None,
            stable_claude_session_order: Vec::new(),
            idle_timer: Instant::now(),
            spring_hide: Spring::new(0.0),
            auto_hidden: false,
            is_dragging: false,
            drag_start_py: 0,
            drag_start_hide_val: 0.0,
            manually_hidden: false,
            drag_has_moved: false,
            last_frame_time: Instant::now(),
            last_mon_size: (0, 0),
            last_mon_pos: (0, 0),
            lyric_scroll_offset: 0.0,
            lyric_scroll_pause: 0.0,
            seeking_progress: false,
            seeking_bar_left: 0.0,
            seeking_bar_right: 0.0,
            seeking_duration_ms: 0,
            hook_server: HookServer::new(),
            claude_watcher: ClaudeWatcher::new(),
            thinking_started_at: None,
            thinking_baseline_content: String::new(),
            last_claude_merge_log: None,
            last_claude_redraw_log: None,
            last_native_hit_region: NativeHitRegion::Disabled,
        }
    }
}

impl App {
    fn collapsed_base_width(&mut self, music_active: bool) -> f32 {
        let is_currently_hidden =
            self.auto_hidden || self.manually_hidden || self.spring_hide.value > 0.1;
        let on_collapsed_page_2 = !self.expanded && self.widget_view;
        if !self.expanded && !is_currently_hidden && on_collapsed_page_2 {
            self.lyric_scroll_offset = 0.0;
            let text = if !self.current_claude_text.is_empty() {
                &self.current_claude_text
            } else {
                &self.old_claude_text
            };
            let measured = FontManager::global().measure_text_cached(
                text,
                11.0 * self.config.global_scale,
                skia_safe::FontStyle::normal(),
            );
            let min_w = self.config.base_width + 35.0;
            let max_w = 320.0 * self.config.global_scale;
            return (measured / self.config.global_scale + 44.0).clamp(min_w, max_w);
        }

        if music_active && !self.expanded && !is_currently_hidden {
            let has_visible_lyrics = self.config.show_lyrics
                && (!self.current_lyric_text.is_empty()
                    || (!self.old_lyric_text.is_empty() && self.lyric_transition < 1.0));

            if has_visible_lyrics {
                let display_text = if !self.current_lyric_text.is_empty() {
                    &self.current_lyric_text
                } else {
                    &self.old_lyric_text
                };
                let mut text_w: f32 = 0.0;
                for c in display_text.chars() {
                    if c.is_ascii() {
                        text_w += 7.5;
                    } else {
                        text_w += 13.5;
                    }
                }

                if self.config.lyrics_scroll {
                    let natural_w = 60.0 + text_w;
                    let max_w = self.config.lyrics_scroll_max_width;
                    if natural_w > max_w {
                        let fixed_w = max_w;
                        let available_text_w = (fixed_w - 59.0) * self.config.global_scale;
                        let full_text_w = text_w * self.config.global_scale;
                        let overflow = full_text_w - available_text_w;
                        if overflow <= 0.0 || self.lyric_transition < 1.0 {
                            self.lyric_scroll_offset = 0.0;
                        }
                        return fixed_w;
                    }
                    self.lyric_scroll_offset = 0.0;
                    let min_w = self.config.base_width + 35.0;
                    return natural_w.clamp(min_w, max_w);
                }

                self.lyric_scroll_offset = 0.0;
                let min_w = self.config.base_width + 35.0;
                let w: f32 = 60.0 + text_w;
                return w.clamp(min_w, 450.0);
            }

            return self.config.base_width + 35.0;
        }

        self.lyric_scroll_offset = 0.0;
        self.config.base_width
    }

    fn collapsed_visual_bounds(&mut self, music_active: bool) -> (f32, f32) {
        let width = self.collapsed_base_width(music_active);
        let height = self.config.base_height;
        (width, height)
    }

    fn collapsed_envelope_bounds(&mut self, music_active: bool) -> (f32, f32) {
        let (width, height) = self.collapsed_visual_bounds(music_active);
        let top_inset = PADDING / (2.0 * self.config.global_scale);
        let side_inset = VISIBLE_HIT_PADDING as f32 * 2.0;
        (width + side_inset, height + top_inset)
    }

    fn expanded_envelope_bounds(&self) -> (f32, f32) {
        let width = self.config.expanded_width;
        let top_inset = PADDING / (2.0 * self.config.global_scale);
        (width, self.config.expanded_height + top_inset)
    }

    fn collapsed_interaction_width(&mut self, music_active: bool) -> f32 {
        self.collapsed_envelope_bounds(music_active).0
    }

    fn collapsed_interaction_height(&mut self, music_active: bool) -> f32 {
        self.collapsed_envelope_bounds(music_active).1
    }

    fn target_dimensions(&mut self, music_active: bool) -> (f32, f32, f32, f32, f32) {
        let target_base_w = self.collapsed_base_width(music_active);
        let target_w = (if self.expanded {
            self.config.expanded_width
        } else {
            target_base_w
        }) * self.config.global_scale;
        let target_h = (if self.expanded {
            self.config.expanded_height
        } else {
            self.config.base_height
        }) * self.config.global_scale;
        let target_r = if self.expanded {
            32.0 * self.config.global_scale
        } else {
            (self.config.base_height * self.config.global_scale) / 2.0
        };
        let target_view = if self.widget_view { 1.0 } else { 0.0 };
        (target_base_w, target_w, target_h, target_r, target_view)
    }

    fn window_envelope_size(
        &mut self,
        expanded: bool,
        collapsed_base_w: f32,
        music_active: bool,
    ) -> (u32, u32) {
        let (logical_w, logical_h) = if expanded {
            self.expanded_envelope_bounds()
        } else {
            (
                collapsed_base_w,
                self.collapsed_interaction_height(music_active),
            )
        };
        (
            (logical_w * self.config.global_scale).round() as u32,
            (logical_h * self.config.global_scale).round() as u32,
        )
    }

    fn current_island_frame(&self) -> (f64, f64, f64, f64, f64) {
        let island_y = PADDING as f64 / 2.0;
        let offset_x = (self.os_w as f64 - self.spring_w.value as f64) / 2.0;
        let hidden_peek_h = (5.0 * self.config.global_scale as f64).max(3.0);
        let hide_distance =
            (self.spring_h.value as f64 - hidden_peek_h + TOP_OFFSET as f64).max(0.0);
        let hide_y_offset = self.spring_hide.value as f64 * hide_distance;
        let current_island_y = island_y - hide_y_offset;
        (
            offset_x,
            current_island_y,
            self.spring_w.value as f64,
            self.spring_h.value as f64,
            hidden_peek_h,
        )
    }

    fn current_native_hit_region(&self) -> NativeHitRegion {
        let (offset_x, current_island_y, island_w, island_h, hidden_peek_h) =
            self.current_island_frame();

        if self.expanded || (!self.auto_hidden && !self.manually_hidden) {
            let padding = VISIBLE_HIT_PADDING * self.config.global_scale as f64;
            return NativeHitRegion::Visible(VisibleIslandRegion {
                x: (offset_x - padding).round() as i32,
                y: (current_island_y - padding).round() as i32,
                width: (island_w + padding * 2.0).round() as i32,
                height: (island_h + padding * 2.0).round() as i32,
                radius: (self.spring_r.value as f64 + padding).round().max(1.0) as i32,
            });
        }

        let hidden_handle_h = (24.0 * self.config.global_scale as f64).max(14.0);
        let hidden_handle_y =
            (current_island_y + island_h - hidden_peek_h - hidden_handle_h * 0.35).max(0.0);

        NativeHitRegion::HiddenHandle(HiddenHandleRegion {
            x: offset_x.round() as i32,
            y: hidden_handle_y.round() as i32,
            width: island_w.round() as i32,
            height: hidden_handle_h.round().max(1.0) as i32,
        })
    }

    fn apply_native_hit_region(&mut self, window: &Window) {
        let region = self.current_native_hit_region();
        if region == self.last_native_hit_region {
            return;
        }

        let recent_music_active = self.last_media_playing
            || self.last_playing_time.elapsed() < Duration::from_secs(5);
        let collapsed_shell_width = if self.expanded {
            0.0
        } else {
            self.collapsed_interaction_width(recent_music_active)
        };
        let region_signature = match region {
            NativeHitRegion::Visible(visible) => format!(
                "apply visible render_w={:.1} render_h={:.1} os_w={} os_h={} region_x={} region_y={} region_w={} region_h={} radius={} expanded={} pending={} music_shell={:.1}",
                self.spring_w.value,
                self.spring_h.value,
                self.os_w,
                self.os_h,
                visible.x,
                visible.y,
                visible.width,
                visible.height,
                visible.radius,
                self.expanded,
                self.pending_collapse_resize,
                collapsed_shell_width,
            ),
            NativeHitRegion::HiddenHandle(handle) => format!(
                "apply hidden render_w={:.1} render_h={:.1} os_w={} os_h={} handle_x={} handle_y={} handle_w={} handle_h={} expanded={} pending={}",
                self.spring_w.value,
                self.spring_h.value,
                self.os_w,
                self.os_h,
                handle.x,
                handle.y,
                handle.width,
                handle.height,
                self.expanded,
                self.pending_collapse_resize,
            ),
            NativeHitRegion::Disabled => format!(
                "apply disabled render_w={:.1} render_h={:.1} os_w={} os_h={} expanded={} pending={}",
                self.spring_w.value,
                self.spring_h.value,
                self.os_w,
                self.os_h,
                self.expanded,
                self.pending_collapse_resize,
            ),
        };
        log_window_sync(region_signature);

        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                let hwnd = HWND(win32_handle.hwnd.get() as _);
                unsafe {
                    match region {
                        NativeHitRegion::Visible(visible) => {
                            let diameter = (visible.radius * 2).max(1);
                            let hrgn = CreateRoundRectRgn(
                                visible.x,
                                visible.y,
                                visible.x + visible.width,
                                visible.y + visible.height,
                                diameter,
                                diameter,
                            );
                            let _ = SetWindowRgn(hwnd, hrgn, true);
                        }
                        NativeHitRegion::HiddenHandle(handle) => {
                            let hrgn = CreateRectRgn(
                                handle.x,
                                handle.y,
                                handle.x + handle.width,
                                handle.y + handle.height,
                            );
                            let _ = SetWindowRgn(hwnd, hrgn, true);
                        }
                        NativeHitRegion::Disabled => {
                            let _ = SetWindowRgn(hwnd, HRGN::default(), true);
                        }
                    }
                }
            }
        }

        self.last_native_hit_region = region;
    }

    fn reset_native_hit_region(&mut self) {
        self.last_native_hit_region = NativeHitRegion::Disabled;
    }

    fn centered_window_position(
        &self,
        monitor: &winit::monitor::MonitorHandle,
        os_w: u32,
        _os_h: u32,
    ) -> (i32, i32) {
        let mon_size = monitor.size();
        let mon_pos = monitor.position();
        let center_x = mon_pos.x + (mon_size.width as i32) / 2;
        let top_y = mon_pos.y + TOP_OFFSET;
        let island_top_inset = (PADDING / 2.0) as i32;
        let desired_island_top = top_y + self.config.position_y_offset;
        (
            center_x - (os_w as i32) / 2 + self.config.position_x_offset,
            desired_island_top - island_top_inset,
        )
    }

    fn apply_native_window_size(
        &mut self,
        window: &Window,
        expanded: bool,
        collapsed_base_w: f32,
        music_active: bool,
    ) {
        let (next_os_w, next_os_h) =
            self.window_envelope_size(expanded, collapsed_base_w, music_active);
        log_window_sync(format!(
            "size target expanded={} music_active={} collapsed_base_w={:.1} next_os_w={} next_os_h={} prev_os_w={} prev_os_h={} render_w={:.1} render_h={:.1}",
            expanded,
            music_active,
            collapsed_base_w,
            next_os_w,
            next_os_h,
            self.os_w,
            self.os_h,
            self.spring_w.value,
            self.spring_h.value,
        ));
        let monitor = Self::get_target_monitor(window, self.config.monitor_index)
            .or_else(|| window.primary_monitor())
            .or_else(|| window.current_monitor());

        if let Some(monitor) = monitor.as_ref() {
            let mon_size = monitor.size();
            let mon_pos = monitor.position();
            self.last_mon_size = (mon_size.width, mon_size.height);
            self.last_mon_pos = (mon_pos.x, mon_pos.y);
            let (next_x, next_y) = self.centered_window_position(monitor, next_os_w, next_os_h);
            self.win_x = next_x;
            self.win_y = next_y;
        }

        let size_changed = self.os_w != next_os_w || self.os_h != next_os_h;
        self.os_w = next_os_w;
        self.os_h = next_os_h;
        self.native_window_expanded = expanded;

        if size_changed {
            let _ = window.request_inner_size(PhysicalSize::new(self.os_w, self.os_h));
            if let Some(surface) = self.surface.as_mut() {
                let _ = surface.resize(
                    std::num::NonZeroU32::new(self.os_w).unwrap(),
                    std::num::NonZeroU32::new(self.os_h).unwrap(),
                );
            }
        }

        window.set_outer_position(PhysicalPosition::new(self.win_x, self.win_y));
        Self::enforce_topmost(window, self.win_x, self.win_y, self.os_w, self.os_h);
        window.request_redraw();
    }

    fn queue_window_mode_sync(&mut self, window: &Window, music_active: bool) {
        let collapsed_interaction_w = self.collapsed_interaction_width(music_active);
        if self.expanded {
            self.pending_collapse_resize = false;
            if !self.native_window_expanded {
                self.apply_native_window_size(window, true, collapsed_interaction_w, music_active);
            }
        } else if self.native_window_expanded {
            self.pending_collapse_resize = true;
        } else {
            self.apply_native_window_size(window, false, collapsed_interaction_w, music_active);
        }
    }

    fn finalize_pending_collapse_resize(&mut self, window: &Window, music_active: bool) {
        if !self.pending_collapse_resize || self.expanded {
            return;
        }
        let (_, target_w, target_h, _, _) = self.target_dimensions(music_active);
        let collapsed_interaction_w = self.collapsed_interaction_width(music_active);
        let settled_w =
            (self.spring_w.value - target_w).abs() < 2.0 && self.spring_w.velocity.abs() < 0.02;
        let settled_h =
            (self.spring_h.value - target_h).abs() < 2.0 && self.spring_h.velocity.abs() < 0.02;
        if settled_w && settled_h {
            self.pending_collapse_resize = false;
            self.apply_native_window_size(window, false, collapsed_interaction_w, music_active);
        }
    }

    fn sync_collapsed_window_envelope_if_needed(&mut self, window: &Window, music_active: bool) {
        if self.expanded {
            return;
        }

        let collapsed_interaction_w = self.collapsed_interaction_width(music_active);
        let side_inset = VISIBLE_HIT_PADDING as f32 * 2.0;
        let spring_based_w = self.spring_w.value / self.config.global_scale + side_inset;
        let effective_w = collapsed_interaction_w.max(spring_based_w);
        let target_width = effective_w * self.config.global_scale;

        let needs_grow = target_width > self.os_w as f32 + COLLAPSED_ENVELOPE_SYNC_THRESHOLD;
        let needs_shrink = self.os_w as f32 > target_width + COLLAPSED_ENVELOPE_SYNC_THRESHOLD;

        if needs_grow {
            log_window_sync(format!(
                "collapsed_sync_grow music_active={} effective_w={:.1} os_w={} render_w={:.1}",
                music_active, target_width, self.os_w, self.spring_w.value,
            ));
            self.apply_native_window_size(window, false, effective_w, music_active);
        } else if needs_shrink
            && !self.pending_collapse_resize
            && !self.native_window_expanded
        {
            log_window_sync(format!(
                "collapsed_sync_shrink music_active={} effective_w={:.1} os_w={} render_w={:.1}",
                music_active, target_width, self.os_w, self.spring_w.value,
            ));
            self.apply_native_window_size(window, false, effective_w, music_active);
        }
    }

    fn claude_display_text(state: &ClaudeState) -> String {
        match state {
            ClaudeState::Idle => "Idle".to_string(),
            ClaudeState::Thinking { word, .. } => word.clone(),
            ClaudeState::Working { project } => project.clone(),
            ClaudeState::Done { message, .. } => message.clone(),
            ClaudeState::Attention { message, .. } => format!("⚠ {}", message),
            ClaudeState::ApprovalRequired { request, .. } => format!("⚠ {}", request.title),
        }
    }

    fn update_claude_waiting_visual(&mut self, state: &ClaudeState) {
        match state {
            ClaudeState::Thinking { .. } | ClaudeState::Working { .. } => {
                self.claude_waiting_visual = true;
            }
            ClaudeState::Done { .. }
            | ClaudeState::Attention { .. }
            | ClaudeState::ApprovalRequired { .. }
            | ClaudeState::Idle => {
                self.claude_waiting_visual = false;
            }
        }
    }

    fn update_collapsed_claude_transition(&mut self, claude_state: &ClaudeState, window: &Window) {
        let next_text = Self::claude_display_text(claude_state);
        if next_text != self.current_claude_text {
            self.old_claude_text = self.current_claude_text.clone();
            self.current_claude_text = next_text;
            self.claude_transition = 0.0;
            window.request_redraw();
        }
    }

    fn enforce_topmost(window: &Window, x: i32, y: i32, w: u32, h: u32) {
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                unsafe {
                    let _ = SetWindowPos(
                        HWND(win32_handle.hwnd.get() as _),
                        HWND_TOPMOST,
                        x,
                        y,
                        w as i32,
                        h as i32,
                        SWP_NOACTIVATE,
                    );
                }
            }
        }
    }
}

fn choice_prompt_response_payload(selected: &str) -> serde_json::Value {
    serde_json::json!({
        "selected": selected,
    })
}

fn should_hide_hook_wrapper_error_line(line: &str) -> bool {
    let normalized = line.trim();
    normalized.contains("cygpath: can't convert empty path")
        || normalized.contains("/hooks/pre-tool-use.sh: No such file or directory")
        || normalized.contains(r"\hooks\pre-tool-use.sh: No such file or directory")
}

fn should_hide_command_line(command: &str) -> bool {
    let normalized = command.trim();
    should_hide_hook_wrapper_error_line(normalized)
        || normalized == "/hooks/pre-tool-use.sh"
        || normalized == r"\hooks\pre-tool-use.sh"
}

fn filter_hook_wrapper_error_detail(detail_text: &str) -> String {
    detail_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !should_hide_hook_wrapper_error_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_foreground_action(
    watcher_snapshot: &crate::core::claude_watcher::ClaudeSessionsSnapshot,
    hook_snapshot: &crate::core::hook_server::ClaudeSessionStates,
) -> Option<ClaudeForegroundApproval> {
    let watcher_result = watcher_snapshot
        .sessions
        .iter()
        .filter_map(|session| {
            let approval_wait = session.approval_wait.as_ref()?;
            if approval_wait.status != "waiting" || approval_wait.waiting_for.trim().is_empty() {
                return None;
            }
            let hook_meta = hook_snapshot.per_session_meta.get(&session.session_id);

            let reason_title = approval_wait.detail_title.trim().to_string();
            let (detail_text, kind) = match approval_wait.action_kind {
                ClaudeActionKind::ChoicePrompt => {
                    let mut detail_text = approval_wait.detail_text.trim().to_string();
                    if detail_text == reason_title {
                        detail_text.clear();
                    } else if let Some(rest) = detail_text.strip_prefix(&reason_title) {
                        let trimmed = rest.trim_start_matches(|ch: char| ch.is_whitespace());
                        detail_text = trimmed.to_string();
                    }
                    let options = approval_wait
                        .choice_options
                        .iter()
                        .map(|option| ClaudeChoiceButton {
                            label: option.label.clone(),
                            value: option.value.clone(),
                        })
                        .collect();
                    (
                        detail_text,
                        ClaudeForegroundActionKind::ChoicePrompt { options },
                    )
                }
                ClaudeActionKind::PermissionApproval => {
                    let mut detail_text = approval_wait.detail_text.trim().to_string();
                    if detail_text == reason_title {
                        detail_text.clear();
                    }
                    if detail_text.is_empty() {
                        let mut detail_parts = Vec::new();
                        if let Some(detail) = hook_meta
                            .and_then(|meta| meta.approval_detail.as_deref())
                            .filter(|s| !s.trim().is_empty())
                        {
                            let filtered_detail = filter_hook_wrapper_error_detail(detail);
                            if !filtered_detail.is_empty() {
                                detail_parts.push(filtered_detail);
                            }
                        } else if !session.preview_text.trim().is_empty() {
                            detail_parts.push(session.preview_text.trim().to_string());
                        }
                        if let Some(tool_name) = hook_meta
                            .and_then(|meta| meta.approval_tool_name.as_deref())
                            .filter(|s| !s.trim().is_empty())
                        {
                            detail_parts.push(format!("Tool: {}", tool_name.trim()));
                        }
                        if let Some(command) = hook_meta
                            .and_then(|meta| meta.approval_command.as_deref())
                            .filter(|s| !s.trim().is_empty())
                        {
                            if !should_hide_command_line(command) {
                                detail_parts.push(format!("Command: {}", command.trim()));
                            }
                        }
                        if let Some(file_path) = hook_meta
                            .and_then(|meta| meta.approval_file_path.as_deref())
                            .filter(|s| !s.trim().is_empty())
                        {
                            detail_parts.push(format!("Path: {}", file_path.trim()));
                        }
                        detail_text = detail_parts.join("\n");
                    } else {
                        detail_text = filter_hook_wrapper_error_detail(&detail_text);
                    }

                    if detail_text.is_empty() {
                        detail_text = reason_title.clone();
                    }

                    (
                        detail_text,
                        ClaudeForegroundActionKind::PermissionApproval {
                            actions: vec![
                                ClaudePermissionActionButton {
                                    label: "Deny".to_string(),
                                    action: ApprovalAction::Deny,
                                },
                                ClaudePermissionActionButton {
                                    label: "Approve once".to_string(),
                                    action: ApprovalAction::ApproveOnce,
                                },
                                ClaudePermissionActionButton {
                                    label: "Allow for session".to_string(),
                                    action: ApprovalAction::AllowForSession,
                                },
                            ],
                        },
                    )
                }
            };

            Some(ClaudeForegroundApproval {
                session_id: session.session_id.clone(),
                project: session.project.clone(),
                reason_title,
                waiting_for: approval_wait.waiting_for.clone(),
                status: approval_wait.status.clone(),
                pid: approval_wait.pid,
                detail_text,
                kind,
                last_active_at: session.last_active_at,
            })
        })
        .max_by(|a, b| {
            a.last_active_at
                .cmp(&b.last_active_at)
                .then_with(|| a.pid.cmp(&b.pid))
        });

    if watcher_result.is_some() {
        return watcher_result;
    }

    // Fallback: check hook server state for ApprovalRequired sessions
    // that the watcher did not report as "waiting"
    hook_snapshot
        .per_session
        .iter()
        .filter_map(|(session_id, state)| {
            let (project, request) = match state {
                ClaudeState::ApprovalRequired { project, request } => (project, request),
                _ => return None,
            };
            let already_handled = watcher_snapshot.sessions.iter().any(|s| {
                s.session_id == *session_id
                    && s.approval_wait
                        .as_ref()
                        .map(|w| w.status == "waiting")
                        .unwrap_or(false)
            });
            if already_handled {
                return None;
            }

            let meta = hook_snapshot.per_session_meta.get(session_id);
            let mut detail_parts = Vec::new();
            if let Some(detail) = meta
                .and_then(|m| m.approval_detail.as_deref())
                .filter(|s| !s.trim().is_empty())
            {
                let filtered = filter_hook_wrapper_error_detail(detail);
                if !filtered.is_empty() {
                    detail_parts.push(filtered);
                }
            }
            if let Some(tool_name) = meta
                .and_then(|m| m.approval_tool_name.as_deref())
                .filter(|s| !s.trim().is_empty())
            {
                detail_parts.push(format!("Tool: {}", tool_name.trim()));
            }
            if let Some(command) = meta
                .and_then(|m| m.approval_command.as_deref())
                .filter(|s| !s.trim().is_empty())
            {
                if !should_hide_command_line(command) {
                    detail_parts.push(format!("Command: {}", command.trim()));
                }
            }
            if let Some(file_path) = meta
                .and_then(|m| m.approval_file_path.as_deref())
                .filter(|s| !s.trim().is_empty())
            {
                detail_parts.push(format!("Path: {}", file_path.trim()));
            }
            let detail_text = if detail_parts.is_empty() {
                request.title.clone()
            } else {
                detail_parts.join("\n")
            };

            Some(ClaudeForegroundApproval {
                session_id: session_id.clone(),
                project: project.clone(),
                reason_title: request.title.clone(),
                waiting_for: request.title.clone(),
                status: "waiting".to_string(),
                pid: 0,
                last_active_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                detail_text,
                kind: ClaudeForegroundActionKind::PermissionApproval {
                    actions: vec![
                        ClaudePermissionActionButton {
                            label: "Deny".to_string(),
                            action: ApprovalAction::Deny,
                        },
                        ClaudePermissionActionButton {
                            label: "Approve once".to_string(),
                            action: ApprovalAction::ApproveOnce,
                        },
                        ClaudePermissionActionButton {
                            label: "Allow for session".to_string(),
                            action: ApprovalAction::AllowForSession,
                        },
                    ],
                },
            })
        })
        .max_by(|a, b| a.last_active_at.cmp(&b.last_active_at))
}

#[cfg(test)]
fn build_choice_prompt_foreground_action_for_test(
    watcher_snapshot: &crate::core::claude_watcher::ClaudeSessionsSnapshot,
) -> Option<ClaudeForegroundApproval> {
    build_foreground_action(
        watcher_snapshot,
        &crate::core::hook_server::ClaudeSessionStates::default(),
    )
}

impl App {
    fn has_foreground_approval(
        &self,
        watcher_snapshot: &crate::core::claude_watcher::ClaudeSessionsSnapshot,
        hook_snapshot: &crate::core::hook_server::ClaudeSessionStates,
    ) -> bool {
        build_foreground_action(watcher_snapshot, hook_snapshot).is_some()
    }

    fn sync_approval_page_visibility(
        &mut self,
        window: &Window,
        watcher_snapshot: &crate::core::claude_watcher::ClaudeSessionsSnapshot,
        hook_snapshot: &crate::core::hook_server::ClaudeSessionStates,
    ) {
        let has_watcher_approval = watcher_snapshot.sessions.iter().any(|session| {
            session
                .approval_wait
                .as_ref()
                .map(|wait| wait.status == "waiting" && !wait.waiting_for.trim().is_empty())
                .unwrap_or(false)
        });
        let has_hook_approval = hook_snapshot
            .per_session
            .values()
            .any(|state| matches!(state, ClaudeState::ApprovalRequired { .. }));
        let has_approval = has_watcher_approval || has_hook_approval;
        if has_approval && !self.had_foreground_approval {
            self.expanded = true;
            self.widget_view = true;
            self.claude_detail_scroll = 0.0;
            self.queue_window_mode_sync(window, false);
            window.request_redraw();
        }
        self.had_foreground_approval = has_approval;
    }

    fn sync_foreground_approval_scroll(&mut self, approval: Option<&ClaudeForegroundApproval>) {
        let next_request_id = approval.map(|item| item.session_id.clone());
        if next_request_id != self.foreground_approval_request_id {
            self.claude_detail_scroll = 0.0;
            self.foreground_approval_request_id = next_request_id;
        }
    }

    fn submit_permission_action(&mut self, session_id: &str, action: ApprovalAction) {
        if self.write_permission_response(session_id, action).is_ok() {
            self.foreground_approval_request_id = Some(session_id.to_string());
        }
    }

    fn write_permission_response(
        &self,
        session_id: &str,
        action: ApprovalAction,
    ) -> Result<(), String> {
        let response_path = if let Some(pid) = self.claude_watcher.find_session_pid(session_id) {
            std::env::temp_dir().join(format!("winisland_response_{}.json", pid))
        } else {
            std::env::temp_dir().join(format!("winisland_response_{}.json", session_id))
        };
        std::fs::write(&response_path, action.as_response_payload().to_string())
            .map_err(|err| format!("Failed to write {}: {}", response_path.display(), err))
    }

    fn submit_choice_prompt_response(&mut self, session_id: &str, selected: String) {
        if self
            .write_choice_prompt_response(session_id, &selected)
            .is_ok()
        {
            self.foreground_approval_request_id = Some(session_id.to_string());
        }
    }

    fn write_choice_prompt_response(&self, session_id: &str, selected: &str) -> Result<(), String> {
        let response_path = if let Some(pid) = self.claude_watcher.find_session_pid(session_id) {
            std::env::temp_dir().join(format!("winisland_response_{}.json", pid))
        } else {
            std::env::temp_dir().join(format!("winisland_response_{}.json", session_id))
        };
        std::fs::write(
            &response_path,
            choice_prompt_response_payload(selected).to_string(),
        )
        .map_err(|err| format!("Failed to write {}: {}", response_path.display(), err))
    }

    fn stage_rank(stage: ClaudeDisplayStage) -> u8 {
        match stage {
            ClaudeDisplayStage::Ready => 0,
            ClaudeDisplayStage::Running => 1,
            ClaudeDisplayStage::IdleOrOther => 2,
        }
    }

    fn classify_claude_entry(
        session: &ClaudeSessionSummary,
        per_session_state: Option<&ClaudeState>,
    ) -> ClaudeDisplayStage {
        match per_session_state {
            Some(ClaudeState::Done { .. }) => ClaudeDisplayStage::Ready,
            Some(ClaudeState::Thinking { .. } | ClaudeState::Working { .. }) => {
                ClaudeDisplayStage::Running
            }
            Some(ClaudeState::Attention { .. } | ClaudeState::ApprovalRequired { .. }) => {
                ClaudeDisplayStage::IdleOrOther
            }
            Some(ClaudeState::Idle) | None => {
                if !session.full_text.trim().is_empty() {
                    ClaudeDisplayStage::Ready
                } else {
                    ClaudeDisplayStage::IdleOrOther
                }
            }
        }
    }

    fn build_claude_entries(
        &self,
        watcher_snapshot: &crate::core::claude_watcher::ClaudeSessionsSnapshot,
        hook_snapshot: &crate::core::hook_server::ClaudeSessionStates,
    ) -> Vec<ClaudeDisplayEntry> {
        watcher_snapshot
            .sessions
            .iter()
            .map(|session| {
                let per_state = hook_snapshot.per_session.get(&session.session_id);
                let approval = match per_state {
                    Some(ClaudeState::ApprovalRequired { request, .. }) => {
                        Some(ClaudeDisplayApproval {
                            request: request.clone(),
                            pending_action: None,
                        })
                    }
                    _ => None,
                };
                ClaudeDisplayEntry {
                    session_id: session.session_id.clone(),
                    project: session.project.clone(),
                    preview_text: session.preview_text.clone(),
                    full_text: session.full_text.clone(),
                    stage: Self::classify_claude_entry(session, per_state),
                    last_active_at: session.last_active_at,
                    approval,
                }
            })
            .collect()
    }

    fn update_stable_claude_order(&mut self, entries: &[ClaudeDisplayEntry]) {
        let mut previous_index = HashMap::new();
        for (idx, id) in self.stable_claude_session_order.iter().enumerate() {
            previous_index.insert(id.clone(), idx);
        }

        let mut sorted = entries.to_vec();
        sorted.sort_by(|a, b| {
            let stage_cmp = Self::stage_rank(a.stage).cmp(&Self::stage_rank(b.stage));
            if stage_cmp != std::cmp::Ordering::Equal {
                return stage_cmp;
            }

            let activity_gap =
                a.last_active_at.max(b.last_active_at) - a.last_active_at.min(b.last_active_at);
            if activity_gap <= 1500 {
                let a_prev = previous_index
                    .get(&a.session_id)
                    .copied()
                    .unwrap_or(usize::MAX);
                let b_prev = previous_index
                    .get(&b.session_id)
                    .copied()
                    .unwrap_or(usize::MAX);
                let prev_cmp = a_prev.cmp(&b_prev);
                if prev_cmp != std::cmp::Ordering::Equal {
                    return prev_cmp;
                }
            }

            b.last_active_at.cmp(&a.last_active_at)
        });

        self.stable_claude_session_order =
            sorted.into_iter().map(|entry| entry.session_id).collect();
    }

    fn log_claude_merge_signature(&mut self, signature: String) {
        if self.last_claude_merge_log.as_ref() != Some(&signature) {
            println!("CLAUDE_MERGE {}", signature);
            self.last_claude_merge_log = Some(signature);
        }
    }

    fn log_claude_redraw_signature(&mut self, signature: String) {
        if self.last_claude_redraw_log.as_ref() != Some(&signature) {
            println!("CLAUDE_REDRAW {}", signature);
            self.last_claude_redraw_log = Some(signature);
        }
    }

    fn preview_text(text: &str) -> String {
        let mut preview = String::new();
        for ch in text.replace('\n', " ").chars() {
            if ch.is_ascii() {
                preview.push(ch);
            } else {
                preview.push('?');
            }
            if preview.len() >= 80 {
                break;
            }
        }
        preview
    }

    fn get_target_monitor(
        window: &Window,
        monitor_index: i32,
    ) -> Option<winit::monitor::MonitorHandle> {
        if monitor_index <= 0 {
            return window
                .primary_monitor()
                .or_else(|| window.current_monitor());
        }
        let monitors: Vec<_> = window.available_monitors().collect();
        let idx = monitor_index as usize;
        if idx < monitors.len() {
            Some(monitors[idx].clone())
        } else {
            window
                .primary_monitor()
                .or_else(|| window.current_monitor())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClaudeForegroundActionKind, build_choice_prompt_foreground_action_for_test,
        choice_prompt_response_payload,
    };
    use crate::core::claude_watcher::{
        ClaudeActionKind, ClaudeApprovalWait, ClaudeChoiceOption, ClaudeSessionSummary,
        ClaudeSessionsSnapshot,
    };
    use crate::core::hook_server::{
        ApprovalRequest, ClaudeSessionStateMeta, ClaudeSessionStates, ClaudeState,
    };

    #[test]
    fn builds_choice_prompt_foreground_action_from_watcher_snapshot() {
        let watcher_snapshot = ClaudeSessionsSnapshot {
            sessions: vec![ClaudeSessionSummary {
                session_id: "session-1".to_string(),
                project: "WinIsland".to_string(),
                preview_text: String::new(),
                full_text: String::new(),
                last_active_at: 1,
                approval_wait: Some(ClaudeApprovalWait {
                    waiting_for: "approve AskUserQuestion".to_string(),
                    status: "waiting".to_string(),
                    pid: 100,
                    detail_title: "Which approach should we use?".to_string(),
                    detail_text: "Which approach should we use?".to_string(),
                    action_kind: ClaudeActionKind::ChoicePrompt,
                    choice_options: vec![
                        ClaudeChoiceOption {
                            label: "Inline".to_string(),
                            value: "Inline".to_string(),
                        },
                        ClaudeChoiceOption {
                            label: "Subagent".to_string(),
                            value: "Subagent".to_string(),
                        },
                    ],
                }),
            }],
        };

        let action = build_choice_prompt_foreground_action_for_test(&watcher_snapshot).unwrap();

        assert!(matches!(
            action.kind,
            ClaudeForegroundActionKind::ChoicePrompt { .. }
        ));
    }

    #[test]
    fn choice_prompt_foreground_action_keeps_question_text_and_ignores_hook_detail_fallback() {
        let watcher_snapshot = ClaudeSessionsSnapshot {
            sessions: vec![ClaudeSessionSummary {
                session_id: "fresh-session".to_string(),
                project: "WinIsland".to_string(),
                preview_text: "Waiting for WinIsland approval...".to_string(),
                full_text: String::new(),
                last_active_at: 20,
                approval_wait: Some(ClaudeApprovalWait {
                    waiting_for: "approve AskUserQuestion".to_string(),
                    status: "waiting".to_string(),
                    pid: 200,
                    detail_title: "Choose an option".to_string(),
                    detail_text: "Choose an option\n\nFirst\nSecond\nThird\nFourth\nFifth"
                        .to_string(),
                    action_kind: ClaudeActionKind::ChoicePrompt,
                    choice_options: vec![
                        ClaudeChoiceOption {
                            label: "First".to_string(),
                            value: "First".to_string(),
                        },
                        ClaudeChoiceOption {
                            label: "Second".to_string(),
                            value: "Second".to_string(),
                        },
                        ClaudeChoiceOption {
                            label: "Third".to_string(),
                            value: "Third".to_string(),
                        },
                        ClaudeChoiceOption {
                            label: "Fourth".to_string(),
                            value: "Fourth".to_string(),
                        },
                        ClaudeChoiceOption {
                            label: "Fifth".to_string(),
                            value: "Fifth".to_string(),
                        },
                    ],
                }),
            }],
        };
        let mut hook_snapshot = ClaudeSessionStates::default();
        hook_snapshot.per_session_meta.insert(
            "fresh-session".to_string(),
            ClaudeSessionStateMeta {
                project: Some("WinIsland".to_string()),
                approval_detail: Some("cygpath: can't convert empty path".to_string()),
                approval_tool_name: Some("Bash".to_string()),
                approval_command: Some("/hooks/pre-tool-use.sh".to_string()),
                approval_file_path: None,
            },
        );

        let action = super::build_foreground_action(&watcher_snapshot, &hook_snapshot).unwrap();

        assert_eq!(action.reason_title, "Choose an option");
        assert_eq!(action.detail_text, "First\nSecond\nThird\nFourth\nFifth");
        match action.kind {
            ClaudeForegroundActionKind::ChoicePrompt { options } => {
                assert_eq!(options.len(), 5);
                assert_eq!(options[0].label, "First");
                assert_eq!(options[4].label, "Fifth");
            }
            _ => panic!("expected choice prompt"),
        }
    }

    #[test]
    fn permission_foreground_action_uses_hook_detail_when_waiting_text_is_generic() {
        let watcher_snapshot = ClaudeSessionsSnapshot {
            sessions: vec![ClaudeSessionSummary {
                session_id: "perm-session".to_string(),
                project: "WinIsland".to_string(),
                preview_text: "Waiting for WinIsland approval...".to_string(),
                full_text: String::new(),
                last_active_at: 20,
                approval_wait: Some(ClaudeApprovalWait {
                    waiting_for: "Bash command".to_string(),
                    status: "waiting".to_string(),
                    pid: 200,
                    detail_title: "Waiting for WinIsland approval...".to_string(),
                    detail_text: String::new(),
                    action_kind: ClaudeActionKind::PermissionApproval,
                    choice_options: vec![],
                }),
            }],
        };
        let mut hook_snapshot = ClaudeSessionStates::default();
        hook_snapshot.per_session_meta.insert(
            "perm-session".to_string(),
            ClaudeSessionStateMeta {
                project: Some("WinIsland".to_string()),
                approval_detail: Some("User wants to run a dangerous script".to_string()),
                approval_tool_name: Some("Bash".to_string()),
                approval_command: Some("rm -rf /tmp/test".to_string()),
                approval_file_path: None,
            },
        );

        let action = super::build_foreground_action(&watcher_snapshot, &hook_snapshot).unwrap();

        assert_eq!(action.reason_title, "Waiting for WinIsland approval...");
        assert!(
            action
                .detail_text
                .contains("User wants to run a dangerous script")
        );
        assert!(action.detail_text.contains("Tool: Bash"));
        assert!(
            action
                .detail_text
                .contains("Command: rm -rf /tmp/test")
        );
        match action.kind {
            ClaudeForegroundActionKind::PermissionApproval { actions } => {
                assert_eq!(actions.len(), 3);
                assert_eq!(actions[0].label, "Deny");
                assert_eq!(actions[2].label, "Allow for session");
            }
            _ => panic!("expected permission approval"),
        }
    }

    #[test]
    fn writes_choice_prompt_response_payload_with_selected_value() {
        let payload = choice_prompt_response_payload("Subagent");

        assert_eq!(
            payload,
            serde_json::json!({
                "selected": "Subagent"
            })
        );
    }

    #[test]
    fn permission_foreground_action_hides_known_hook_wrapper_path_errors() {
        let watcher_snapshot = ClaudeSessionsSnapshot {
            sessions: vec![ClaudeSessionSummary {
                session_id: "perm-session".to_string(),
                project: "WinIsland".to_string(),
                preview_text: "Waiting for WinIsland approval...".to_string(),
                full_text: String::new(),
                last_active_at: 20,
                approval_wait: Some(ClaudeApprovalWait {
                    waiting_for: "Bash command".to_string(),
                    status: "waiting".to_string(),
                    pid: 200,
                    detail_title: "Waiting for WinIsland approval...".to_string(),
                    detail_text: "Waiting for WinIsland approval...".to_string(),
                    action_kind: ClaudeActionKind::PermissionApproval,
                    choice_options: vec![],
                }),
            }],
        };
        let mut hook_snapshot = ClaudeSessionStates::default();
        hook_snapshot.per_session_meta.insert(
            "perm-session".to_string(),
            ClaudeSessionStateMeta {
                project: Some("WinIsland".to_string()),
                approval_detail: Some("cygpath: can't convert empty path\n/usr/bin/bash: line 1: /hooks/pre-tool-use.sh: No such file or directory".to_string()),
                approval_tool_name: Some("Bash".to_string()),
                approval_command: Some("/hooks/pre-tool-use.sh".to_string()),
                approval_file_path: None,
            },
        );

        let action = super::build_foreground_action(&watcher_snapshot, &hook_snapshot).unwrap();

        assert_eq!(action.detail_text, "Tool: Bash");
        match action.kind {
            ClaudeForegroundActionKind::PermissionApproval { actions } => {
                assert_eq!(actions.len(), 3);
            }
            _ => panic!("expected permission approval"),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let startup_collapsed_w = self.collapsed_interaction_width(false);
            let (startup_os_w, startup_os_h) =
                self.window_envelope_size(false, startup_collapsed_w, false);
            self.os_w = startup_os_w;
            self.os_h = startup_os_h;
            self.native_window_expanded = false;
            self.pending_collapse_resize = false;
            let attrs = Window::default_attributes()
                .with_title(WINDOW_TITLE)
                .with_inner_size(PhysicalSize::new(self.os_w, self.os_h))
                .with_transparent(true)
                .with_decorations(false)
                .with_resizable(false)
                .with_enabled_buttons(WindowButtons::empty())
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_skip_taskbar(true)
                .with_window_icon(get_app_icon());
            let window = Arc::new(event_loop.create_window(attrs).unwrap());

            if let Ok(handle) = window.window_handle() {
                if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                    let hwnd = HWND(win32_handle.hwnd.get() as _);
                    unsafe {
                        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                        SetWindowLongPtrW(
                            hwnd,
                            GWL_EXSTYLE,
                            ex_style | WS_EX_TOOLWINDOW.0 as isize | WS_EX_NOACTIVATE.0 as isize,
                        );
                        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                        SetWindowLongPtrW(
                            hwnd,
                            GWL_STYLE,
                            style & !(WS_MAXIMIZEBOX.0 as isize | WS_THICKFRAME.0 as isize),
                        );
                    }
                    set_glass_hwnd(win32_handle.hwnd.get() as isize);
                }
            }

            self.window = Some(window.clone());

            let mut monitor_opt = None;
            for _ in 0..10 {
                if let Some(monitor) = Self::get_target_monitor(&window, self.config.monitor_index)
                {
                    let size = monitor.size();
                    if size.width > 0 && size.height > 0 {
                        monitor_opt = Some(monitor);
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }

            if let Some(monitor) = monitor_opt {
                let (win_x, win_y) = self.centered_window_position(&monitor, self.os_w, self.os_h);
                self.win_x = win_x;
                self.win_y = win_y;
                window.set_outer_position(PhysicalPosition::new(self.win_x, self.win_y));
            }
            let context = Context::new(window.clone()).unwrap();
            let mut surface = Surface::new(&context, window.clone()).unwrap();
            surface
                .resize(
                    std::num::NonZeroU32::new(self.os_w).unwrap(),
                    std::num::NonZeroU32::new(self.os_h).unwrap(),
                )
                .unwrap();
            self.surface = Some(surface);
            let is_light = window.theme() == Some(winit::window::Theme::Light);
            self.tray = Some(TrayManager::new(is_light));
            Self::enforce_topmost(&window, self.win_x, self.win_y, self.os_w, self.os_h);
            self.reset_native_hit_region();
            self.apply_native_hit_region(&window);
            window.request_redraw();
        }
    }
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(win) = self.window.as_ref().cloned() {
            if win.id() == id {
                match event {
                    WindowEvent::ThemeChanged(theme) => {
                        let is_light = theme == winit::window::Theme::Light;
                        if let Some(tray) = self.tray.as_mut() {
                            tray.update_theme(is_light);
                        }
                    }
                    WindowEvent::Resized(_) => {
                        if win.is_maximized() {
                            win.set_maximized(false);
                        }
                    }
                    WindowEvent::CloseRequested => (),
                    WindowEvent::MouseInput {
                        state,
                        button: MouseButton::Left,
                        ..
                    } => {
                        let (px, py) = get_global_cursor_pos();
                        let rel_x = px - self.win_x;
                        let rel_y = py - self.win_y;
                        let (offset_x, current_island_y, island_w, island_h, hidden_peek_h) =
                            self.current_island_frame();
                        let island_y = PADDING as f64 / 2.0;
                        let scale = self.config.global_scale as f64;

                        let is_hovering_visible = is_point_in_rect(
                            rel_x as f64,
                            rel_y as f64,
                            offset_x,
                            current_island_y,
                            island_w,
                            island_h,
                        );

                        let hidden_handle_h = (24.0 * scale).max(14.0);
                        let hidden_handle_y =
                            (current_island_y + island_h - hidden_peek_h - hidden_handle_h * 0.35)
                                .max(0.0);
                        let is_on_hidden_handle = (self.auto_hidden || self.manually_hidden)
                            && is_point_in_rect(
                                rel_x as f64,
                                rel_y as f64,
                                offset_x,
                                hidden_handle_y,
                                island_w,
                                hidden_handle_h,
                            );

                        if state == ElementState::Pressed {
                            if self.expanded {
                                let view_val = self.spring_view.value as f64;
                                let w = self.spring_w.value as f64;
                                let h = self.spring_h.value as f64;
                                let page_shift = view_val * w;

                                let hook_snapshot = self.hook_server.get_session_states();
                                let watcher_snapshot = self.claude_watcher.get_snapshot();
                                let mut claude_entries =
                                    self.build_claude_entries(&watcher_snapshot, &hook_snapshot);
                                self.update_stable_claude_order(&claude_entries);
                                let stable_index: HashMap<_, _> = self
                                    .stable_claude_session_order
                                    .iter()
                                    .enumerate()
                                    .map(|(idx, id)| (id.clone(), idx))
                                    .collect();
                                claude_entries.sort_by_key(|entry| {
                                    stable_index
                                        .get(&entry.session_id)
                                        .copied()
                                        .unwrap_or(usize::MAX)
                                });
                                let foreground_approval =
                                    build_foreground_action(&watcher_snapshot, &hook_snapshot);

                                if view_val < 0.5 {
                                    let media = self.smtc.get_info();
                                    let music_on = self.config.smtc_enabled
                                        && !media.title.is_empty()
                                        && (media.is_playing
                                            || self.last_playing_time.elapsed()
                                                < Duration::from_secs(5));

                                    let (bx, by, bw, bh) = get_pause_btn_rect(
                                        offset_x as f32,
                                        island_y as f32,
                                        w as f32,
                                        h as f32,
                                        self.config.global_scale,
                                    );
                                    let cx = rel_x as f32 - (page_shift as f32);
                                    let cy = rel_y as f32;
                                    if music_on
                                        && cx >= bx
                                        && cx <= bx + bw
                                        && cy >= by
                                        && cy <= by + bh
                                    {
                                        trigger_pause_click(media.is_playing);
                                        self.smtc.request_toggle_play();
                                        return;
                                    }

                                    let (px, py, pw, ph) = get_prev_btn_rect(
                                        offset_x as f32,
                                        island_y as f32,
                                        w as f32,
                                        h as f32,
                                        self.config.global_scale,
                                    );
                                    if music_on
                                        && cx >= px
                                        && cx <= px + pw
                                        && cy >= py
                                        && cy <= py + ph
                                    {
                                        trigger_cover_flip();
                                        trigger_prev_click();
                                        self.smtc.request_prev();
                                        return;
                                    }

                                    let (nx, ny, nw, nh) = get_next_btn_rect(
                                        offset_x as f32,
                                        island_y as f32,
                                        w as f32,
                                        h as f32,
                                        self.config.global_scale,
                                    );
                                    if music_on
                                        && cx >= nx
                                        && cx <= nx + nw
                                        && cy >= ny
                                        && cy <= ny + nh
                                    {
                                        trigger_cover_flip();
                                        trigger_next_click();
                                        self.smtc.request_next();
                                        return;
                                    }

                                    if let Some((bar_left, bar_right, bar_top, bar_hit_h)) =
                                        get_progress_bar_rect(
                                            offset_x as f32,
                                            island_y as f32,
                                            w as f32,
                                            &media,
                                            music_on,
                                            self.config.global_scale,
                                        )
                                    {
                                        if cx >= bar_left
                                            && cx <= bar_right
                                            && cy >= bar_top
                                            && cy <= bar_top + bar_hit_h
                                        {
                                            let ratio = ((cx - bar_left) / (bar_right - bar_left))
                                                .clamp(0.0, 1.0);
                                            let seek_ms = (ratio as f64
                                                * media.duration_secs as f64
                                                * 1000.0)
                                                as u64;
                                            self.smtc.request_seek(seek_ms);
                                            self.seeking_progress = true;
                                            self.seeking_bar_left = bar_left;
                                            self.seeking_bar_right = bar_right;
                                            self.seeking_duration_ms = media.duration_secs * 1000;
                                            return;
                                        }
                                    }
                                }

                                if view_val > 0.5 {
                                    let page2_offset_x = offset_x + w - page_shift;

                                    let gear_x = page2_offset_x + w - 28.0 * scale;
                                    let gear_y = island_y + h - 28.0 * scale;
                                    let dist_sq = (rel_x as f64 - gear_x).powi(2)
                                        + (rel_y as f64 - gear_y).powi(2);
                                    if dist_sq <= (20.0 * scale).powi(2) {
                                        let _ = std::process::Command::new(
                                            std::env::current_exe().unwrap(),
                                        )
                                        .arg("--settings")
                                        .spawn();
                                        return;
                                    }

                                    let arrow_x = page2_offset_x + 12.0 * scale;
                                    let arrow_y = island_y + h / 2.0;
                                    let adx = rel_x as f64 - arrow_x;
                                    let ady = rel_y as f64 - arrow_y;
                                    if adx * adx + ady * ady <= (20.0 * scale).powi(2) {
                                        if foreground_approval.is_some() {
                                            self.expanded = false;
                                            self.queue_window_mode_sync(&win, false);
                                        } else if self.selected_claude_session_id.is_some() {
                                            self.selected_claude_session_id = None;
                                            self.claude_detail_scroll = 0.0;
                                        } else {
                                            self.widget_view = false;
                                        }
                                        return;
                                    }

                                    if let Some(foreground_action) = foreground_approval.as_ref() {
                                        match &foreground_action.kind {
                                            ClaudeForegroundActionKind::PermissionApproval {
                                                actions,
                                            } => {
                                                let button_rects = permission_button_rects(
                                                    w as f32,
                                                    h as f32,
                                                    self.config.global_scale,
                                                );
                                                for (idx, button) in actions
                                                    .iter()
                                                    .take(button_rects.len())
                                                    .enumerate()
                                                {
                                                    let rect = button_rects[idx];
                                                    let left = page2_offset_x + rect.left as f64;
                                                    let right = page2_offset_x + rect.right as f64;
                                                    let top = island_y + rect.top as f64;
                                                    let bottom = island_y + rect.bottom as f64;
                                                    if rel_x as f64 >= left
                                                        && rel_x as f64 <= right
                                                        && rel_y as f64 >= top
                                                        && rel_y as f64 <= bottom
                                                    {
                                                        self.submit_permission_action(
                                                            &foreground_action.session_id,
                                                            button.action.clone(),
                                                        );
                                                        return;
                                                    }
                                                }
                                            }
                                            ClaudeForegroundActionKind::ChoicePrompt {
                                                options,
                                            } => {
                                                for button in choice_button_rects(
                                                    options,
                                                    w as f32,
                                                    h as f32,
                                                    self.config.global_scale,
                                                    self.claude_detail_scroll,
                                                ) {
                                                    let left =
                                                        page2_offset_x + button.rect.left as f64;
                                                    let right =
                                                        page2_offset_x + button.rect.right as f64;
                                                    let top = island_y + button.rect.top as f64;
                                                    let bottom =
                                                        island_y + button.rect.bottom as f64;
                                                    if rel_x as f64 >= left
                                                        && rel_x as f64 <= right
                                                        && rel_y as f64 >= top
                                                        && rel_y as f64 <= bottom
                                                    {
                                                        self.submit_choice_prompt_response(
                                                            &foreground_action.session_id,
                                                            button.value.clone(),
                                                        );
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if foreground_approval.is_none()
                                        && self.selected_claude_session_id.is_none()
                                    {
                                        let row_h = 42.0 * scale;
                                        let start_y = island_y + 24.0 * scale;
                                        let row_left = page2_offset_x + 16.0 * scale;
                                        let row_right = page2_offset_x + w - 16.0 * scale;
                                        for (idx, entry) in
                                            claude_entries.iter().take(4).enumerate()
                                        {
                                            let row_top =
                                                start_y + idx as f64 * row_h - 14.0 * scale;
                                            let row_bottom = row_top + row_h;
                                            if rel_x as f64 >= row_left
                                                && rel_x as f64 <= row_right
                                                && rel_y as f64 >= row_top
                                                && rel_y as f64 <= row_bottom
                                            {
                                                self.selected_claude_session_id =
                                                    Some(entry.session_id.clone());
                                                self.claude_detail_scroll = 0.0;
                                                return;
                                            }
                                        }
                                    }
                                }

                                if view_val < 0.5 {
                                    let arrow_x = offset_x + w - 12.0 * scale;
                                    let arrow_y = island_y + h / 2.0;
                                    let adx = rel_x as f64 - arrow_x;
                                    let ady = rel_y as f64 - arrow_y;
                                    if adx * adx + ady * ady <= (20.0 * scale).powi(2) {
                                        self.widget_view = true;
                                        return;
                                    }
                                }

                                if (rel_y as f64) < island_y + 40.0 * scale {
                                    self.expanded = false;
                                    self.queue_window_mode_sync(&win, false);
                                }
                            } else {
                                if is_hovering_visible || is_on_hidden_handle {
                                    self.is_dragging = true;
                                    self.drag_start_py = py;
                                    self.drag_start_hide_val = self.spring_hide.value;
                                    self.drag_has_moved = false;
                                }
                            }
                        } else if state == ElementState::Released {
                            if self.seeking_progress {
                                self.seeking_progress = false;
                                return;
                            }
                            if self.is_dragging {
                                self.is_dragging = false;
                                if !self.drag_has_moved {
                                    if self.auto_hidden || self.manually_hidden {
                                        self.auto_hidden = false;
                                        self.manually_hidden = false;
                                        self.spring_hide.velocity = -0.45;
                                        self.idle_timer = Instant::now();
                                    } else {
                                        self.expanded = true;
                                        self.queue_window_mode_sync(&win, false);
                                    }
                                } else {
                                    if self.spring_hide.value > 0.3 {
                                        self.manually_hidden = true;
                                        self.auto_hidden = false;
                                    } else {
                                        self.manually_hidden = false;
                                        self.auto_hidden = false;
                                    }
                                }
                                return;
                            }
                        }
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        if self.expanded && self.spring_view.value > 0.5 {
                            let hook_snapshot = self.hook_server.get_session_states();
                            let watcher_snapshot = self.claude_watcher.get_snapshot();
                            let mut claude_entries =
                                self.build_claude_entries(&watcher_snapshot, &hook_snapshot);
                            self.update_stable_claude_order(&claude_entries);
                            let stable_index: HashMap<_, _> = self
                                .stable_claude_session_order
                                .iter()
                                .enumerate()
                                .map(|(idx, id)| (id.clone(), idx))
                                .collect();
                            claude_entries.sort_by_key(|entry| {
                                stable_index
                                    .get(&entry.session_id)
                                    .copied()
                                    .unwrap_or(usize::MAX)
                            });
                            let foreground_approval =
                                build_foreground_action(&watcher_snapshot, &hook_snapshot);

                            let scroll_delta = match delta {
                                MouseScrollDelta::LineDelta(_, y) => -y * 24.0,
                                MouseScrollDelta::PixelDelta(pos) => -pos.y as f32,
                            };
                            if scroll_delta.abs() > f32::EPSILON {
                                let limit = if let Some(approval) = foreground_approval.as_ref() {
                                    approval_scroll_limit(
                                        approval,
                                        self.spring_w.value,
                                        self.spring_h.value,
                                        self.config.global_scale,
                                    )
                                } else if let Some(selected) =
                                    self.selected_claude_session_id.as_ref().and_then(|id| {
                                        claude_entries.iter().find(|entry| &entry.session_id == id)
                                    })
                                {
                                    detail_scroll_limit(
                                        selected,
                                        self.spring_w.value,
                                        self.spring_h.value,
                                        self.config.global_scale,
                                    )
                                } else {
                                    0.0
                                };
                                let next_scroll =
                                    (self.claude_detail_scroll + scroll_delta).clamp(0.0, limit);
                                if (next_scroll - self.claude_detail_scroll).abs() > f32::EPSILON {
                                    self.claude_detail_scroll = next_scroll;
                                    win.request_redraw();
                                }
                            }
                        }
                    }
                    WindowEvent::RedrawRequested => {
                        let sigmas = if self.config.motion_blur {
                            calculate_blur_sigmas(
                                self.spring_w.velocity,
                                self.spring_h.velocity,
                                self.spring_view.velocity,
                                self.spring_w.value,
                            )
                        } else {
                            (0.0, 0.0)
                        };
                        let total_h = (self.config.expanded_height - self.config.base_height)
                            .abs()
                            .max(1.0)
                            * self.config.global_scale;
                        let dist_h = (self.spring_h.value
                            - self.config.base_height * self.config.global_scale)
                            .abs();
                        let progress = (dist_h / total_h).clamp(0.0, 1.0);
                        let mut media_info = if self.config.smtc_enabled {
                            self.smtc.get_info()
                        } else {
                            crate::core::smtc::MediaInfo::default()
                        };
                        media_info.spectrum = self.audio.get_spectrum();
                        let mut music_active = false;
                        if self.config.smtc_enabled && !media_info.title.is_empty() {
                            if media_info.is_playing {
                                music_active = true;
                            } else if self.last_playing_time.elapsed() < Duration::from_secs(5) {
                                music_active = true;
                            }
                        }

                        let hook_snapshot = self.hook_server.get_session_states();
                        let hook_state = hook_snapshot.current.clone();
                        let watcher_snapshot = self.claude_watcher.get_snapshot();

                        let mut claude_entries =
                            self.build_claude_entries(&watcher_snapshot, &hook_snapshot);

                        self.update_stable_claude_order(&claude_entries);
                        let stable_index: HashMap<_, _> = self
                            .stable_claude_session_order
                            .iter()
                            .enumerate()
                            .map(|(idx, id)| (id.clone(), idx))
                            .collect();
                        claude_entries.sort_by_key(|entry| {
                            stable_index
                                .get(&entry.session_id)
                                .copied()
                                .unwrap_or(usize::MAX)
                        });
                        let foreground_approval =
                            build_foreground_action(&watcher_snapshot, &hook_snapshot);
                        self.sync_foreground_approval_scroll(foreground_approval.as_ref());
                        self.sync_approval_page_visibility(&win, &watcher_snapshot, &hook_snapshot);

                        if let Some(selected_id) = self.selected_claude_session_id.clone() {
                            if !claude_entries
                                .iter()
                                .any(|entry| entry.session_id == selected_id)
                            {
                                self.selected_claude_session_id = None;
                                self.claude_detail_scroll = 0.0;
                            }
                        }

                        let claude_state = match &hook_state {
                            crate::core::hook_server::ClaudeState::Thinking { .. } => {
                                if self.thinking_started_at.is_none() {
                                    self.thinking_started_at = Some(std::time::Instant::now());
                                    self.thinking_baseline_content = watcher_snapshot
                                        .sessions
                                        .first()
                                        .map(|session| session.preview_text.clone())
                                        .unwrap_or_default();
                                }
                                hook_state.clone()
                            }
                            crate::core::hook_server::ClaudeState::Working { .. } => {
                                self.thinking_started_at = None;
                                self.thinking_baseline_content.clear();
                                hook_state.clone()
                            }
                            crate::core::hook_server::ClaudeState::Done { .. }
                            | crate::core::hook_server::ClaudeState::Attention { .. }
                            | crate::core::hook_server::ClaudeState::ApprovalRequired { .. } => {
                                self.thinking_started_at = None;
                                self.thinking_baseline_content.clear();
                                hook_state.clone()
                            }
                            crate::core::hook_server::ClaudeState::Idle => {
                                self.thinking_started_at = None;
                                self.thinking_baseline_content.clear();
                                hook_state.clone()
                            }
                        };
                        let claude_merge_signature = format!(
                            "raw={} watcher_sessions={} watcher_preview={} merged={}",
                            hook_state,
                            watcher_snapshot.sessions.len(),
                            watcher_snapshot
                                .sessions
                                .first()
                                .map(|session| Self::preview_text(&session.preview_text))
                                .unwrap_or_else(|| "<none>".to_string()),
                            claude_state
                        );

                        if let Some(surface) = self.surface.as_mut() {
                            draw_island(
                                surface,
                                self.spring_w.value,
                                self.spring_h.value,
                                self.spring_r.value,
                                self.os_w,
                                self.os_h,
                                self.border_weights,
                                sigmas,
                                progress,
                                self.spring_view.value,
                                &media_info,
                                music_active,
                                self.config.global_scale,
                                &self.current_lyric_text,
                                &self.old_lyric_text,
                                self.lyric_transition,
                                &self.current_claude_text,
                                &self.old_claude_text,
                                self.claude_transition,
                                self.claude_waiting_visual,
                                self.config.motion_blur,
                                self.spring_hide.value,
                                self.lyric_scroll_offset,
                                &self.config.island_style,
                                self.win_x,
                                self.win_y,
                                self.config.font_size,
                                self.claude_pulse_time,
                                &claude_entries,
                                foreground_approval.as_ref(),
                                self.selected_claude_session_id.as_deref(),
                                self.claude_detail_scroll,
                                &claude_state,
                            );
                            self.log_claude_merge_signature(claude_merge_signature);
                        }
                    }
                    _ => (),
                }
            }
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = self.window.clone() {
            Self::enforce_topmost(&window, self.win_x, self.win_y, self.os_w, self.os_h);
            let frame_start = Instant::now();
            if let Some(tray) = &self.tray {
                if let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
                    match TrayAction::from_id(event.id, tray) {
                        Some(TrayAction::ToggleVisibility) => {
                            self.visible = !self.visible;
                            window.set_visible(self.visible);
                            tray.update_item_text(self.visible);
                        }
                        Some(TrayAction::OpenSettings) => {
                            let _ = std::process::Command::new(std::env::current_exe().unwrap())
                                .arg("--settings")
                                .spawn();
                        }
                        Some(TrayAction::InstallHooks) => {
                            use crate::core::hook_installer::HookInstaller;
                            match HookInstaller::new() {
                                Ok(installer) => match installer.install() {
                                    Ok(_) => {
                                        println!("[App] Claude Code hooks installed successfully")
                                    }
                                    Err(e) => eprintln!("[App] Failed to install hooks: {}", e),
                                },
                                Err(e) => eprintln!("[App] Failed to create installer: {}", e),
                            }
                        }
                        Some(TrayAction::UninstallHooks) => {
                            use crate::core::hook_installer::HookInstaller;
                            match HookInstaller::new() {
                                Ok(installer) => match installer.uninstall() {
                                    Ok(_) => {
                                        println!("[App] Claude Code hooks uninstalled successfully")
                                    }
                                    Err(e) => eprintln!("[App] Failed to uninstall hooks: {}", e),
                                },
                                Err(e) => eprintln!("[App] Failed to create installer: {}", e),
                            }
                        }
                        Some(TrayAction::Exit) => {
                            event_loop.exit();
                        }
                        None => (),
                    }
                }
            }
            if self.frame_count % 60 == 0 {
                let current_config = load_config();
                if current_config != self.config {
                    self.config = current_config;
                    self.smtc
                        .set_lyrics_source(self.config.lyrics_source.clone());
                    self.smtc.set_lyrics_fallback(self.config.lyrics_fallback);
                    self.smtc.set_allowed_apps(self.config.smtc_apps.clone());
                    self.queue_window_mode_sync(&window, false);
                } else if let Some(monitor) =
                    Self::get_target_monitor(&window, self.config.monitor_index)
                {
                    let mon_size = monitor.size();
                    let mon_pos = monitor.position();
                    let cur_mon_size = (mon_size.width, mon_size.height);
                    let cur_mon_pos = (mon_pos.x, mon_pos.y);
                    if cur_mon_size != self.last_mon_size || cur_mon_pos != self.last_mon_pos {
                        if cur_mon_size.0 > 0 && cur_mon_size.1 > 0 {
                            self.last_mon_size = cur_mon_size;
                            self.last_mon_pos = cur_mon_pos;
                            let (win_x, win_y) =
                                self.centered_window_position(&monitor, self.os_w, self.os_h);
                            self.win_x = win_x;
                            self.win_y = win_y;
                            window
                                .set_outer_position(PhysicalPosition::new(self.win_x, self.win_y));
                        }
                    }
                }
            }

            let mut music_active = false;
            let media = self.smtc.get_info();
            if self.config.smtc_enabled && !media.title.is_empty() {
                self.last_media_playing = media.is_playing;
                if self.last_media_playing {
                    self.last_playing_time = Instant::now();
                    music_active = true;
                } else if self.last_playing_time.elapsed() < Duration::from_secs(5) {
                    music_active = true;
                }
                if media.title != self.last_media_title {
                    self.last_media_title = media.title.clone();
                    window.request_redraw();
                }
            }
            self.sync_collapsed_window_envelope_if_needed(&window, music_active);
            self.finalize_pending_collapse_resize(&window, music_active);

            let dt = (self.last_frame_time.elapsed().as_secs_f32() * 60.0).clamp(0.1, 3.0);
            self.last_frame_time = Instant::now();
            self.claude_pulse_time += dt / 60.0;

            if !self.visible {
                std::thread::sleep(Duration::from_millis(16));
                return;
            }
            let (px, py) = get_global_cursor_pos();
            let rel_x = px - self.win_x;
            let rel_y = py - self.win_y;
            let (offset_x, current_island_y, island_w, island_h, hidden_peek_h) =
                self.current_island_frame();
            let hide_distance = (island_h - hidden_peek_h + TOP_OFFSET as f64).max(0.0);
            let island_y = PADDING as f64 / 2.0;
            let is_hovering_visible = is_point_in_rect(
                rel_x as f64,
                rel_y as f64,
                offset_x,
                current_island_y,
                island_w,
                island_h,
            );
            let hidden_handle_h = (24.0 * self.config.global_scale as f64).max(14.0);
            let hidden_handle_y =
                (current_island_y + island_h - hidden_peek_h - hidden_handle_h * 0.35).max(0.0);
            let is_on_hidden_handle = (self.auto_hidden || self.manually_hidden)
                && is_point_in_rect(
                    rel_x as f64,
                    rel_y as f64,
                    offset_x,
                    hidden_handle_y,
                    island_w,
                    hidden_handle_h,
                );
            self.apply_native_hit_region(&window);

            let raw_hook_state = self.hook_server.get_state();
            self.update_claude_waiting_visual(&raw_hook_state);
            self.update_collapsed_claude_transition(&raw_hook_state, &window);

            let is_idle =
                !is_hovering_visible && !self.expanded && !music_active && !self.is_dragging;
            if !self.config.auto_hide {
                self.auto_hidden = false;
                self.idle_timer = Instant::now();
            } else {
                if music_active && self.auto_hidden && !self.manually_hidden {
                    self.auto_hidden = false;
                    self.idle_timer = Instant::now();
                    self.spring_hide.velocity = -0.65;
                } else if self.auto_hidden {
                    if is_on_hidden_handle || is_hovering_visible {
                        self.auto_hidden = false;
                        self.idle_timer = Instant::now();
                        self.spring_hide.velocity = -0.45;
                    } else if !self.expanded && !music_active {
                        // Let idle_timer expire
                    }
                } else if is_idle && !self.manually_hidden {
                    if self.idle_timer.elapsed().as_secs_f32() > self.config.auto_hide_delay {
                        self.auto_hidden = true;
                    }
                } else if !self.manually_hidden && !is_idle {
                    self.idle_timer = Instant::now();
                }
            }

            // Handle dragging on the progress bar while mouse is held
            if self.seeking_progress && is_left_button_pressed() {
                let click_x = rel_x as f32;
                let ratio = ((click_x - self.seeking_bar_left)
                    / (self.seeking_bar_right - self.seeking_bar_left))
                    .clamp(0.0, 1.0);
                let seek_ms = (ratio as f64 * self.seeking_duration_ms as f64) as u64;
                self.smtc.request_seek(seek_ms);
                window.request_redraw();
            } else if self.seeking_progress {
                self.seeking_progress = false;
            }

            let progress_hover_active = if self.seeking_progress {
                true
            } else if self.expanded && (self.spring_view.value as f64) < 0.5 {
                if let Some((bar_left, bar_right, bar_top, bar_hit_h)) = get_progress_bar_rect(
                    offset_x as f32,
                    island_y as f32,
                    self.spring_w.value,
                    &media,
                    music_active,
                    self.config.global_scale,
                ) {
                    let page_shift = self.spring_view.value * self.spring_w.value;
                    let cx = rel_x as f32 - page_shift;
                    let cy = rel_y as f32;
                    let margin = 4.0 * self.config.global_scale;
                    cx >= bar_left - margin
                        && cx <= bar_right + margin
                        && cy >= bar_top - margin
                        && cy <= bar_top + bar_hit_h + margin
                } else {
                    false
                }
            } else {
                false
            };
            set_progress_hover(progress_hover_active);

            if self.is_dragging {
                let diff_y = self.drag_start_py - py;
                if diff_y.abs() > 3 {
                    self.drag_has_moved = true;
                }
                if hide_distance > 0.0 {
                    let mut new_val =
                        self.drag_start_hide_val + (diff_y as f32 / hide_distance as f32);
                    new_val = new_val.clamp(0.0, 1.0);
                    self.spring_hide.value = new_val;
                    self.spring_hide.velocity = 0.0;
                    window.request_redraw();
                }
            } else {
                let hide_target = if self.auto_hidden || self.manually_hidden {
                    1.0
                } else {
                    0.0
                };
                let (stiffness, damping) = if self.auto_hidden || self.manually_hidden {
                    (0.12, 0.70)
                } else {
                    (0.08, 0.78)
                };
                self.spring_hide
                    .update_dt(hide_target, stiffness, damping, dt);
            }

            if self.spring_hide.velocity.abs() > 0.001
                || (self.spring_hide.value > 0.0 && self.spring_hide.value < 1.0)
            {
                window.request_redraw();
            }
            self.apply_native_hit_region(&window);

            if self.expanded && !is_hovering_visible && is_left_button_pressed() {
                self.expanded = false;
                window.request_redraw();
            }

            if !self.expanded && is_hovering_visible && is_left_button_pressed() {
                self.idle_timer = Instant::now();
            }

            if self.config.adaptive_border {
                if self.frame_count % 30 == 0 {
                    let island_cx = self.win_x + (self.os_w as i32 / 2);
                    let island_cy =
                        self.win_y + (PADDING as i32 / 2) + (self.spring_h.value as i32 / 2);
                    let raw_weights = get_island_border_weights(
                        island_cx,
                        island_cy,
                        self.spring_w.value,
                        self.spring_h.value,
                    );
                    self.target_border_weights =
                        raw_weights.map(|w| if w > 0.85 { w } else { 0.0 });
                }
            } else {
                self.target_border_weights = [0.0; 4];
            }
            self.frame_count += 1;
            for i in 0..4 {
                let diff = self.target_border_weights[i] - self.border_weights[i];
                if diff.abs() > 0.005 {
                    self.border_weights[i] += diff * 0.1 * dt;
                } else {
                    self.border_weights[i] = self.target_border_weights[i];
                }
            }

            let current_lyric_opt = if self.config.show_lyrics {
                media.current_lyric((self.config.lyrics_delay * 1000.0) as i64)
            } else {
                None
            };
            if let Some(lyric) = current_lyric_opt {
                if lyric != self.current_lyric_text {
                    self.old_lyric_text = self.current_lyric_text.clone();
                    self.current_lyric_text = lyric.clone();
                    self.lyric_transition = 0.0;
                    self.lyric_scroll_offset = 0.0;
                    self.lyric_scroll_pause = 0.0;
                }
            } else if !self.current_lyric_text.is_empty() {
                self.old_lyric_text = self.current_lyric_text.clone();
                self.current_lyric_text = String::new();
                self.lyric_transition = 0.0;
                self.lyric_scroll_offset = 0.0;
                self.lyric_scroll_pause = 0.0;
            }

            if self.lyric_transition < 1.0 {
                self.lyric_transition += 0.05 * dt;
                if self.lyric_transition > 1.0 {
                    self.lyric_transition = 1.0;
                }
                window.request_redraw();
            }

            if self.claude_transition < 1.0 {
                self.claude_transition += 0.05 * dt;
                if self.claude_transition > 1.0 {
                    self.claude_transition = 1.0;
                }
                window.request_redraw();
            }

            let (_, target_w, target_h, target_r, target_view) =
                self.target_dimensions(music_active);
            self.spring_w.update_dt(target_w, 0.10, 0.68, dt);
            self.spring_h.update_dt(target_h, 0.10, 0.68, dt);
            self.spring_r.update_dt(target_r, 0.10, 0.68, dt);
            self.spring_view.update_dt(target_view, 0.12, 0.68, dt);
            self.finalize_pending_collapse_resize(&window, music_active);
            self.sync_collapsed_window_envelope_if_needed(&window, music_active);

            let raw_hook_state = self.hook_server.get_state();
            let merged_claude_state = raw_hook_state.clone();

            // Check if Claude Code is active
            let claude_active =
                !matches!(raw_hook_state, crate::core::hook_server::ClaudeState::Idle);
            let claude_animating = !self.expanded && self.widget_view && self.claude_waiting_visual;
            let merged_claude_active = claude_active;
            let has_animation = self.spring_w.velocity.abs() > 0.001
                || self.spring_h.velocity.abs() > 0.001
                || self.spring_r.velocity.abs() > 0.001
                || self.spring_view.velocity.abs() > 0.001;
            let redraw_requested =
                self.expanded || music_active || claude_active || claude_animating || has_animation;
            if claude_active != merged_claude_active {
                let claude_redraw_signature = format!(
                    "raw_active={} merged_active={} music_active={} redraw_requested={} raw={} merged={}",
                    claude_active,
                    merged_claude_active,
                    music_active,
                    redraw_requested,
                    raw_hook_state,
                    merged_claude_state
                );
                self.log_claude_redraw_signature(claude_redraw_signature);
            }

            if redraw_requested {
                window.request_redraw();
            }
            let elapsed = frame_start.elapsed();
            let target_frame_time = Duration::from_micros(6944);
            if elapsed < target_frame_time {
                std::thread::sleep(target_frame_time - elapsed);
            }
        }
    }
}
