//! 会话页：一行一场，点整行打开对应软件。
use crate::agent_session::{Phase, Row, Source};
use crate::app::{App, Cmd, SESSION_PAGE};
use crate::paint::{Card, H};
use crate::theme::{self, W};
use eframe::egui::{self, Color32};

const TOP: f32 = 40.0;
const BOTTOM: f32 = 214.0;
const ROW: f32 = 34.0;

fn phase_color(phase: Phase) -> Color32 {
    match phase {
        Phase::NeedsYou => theme::ORANGE,
        Phase::Errored => theme::RED,
        Phase::Running => theme::GREEN,
        Phase::JustFinished => theme::GRAY,
    }
}

fn source_bytes(source: Source) -> &'static [u8] {
    match source {
        Source::Cursor => include_bytes!("../assets/ai-icons/cursor.svg"),
        Source::Claude => include_bytes!("../assets/ai-icons/claude.svg"),
        Source::Codex => include_bytes!("../assets/ai-icons/codex.svg"),
        Source::WorkBuddy => include_bytes!("../assets/ai-icons/workbuddy.svg"),
        Source::DeepSeekHarness => include_bytes!("../assets/ai-icons/deepseek.svg"),
        Source::Antigravity => include_bytes!("../assets/ai-icons/gemini.svg"),
    }
}

/// 会话行和常驻标记共用的厂商图标。WorkBuddy 保留原色，其余压成浅色标识。
pub(crate) fn source_icon(c: &Card, source: Source, x: f32, y: f32, size: f32) {
    let ctx = c.ui.ctx();
    let ppp = ctx.pixels_per_point();
    let pixels = (size * c.s * ppp).round().max(1.0) as u32;
    let key = egui::Id::new(("session-logo", source.name(), pixels));
    let texture = ctx.data_mut(|data| data.get_temp::<egui::TextureHandle>(key)).or_else(|| {
        let mut image = egui_extras::image::load_svg_bytes_with_size(source_bytes(source), Some(egui::load::SizeHint::Size(pixels, pixels))).ok()?;
        let brand = matches!(source, Source::WorkBuddy | Source::DeepSeekHarness);
        for pixel in &mut image.pixels {
            if source == Source::WorkBuddy { continue; }
            *pixel = if source == Source::DeepSeekHarness {
                Color32::from_rgba_unmultiplied(77, 107, 254, pixel.a())
            } else {
                Color32::from_white_alpha(pixel.a())
            };
        }
        let texture = ctx.load_texture(format!("session-logo-{}-{pixels}", source.name()), image, egui::TextureOptions::LINEAR);
        if brand { crate::skin_paint::mark_colored(texture.id()); } else { crate::skin_paint::mark_monochrome(texture.id()); }
        ctx.data_mut(|data| data.insert_temp(key, texture.clone()));
        Some(texture)
    });
    let Some(texture) = texture else { return; };
    let center = c.pos(x, y);
    let half = pixels as f32 / 2.0;
    let min = egui::pos2((center.x * ppp - half).round() / ppp, (center.y * ppp - half).round() / ppp);
    c.painter.image(texture.id(), egui::Rect::from_min_size(min, egui::Vec2::splat(pixels as f32 / ppp)), egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
}

impl App {
    pub(crate) fn session_visible_rows(&self) -> usize {
        ((BOTTOM - TOP) / ROW).floor() as usize
    }

    pub(crate) fn session_scroll_max(&self) -> usize {
        self.sessions.rows.len().saturating_sub(self.session_visible_rows())
    }

    pub(crate) fn page_sessions(&mut self, c: &Card, cmds: &mut Vec<Cmd>) {
        if self.sessions.rows.is_empty() {
            c.text(209.5, 120.0, "现在没有在跑的会话", 12.0, W::Regular, theme::GRAY, H::Center);
            return;
        }
        let max = self.session_scroll_max();
        self.sessions.first = self.sessions.first.min(max);
        let start = self.sessions.first;
        let end = (start + self.session_visible_rows()).min(self.sessions.rows.len());
        let rows = self.sessions.rows[start..end].to_vec();
        let mut opened = None;
        for (offset, row) in rows.iter().enumerate() {
            let y = TOP + offset as f32 * ROW;
            if session_row(c, y, row, cmds) { opened = Some(row.id.clone()); }
        }
        if let Some(id) = opened { self.sessions.dismiss(&id); }
    }

    pub(crate) fn open_session_page(&mut self) {
        self.sessions.first = 0;
        self.show_page(SESSION_PAGE);
    }

    pub(crate) fn session_settings(&mut self, ui: &mut eframe::egui::Ui) {
        crate::extras::settings_label(ui, "刚跑完");
        crate::extras::settings_card(ui, |ui| {
            ui.label("正常结束后留在列表里。在岛上点开这一行，或者到了这个时间，才消失。报错停了也按这个时间收起。");
            let mut mins = self.settings.session_keep_mins;
            if ui.add(eframe::egui::Slider::new(&mut mins, 1..=60).suffix(" 分钟")).changed() {
                self.settings.session_keep_mins = mins;
            }
        });
        crate::extras::settings_label(ui, "读取器");
        crate::extras::settings_card(ui, |ui| {
            let sources = crate::agent_session::Source::ALL;
            for (index, source) in sources.iter().enumerate() {
                if index > 0 { ui.separator(); }
                let mut on = source.enabled(self.settings.session_sources);
                crate::extras::setting_row(ui, source.name(), "读取这一家留在本机的会话", &mut on);
                if on { self.settings.session_sources |= source.bit(); }
                else { self.settings.session_sources &= !source.bit(); }
            }
        });
    }
}

fn session_row(c: &Card, y: f32, row: &Row, cmds: &mut Vec<Cmd>) -> bool {
    let hit = c.hit(12.0, y, 395.0, ROW - 4.0, ("session-row", row.id.as_str()));
    if hit.hovered() { c.fill(12.0, y, 395.0, ROW - 4.0, 8.0, Color32::from_white_alpha(16)); }
    let opened = hit.clicked();
    if opened { cmds.push(Cmd::OpenSession(row.launch.clone())); }
    source_icon(c, row.source, 24.0, y + 14.0, 16.0);
    let title = c.fit(&row.title, 12.0, W::Semibold, 168.0);
    c.text(40.0, y + 13.0, &title, 12.0, W::Semibold, theme::WHITE, H::Left);
    let phase = row.phase.label();
    c.text(214.0, y + 13.0, phase, 10.5, W::Semibold, phase_color(row.phase), H::Left);
    let detail = if row.children > 0 { format!("{} · {} 个在跑", row.step, row.children) } else { row.step.clone() };
    let detail = if detail.trim().is_empty() { row.project.clone() } else if row.project.is_empty() { detail } else { format!("{detail} · {}", row.project) };
    c.text(40.0, y + 27.0, &c.fit(&detail, 10.0, W::Regular, 280.0), 10.0, W::Regular, theme::GRAY, H::Left);
    if let Some(usage) = &row.usage {
        c.text(396.0, y + 16.0, &c.fit(usage, 10.5, W::Semibold, 52.0), 10.5, W::Semibold, theme::GRAY, H::Right);
    }
    let _ = hit.on_hover_text(format!("{} · {}", row.source.name(), row.phase.label()));
    opened
}
