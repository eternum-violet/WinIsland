use crate::icons::arrows::draw_arrow_left;
use crate::utils::font::FontManager;
use crate::window::app::{
    ClaudeChoiceButton, ClaudeDisplayEntry, ClaudeDisplayStage, ClaudeForegroundActionKind,
    ClaudeForegroundApproval, ClaudePermissionActionButton,
};
use skia_safe::{Canvas, Color, FontStyle, Paint, Rect};

#[derive(Debug, Clone)]
pub struct ChoiceButtonRect {
    pub label: String,
    pub value: String,
    pub rect: Rect,
}

pub fn permission_button_rects(w: f32, h: f32, scale: f32) -> [Rect; 3] {
    let button_h = 30.0 * scale;
    let gap = 8.0 * scale;
    let side_padding = 20.0 * scale;
    let bottom_margin = 18.0 * scale;
    let total_gap = gap * 2.0;
    let button_w = ((w - side_padding * 2.0 - total_gap) / 3.0).max(48.0 * scale);
    let top = h - bottom_margin - button_h;

    std::array::from_fn(|idx| {
        let left = side_padding + idx as f32 * (button_w + gap);
        Rect::from_xywh(left, top, button_w, button_h)
    })
}

pub fn choice_button_rects(
    options: &[ClaudeChoiceButton],
    w: f32,
    h: f32,
    scale: f32,
    detail_scroll: f32,
) -> Vec<ChoiceButtonRect> {
    let side_padding = 20.0 * scale;
    let top = 118.0 * scale;
    let gap = 8.0 * scale;
    let width = (w - side_padding * 2.0).max(40.0 * scale);
    let button_h = 30.0 * scale;
    let scroll_top = 76.0 * scale;
    let bottom_limit = h - 18.0 * scale;
    let clamped_scroll = detail_scroll.max(0.0);
    if options.is_empty() || bottom_limit <= scroll_top {
        return Vec::new();
    }

    options
        .iter()
        .enumerate()
        .map(|(idx, option)| {
            let y = top + idx as f32 * (button_h + gap) - clamped_scroll;
            ChoiceButtonRect {
                label: option.label.clone(),
                value: option.value.clone(),
                rect: Rect::from_xywh(side_padding, y, width, button_h),
            }
        })
        .filter(|button| button.rect.bottom >= scroll_top && button.rect.top <= bottom_limit)
        .collect()
}

pub fn approval_scroll_limit(
    approval: &ClaudeForegroundApproval,
    w: f32,
    h: f32,
    scale: f32,
) -> f32 {
    let body_size = 10.0 * scale;
    let line_height = body_size * 1.45;
    let text_width = w - 56.0 * scale;
    let content_top = 76.0 * scale;
    let content_bottom = match &approval.kind {
        ClaudeForegroundActionKind::PermissionApproval { .. } => h - 62.0 * scale,
        ClaudeForegroundActionKind::ChoicePrompt { .. } => h - 20.0 * scale,
    };
    let visible_height = (content_bottom - content_top).max(line_height);
    let wrapped = wrap_text_lines(&approval.detail_text, body_size, text_width);
    let text_height = wrapped.len() as f32 * line_height;
    let buttons_height = match &approval.kind {
        ClaudeForegroundActionKind::PermissionApproval { .. } => 0.0,
        ClaudeForegroundActionKind::ChoicePrompt { options } => {
            if options.is_empty() {
                0.0
            } else {
                let first_top = 118.0 * scale;
                let last_bottom = first_top
                    + options.len() as f32 * (30.0 * scale)
                    + (options.len().saturating_sub(1) as f32) * (8.0 * scale);
                (last_bottom - content_top).max(0.0)
            }
        }
    };
    (text_height + buttons_height - visible_height).max(0.0)
}

pub fn detail_scroll_limit(entry: &ClaudeDisplayEntry, w: f32, h: f32, scale: f32) -> f32 {
    let body_size = 10.0 * scale;
    let line_height = body_size * 1.45;
    let text_width = w - 56.0 * scale;
    let content_top = 72.0 * scale;
    let content_bottom = h - if entry.approval.is_some() {
        70.0 * scale
    } else {
        20.0 * scale
    };
    let visible_height = (content_bottom - content_top).max(line_height);
    let content = approval_content(entry).unwrap_or_else(|| entry.full_text.clone());
    let wrapped = wrap_text_lines(&content, body_size, text_width);
    let content_height = wrapped.len() as f32 * line_height;
    (content_height - visible_height).max(0.0)
}

pub fn draw_approval_page(
    canvas: &Canvas,
    ox: f32,
    oy: f32,
    w: f32,
    h: f32,
    alpha: u8,
    scale: f32,
    approval: &ClaudeForegroundApproval,
    detail_scroll: f32,
) {
    draw_arrow_left(canvas, ox + 12.0 * scale, oy + h / 2.0, alpha, scale);

    if alpha <= 30 {
        return;
    }

    let title_size = 12.0 * scale;
    let body_size = 10.0 * scale;

    let mut title_paint = Paint::default();
    title_paint.set_color(Color::from_argb(alpha, 255, 255, 255));
    title_paint.set_anti_alias(true);
    FontManager::global().draw_text_cached(
        canvas,
        &approval.project,
        (ox + 28.0 * scale, oy + 26.0 * scale),
        title_size,
        FontStyle::normal(),
        &title_paint,
        false,
        w - 56.0 * scale,
    );

    let mut body_paint = Paint::default();
    body_paint.set_color(Color::from_argb(alpha, 220, 220, 220));
    body_paint.set_anti_alias(true);
    FontManager::global().draw_text_cached(
        canvas,
        &approval.reason_title,
        (ox + 28.0 * scale, oy + 46.0 * scale),
        body_size,
        FontStyle::normal(),
        &body_paint,
        false,
        w - 56.0 * scale,
    );

    let text_width = w - 56.0 * scale;
    let line_height = body_size * 1.45;
    let content_top = oy + 76.0 * scale;
    let wrapped = wrap_text_lines(&approval.detail_text, body_size, text_width);
    let clamped_scroll = detail_scroll.clamp(0.0, approval_scroll_limit(approval, w, h, scale));

    match &approval.kind {
        ClaudeForegroundActionKind::PermissionApproval { actions } => {
            let content_bottom = oy + h - 62.0 * scale;
            for (idx, line) in wrapped.iter().enumerate() {
                let line_y = content_top + idx as f32 * line_height - clamped_scroll;
                if line_y < content_top - line_height || line_y > content_bottom {
                    continue;
                }
                FontManager::global().draw_text_cached(
                    canvas,
                    line,
                    (ox + 28.0 * scale, line_y),
                    body_size,
                    FontStyle::normal(),
                    &body_paint,
                    false,
                    text_width,
                );
            }
            draw_permission_buttons(canvas, ox, oy, alpha, scale, actions, w, h);
        }
        ClaudeForegroundActionKind::ChoicePrompt { options } => {
            let button_rects = choice_button_rects(options, w, h, scale, clamped_scroll);
            let content_bottom = oy + h - 18.0 * scale;
            for (idx, line) in wrapped.iter().enumerate() {
                let line_y = content_top + idx as f32 * line_height - clamped_scroll;
                if line_y < content_top - line_height || line_y > content_bottom {
                    continue;
                }
                FontManager::global().draw_text_cached(
                    canvas,
                    line,
                    (ox + 28.0 * scale, line_y),
                    body_size,
                    FontStyle::normal(),
                    &body_paint,
                    false,
                    text_width,
                );
            }
            draw_choice_buttons(canvas, ox, oy, alpha, scale, &button_rects);
        }
    }
}

pub fn draw_widget_page(
    canvas: &Canvas,
    ox: f32,
    oy: f32,
    w: f32,
    h: f32,
    alpha: u8,
    scale: f32,
    entries: &[ClaudeDisplayEntry],
    selected_session_id: Option<&str>,
    detail_scroll: f32,
) {
    let arrow_alpha = alpha;
    if arrow_alpha > 0 {
        draw_arrow_left(canvas, ox + 12.0 * scale, oy + h / 2.0, arrow_alpha, scale);
    }

    if alpha <= 30 {
        return;
    }

    let title_size = 12.0 * scale;
    let body_size = 10.0 * scale;

    if let Some(selected) =
        selected_session_id.and_then(|id| entries.iter().find(|entry| entry.session_id == id))
    {
        let mut title_paint = Paint::default();
        title_paint.set_color(Color::from_argb(alpha, 255, 255, 255));
        title_paint.set_anti_alias(true);
        FontManager::global().draw_text_cached(
            canvas,
            &selected.project,
            (ox + 28.0 * scale, oy + 26.0 * scale),
            title_size,
            FontStyle::normal(),
            &title_paint,
            false,
            w - 56.0 * scale,
        );

        let stage_label = match selected.stage {
            ClaudeDisplayStage::Ready => "Ready",
            ClaudeDisplayStage::Running => "Running",
            ClaudeDisplayStage::IdleOrOther => "Idle",
        };
        let stage_color = match selected.stage {
            ClaudeDisplayStage::Ready => Color::from_argb(alpha, 74, 222, 128),
            ClaudeDisplayStage::Running => Color::from_argb(alpha, 147, 130, 255),
            ClaudeDisplayStage::IdleOrOther => Color::from_argb(alpha, 150, 150, 150),
        };
        let mut stage_paint = Paint::default();
        stage_paint.set_color(stage_color);
        stage_paint.set_anti_alias(true);
        FontManager::global().draw_text_cached(
            canvas,
            stage_label,
            (ox + 28.0 * scale, oy + 46.0 * scale),
            body_size,
            FontStyle::normal(),
            &stage_paint,
            false,
            w - 56.0 * scale,
        );

        let mut body_paint = Paint::default();
        body_paint.set_color(Color::from_argb(alpha, 220, 220, 220));
        body_paint.set_anti_alias(true);
        let text_width = w - 56.0 * scale;
        let line_height = body_size * 1.45;
        let content_top = oy + 72.0 * scale;
        let content_bottom = oy + h
            - if selected.approval.is_some() {
                70.0 * scale
            } else {
                20.0 * scale
            };
        let content = approval_content(selected).unwrap_or_else(|| selected.full_text.clone());
        let wrapped = wrap_text_lines(&content, body_size, text_width);
        let clamped_scroll = detail_scroll.clamp(0.0, detail_scroll_limit(selected, w, h, scale));

        for (idx, line) in wrapped.iter().enumerate() {
            let line_y = content_top + idx as f32 * line_height - clamped_scroll;
            if line_y < content_top - line_height || line_y > content_bottom {
                continue;
            }
            FontManager::global().draw_text_cached(
                canvas,
                line,
                (ox + 28.0 * scale, line_y),
                body_size,
                FontStyle::normal(),
                &body_paint,
                false,
                text_width,
            );
        }
        return;
    }

    if entries.is_empty() {
        let mut text_paint = Paint::default();
        text_paint.set_color(Color::from_argb(alpha, 150, 150, 150));
        text_paint.set_anti_alias(true);
        let text = "Claude Code - 空闲";
        let text_width =
            FontManager::global().measure_text_cached(text, 11.0 * scale, FontStyle::normal());
        let text_x = ox + w / 2.0 - text_width / 2.0;
        let text_y = oy + h / 2.0;
        FontManager::global().draw_text_cached(
            canvas,
            text,
            (text_x, text_y),
            11.0 * scale,
            FontStyle::normal(),
            &text_paint,
            false,
            w - 20.0 * scale,
        );
        return;
    }

    let row_h = 42.0 * scale;
    let start_y = oy + 24.0 * scale;
    for (idx, entry) in entries.iter().take(4).enumerate() {
        let row_y = start_y + idx as f32 * row_h;
        let stage_color = match entry.stage {
            ClaudeDisplayStage::Ready => Color::from_argb(alpha, 74, 222, 128),
            ClaudeDisplayStage::Running => Color::from_argb(alpha, 147, 130, 255),
            ClaudeDisplayStage::IdleOrOther => Color::from_argb(alpha, 150, 150, 150),
        };

        let mut dot_paint = Paint::default();
        dot_paint.set_color(stage_color);
        dot_paint.set_anti_alias(true);
        canvas.draw_circle((ox + 28.0 * scale, row_y), 4.0 * scale, &dot_paint);

        let mut title_paint = Paint::default();
        title_paint.set_color(Color::from_argb(alpha, 255, 255, 255));
        title_paint.set_anti_alias(true);
        FontManager::global().draw_text_cached(
            canvas,
            &entry.project,
            (ox + 40.0 * scale, row_y + 4.0 * scale),
            title_size,
            FontStyle::normal(),
            &title_paint,
            false,
            w - 68.0 * scale,
        );

        let mut preview_paint = Paint::default();
        preview_paint.set_color(Color::from_argb((alpha as f32 * 0.78) as u8, 210, 210, 210));
        preview_paint.set_anti_alias(true);
        FontManager::global().draw_text_cached(
            canvas,
            &entry.preview_text,
            (ox + 40.0 * scale, row_y + 18.0 * scale),
            body_size,
            FontStyle::normal(),
            &preview_paint,
            false,
            w - 68.0 * scale,
        );
    }
}

fn approval_content(entry: &ClaudeDisplayEntry) -> Option<String> {
    entry.approval.as_ref().map(|approval| {
        let mut parts = vec![approval.request.detail.clone()];
        if parts[0].trim().is_empty() {
            parts[0] = approval.request.title.clone();
        }
        parts.join("\n")
    })
}

fn draw_permission_buttons(
    canvas: &Canvas,
    ox: f32,
    oy: f32,
    alpha: u8,
    scale: f32,
    actions: &[ClaudePermissionActionButton],
    w: f32,
    h: f32,
) {
    let rects = permission_button_rects(w, h, scale);
    for (idx, action) in actions.iter().take(rects.len()).enumerate() {
        draw_button(canvas, ox, oy, alpha, scale, rects[idx], &action.label);
    }
}

fn draw_choice_buttons(
    canvas: &Canvas,
    ox: f32,
    oy: f32,
    alpha: u8,
    scale: f32,
    buttons: &[ChoiceButtonRect],
) {
    for button in buttons {
        draw_button(canvas, ox, oy, alpha, scale, button.rect, &button.label);
    }
}

fn draw_button(canvas: &Canvas, ox: f32, oy: f32, alpha: u8, scale: f32, rect: Rect, label: &str) {
    let translated = rect.with_offset((ox, oy));

    let mut fill = Paint::default();
    fill.set_color(Color::from_argb((alpha as f32 * 0.22) as u8, 255, 255, 255));
    fill.set_anti_alias(true);
    canvas.draw_round_rect(translated, 10.0 * scale, 10.0 * scale, &fill);

    let mut border = Paint::default();
    border.set_style(skia_safe::paint::Style::Stroke);
    border.set_stroke_width(1.0 * scale);
    border.set_color(Color::from_argb((alpha as f32 * 0.35) as u8, 255, 255, 255));
    border.set_anti_alias(true);
    canvas.draw_round_rect(translated, 10.0 * scale, 10.0 * scale, &border);

    let mut text = Paint::default();
    text.set_color(Color::from_argb(alpha, 255, 255, 255));
    text.set_anti_alias(true);
    FontManager::global().draw_text_cached(
        canvas,
        label,
        (
            translated.left + 12.0 * scale,
            translated.top + 20.0 * scale,
        ),
        10.0 * scale,
        FontStyle::normal(),
        &text,
        false,
        translated.width() - 24.0 * scale,
    );
}

fn wrap_text_lines(text: &str, size: f32, max_width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let trimmed = paragraph.trim();
        if trimmed.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current = String::new();
        for word in trimmed.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{} {}", current, word)
            };
            if FontManager::global().measure_text_cached(&candidate, size, FontStyle::normal())
                <= max_width
            {
                current = candidate;
                continue;
            }

            if !current.is_empty() {
                lines.push(current);
            }

            if FontManager::global().measure_text_cached(word, size, FontStyle::normal())
                <= max_width
            {
                current = word.to_string();
                continue;
            }

            let mut chunk = String::new();
            for ch in word.chars() {
                let candidate = format!("{}{}", chunk, ch);
                if !chunk.is_empty()
                    && FontManager::global().measure_text_cached(
                        &candidate,
                        size,
                        FontStyle::normal(),
                    ) > max_width
                {
                    lines.push(chunk);
                    chunk = ch.to_string();
                } else {
                    chunk = candidate;
                }
            }
            current = chunk;
        }

        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::choice_button_rects;
    use crate::window::app::ClaudeChoiceButton;

    #[test]
    fn choice_prompt_buttons_stack_vertically_in_original_order() {
        let buttons = choice_button_rects(
            &[
                ClaudeChoiceButton {
                    label: "Inline".to_string(),
                    value: "Inline".to_string(),
                },
                ClaudeChoiceButton {
                    label: "Subagent".to_string(),
                    value: "Subagent".to_string(),
                },
                ClaudeChoiceButton {
                    label: "Skip".to_string(),
                    value: "Skip".to_string(),
                },
            ],
            300.0,
            250.0,
            1.0,
            0.0,
        );

        assert_eq!(buttons.len(), 3);
        assert_eq!(buttons[0].label, "Inline");
        assert_eq!(buttons[1].label, "Subagent");
        assert_eq!(buttons[2].label, "Skip");
        assert!(buttons[1].rect.top > buttons[0].rect.top);
        assert!(buttons[2].rect.top > buttons[1].rect.top);
    }

    #[test]
    fn choice_prompt_buttons_scroll_into_view_instead_of_shrinking() {
        // Use 5 buttons in a height that fits at most 3 visible, so scrolling
        // reveals later buttons instead of shrinking them.
        let all_buttons = &[
            ClaudeChoiceButton {
                label: "Fresh 1".to_string(),
                value: "Fresh 1".to_string(),
            },
            ClaudeChoiceButton {
                label: "Fresh 2".to_string(),
                value: "Fresh 2".to_string(),
            },
            ClaudeChoiceButton {
                label: "Fresh 3".to_string(),
                value: "Fresh 3".to_string(),
            },
            ClaudeChoiceButton {
                label: "Fresh 4".to_string(),
                value: "Fresh 4".to_string(),
            },
            ClaudeChoiceButton {
                label: "Fresh 5".to_string(),
                value: "Fresh 5".to_string(),
            },
        ];
        // h=200: 5 buttons * (30+8) - 8 = 218 total, visible area 200-76-18 = 106 => fits ~2.7
        let initial = choice_button_rects(all_buttons, 300.0, 200.0, 1.0, 0.0);
        // scrolled by 120px
        let scrolled = choice_button_rects(all_buttons, 300.0, 200.0, 1.0, 120.0);

        assert!(initial.len() >= 2, "should show at least 2 buttons without scroll");
        assert!(
            initial
                .iter()
                .all(|button| (button.rect.height() - 30.0).abs() < f32::EPSILON)
        );
        assert_eq!(
            scrolled.last().map(|button| button.label.as_str()),
            Some("Fresh 5")
        );
        assert!(scrolled.iter().all(|button| button.rect.top <= 182.0));
        assert!(scrolled.iter().all(|button| button.rect.bottom >= 76.0));
    }
}
