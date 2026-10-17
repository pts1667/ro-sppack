//! The launcher's home screen (background art, world card, update bar and PLAY) and the settings
//! menu drawn over it. The window has no system frame, so this also draws the title bar.

use super::*;
use egui::{Align2, FontId, Rect, Sense, Stroke, UiBuilder, pos2, vec2};

const GOLD: Color32 = Color32::from_rgb(0xF0, 0xC2, 0x3C);
const GOLD_DEEP: Color32 = Color32::from_rgb(0xC0, 0x8A, 0x16);
const INK: Color32 = Color32::from_rgb(0x1C, 0x16, 0x0A);
const TEXT: Color32 = Color32::from_gray(0xE8);
const TEXT_DIM: Color32 = Color32::from_gray(0xA8);
const TITLE_BAR_HEIGHT: f32 = 40.0;
const MARGIN: f32 = 36.0;
const PLAY_SIZE: egui::Vec2 = egui::Vec2::new(250.0, 62.0);

/// Dark visuals with gold accents, matching the button and the progress bar.
pub(super) fn apply_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.selection.bg_fill = Color32::from_rgb(0x7A, 0x5A, 0x16);
    visuals.selection.stroke = Stroke::new(1.0_f32, GOLD);
    visuals.hyperlink_color = GOLD;
    visuals.panel_fill = Color32::from_rgb(0x15, 0x16, 0x1B);
    visuals.window_fill = visuals.panel_fill;
    visuals.extreme_bg_color = Color32::from_rgb(0x0D, 0x0E, 0x12);
    ctx.set_visuals(visuals);
}

/// The background art, decoded once and kept as a texture.
pub(super) fn load_background(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let bytes = include_bytes!("../../res/background.webp");
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
        .ok()?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture("background", pixels, egui::TextureOptions::LINEAR))
}

/// What the update bar shows.
struct UpdateBar {
    status: String,
    color: Color32,
    fraction: f32,
    detail: String,
    elapsed: String,
}

/// What pressing the big button does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PlayAction {
    Play,
    Install,
    Nothing,
}

impl App {
    pub(super) fn window(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(0x0C, 0x0C, 0x10)))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                self.paint_background(ui, full);
                let (title, body) = full.split_top_bottom_at_y(full.top() + TITLE_BAR_HEIGHT);
                match self.menu {
                    Some(page) => self.menu_ui(ui, body, page),
                    None => self.home(ui, body),
                }
                self.title_bar(ui, title);
                ui.painter().rect_stroke(
                    full.shrink(0.5),
                    0.0,
                    Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
                );
            });
    }

    fn paint_background(&self, ui: &egui::Ui, full: Rect) {
        let painter = ui.painter();
        if let Some(texture) = &self.background {
            painter.image(
                texture.id(),
                full,
                cover_uv(texture.size_vec2(), full.size()),
                Color32::WHITE,
            );
        }
        // Darken the title bar, the left (world card) and the bottom (update bar), as launchers do.
        painter.rect_filled(
            Rect::from_min_size(full.min, vec2(full.width(), TITLE_BAR_HEIGHT)),
            0.0,
            Color32::from_black_alpha(170),
        );
        gradient(
            painter,
            Rect::from_min_max(
                full.min,
                pos2(full.left() + full.width() * 0.5, full.bottom()),
            ),
            Color32::from_black_alpha(150),
            Color32::TRANSPARENT,
            false,
        );
        let fade_top = full.top() + full.height() * 0.45;
        let fade_mid = full.bottom() - 175.0;
        gradient(
            painter,
            Rect::from_min_max(pos2(full.left(), fade_top), pos2(full.right(), fade_mid)),
            Color32::TRANSPARENT,
            Color32::from_rgba_unmultiplied(8, 8, 12, 215),
            true,
        );
        gradient(
            painter,
            Rect::from_min_max(pos2(full.left(), fade_mid), full.max),
            Color32::from_rgba_unmultiplied(8, 8, 12, 215),
            Color32::from_rgba_unmultiplied(8, 8, 12, 250),
            true,
        );
        if self.menu.is_some() {
            painter.rect_filled(full, 0.0, Color32::from_rgba_unmultiplied(8, 8, 12, 200));
        }
    }

    // -- title bar ---------------------------------------------------------------

    fn title_bar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let drag = ui.interact(rect, egui::Id::new("title_bar"), Sense::click_and_drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let painter = ui.painter();
        let y = rect.center().y;
        let mut x = rect.left() + 20.0;
        x = painter
            .text(
                pos2(x, y),
                Align2::LEFT_CENTER,
                "ro-sppack",
                FontId::proportional(16.0),
                GOLD,
            )
            .right()
            + 14.0;
        painter.vline(
            x,
            (y - 8.0)..=(y + 8.0),
            Stroke::new(1.0_f32, Color32::from_white_alpha(60)),
        );
        x += 14.0;
        x = painter
            .text(
                pos2(x, y),
                Align2::LEFT_CENTER,
                "Game Status:",
                FontId::proportional(13.0),
                TEXT_DIM,
            )
            .right()
            + 6.0;
        let (status, color) = self.game_status();
        painter.text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            status,
            FontId::proportional(13.0),
            color,
        );

        // Window buttons, right to left.
        let size = vec2(40.0, rect.height());
        let close = Rect::from_min_size(pos2(rect.right() - size.x, rect.top()), size);
        let minimize = close.translate(vec2(-size.x, 0.0));
        if chrome_button(ui, close, "close", Chrome::Close).clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if chrome_button(ui, minimize, "minimize", Chrome::Minimize).clicked() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        let mut right = minimize.left() - 10.0;
        for (label, page) in [("Logs", Page::Logs), ("Settings", Page::General)] {
            let galley =
                ui.painter()
                    .layout_no_wrap(label.to_string(), FontId::proportional(14.0), TEXT);
            let button = Rect::from_min_max(
                pos2(right - galley.size().x - 24.0, rect.top() + 7.0),
                pos2(right, rect.bottom() - 7.0),
            );
            right = button.left() - 4.0;
            let active = match self.menu {
                Some(Page::Logs) => page == Page::Logs,
                Some(_) => page == Page::General,
                None => false,
            };
            let response = ui.interact(button, egui::Id::new(("title", label)), Sense::click());
            let fill = if active {
                Color32::from_rgba_unmultiplied(240, 194, 60, 50)
            } else if response.hovered() {
                Color32::from_white_alpha(22)
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(button, 4.0, fill);
            ui.painter().galley(
                button.center() - galley.size() / 2.0,
                galley,
                if active { GOLD } else { TEXT },
            );
            if response.clicked() {
                self.menu = if active { None } else { Some(page) };
            }
        }
    }

    fn game_status(&self) -> (&'static str, Color32) {
        if self.server_stopping {
            ("Stopping…", WARN)
        } else if self.client_pid.is_some() {
            ("Playing", OK)
        } else if self.server_pid.is_some() && self.server_ready {
            ("Server online", OK)
        } else if self.server_pid.is_some() {
            ("Server starting…", WARN)
        } else {
            ("Game is not running", TEXT)
        }
    }

    // -- home ----------------------------------------------------------------------

    fn home(&mut self, ui: &mut egui::Ui, body: Rect) {
        let card = Rect::from_min_size(
            pos2(body.left() + MARGIN, body.top() + 64.0),
            vec2(330.0, 0.0),
        );
        ui.allocate_new_ui(
            UiBuilder::new().max_rect(card.with_max_y(body.bottom())),
            |ui| {
                self.world_card(ui);
            },
        );

        let play = Rect::from_min_size(
            pos2(
                body.right() - MARGIN - PLAY_SIZE.x,
                body.bottom() - 40.0 - PLAY_SIZE.y,
            ),
            PLAY_SIZE,
        );
        self.update_bar(
            ui,
            Rect::from_min_max(
                pos2(body.left() + MARGIN, play.top()),
                pos2(play.left() - 40.0, play.bottom()),
            ),
        );
        self.play_area(ui, play);
    }

    /// Server, bots and rates at a glance, where launchers show their news.
    fn world_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgba_unmultiplied(12, 12, 18, 205))
            .stroke(Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(240, 194, 60, 70),
            ))
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(18.0, 14.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("WORLD").size(12.0).color(GOLD));
                ui.add_space(2.0);
                ui.label(RichText::new("Ragnarok Online").size(20.0).color(TEXT));
                ui.add_space(8.0);

                let settings = &self.settings;
                let (server, server_color) = self.game_status();
                let (bots, bots_color) = self.bots_status();
                let rates = format!(
                    "Base {}× · Job {}× · Drop {}×",
                    settings.base_exp_rate, settings.job_exp_rate, settings.drop_rate
                );
                egui::Grid::new("world")
                    .num_columns(2)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Server").color(TEXT_DIM));
                        ui.label(RichText::new(server).color(server_color));
                        ui.end_row();
                        ui.label(RichText::new("Bots").color(TEXT_DIM));
                        ui.label(RichText::new(bots).color(bots_color));
                        ui.end_row();
                        ui.label(RichText::new("Rates").color(TEXT_DIM));
                        ui.label(RichText::new(rates).color(TEXT));
                        ui.end_row();
                    });

                if self.any_process() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.server_stopping, egui::Button::new("Stop all"))
                            .on_hover_text(
                                "Logs the bots out, then stops the server and the client",
                            )
                            .clicked()
                        {
                            self.stop_all();
                        }
                        if ui.button("Processes").clicked() {
                            self.menu = Some(Page::Processes);
                        }
                    });
                }
            });
    }

    fn bots_status(&self) -> (String, Color32) {
        let count = self.settings.bot_count;
        if self.bot_pid.is_some() && self.bots_stopping {
            ("Logging out…".into(), WARN)
        } else if self.bot_pid.is_some() {
            (format!("{count} playing"), OK)
        } else if self.bots_waiting {
            ("Waiting for the server…".into(), WARN)
        } else if !self.settings.launch_bots || !self.settings.bot_api_enabled {
            ("Off".into(), TEXT_DIM)
        } else {
            (format!("{count} join with the server"), TEXT)
        }
    }

    // -- update bar ------------------------------------------------------------------

    fn update_bar(&self, ui: &mut egui::Ui, area: Rect) {
        let bar = self.update_bar_state();
        let painter = ui.painter();
        let track = Rect::from_min_max(
            pos2(area.left(), area.center().y - 2.0),
            pos2(area.right(), area.center().y + 8.0),
        );

        painter.text(
            pos2(track.left(), track.top() - 12.0),
            Align2::LEFT_BOTTOM,
            &bar.status,
            FontId::proportional(15.0),
            bar.color,
        );

        painter.rect_filled(track, 3.0, Color32::from_white_alpha(26));
        let fraction = bar.fraction.clamp(0.0, 1.0);
        if fraction > 0.0 {
            let filled = Rect::from_min_max(
                track.min,
                pos2(track.lerp_inside(vec2(fraction, 0.0)).x, track.bottom()),
            );
            painter.rect_filled(filled, 3.0, GOLD);
            // A light sweep while something is running.
            if self.runner.is_running() {
                let time = ui.input(|input| input.time) as f32;
                let sweep = (time * 0.6).fract();
                let x = filled.left() + filled.width() * sweep;
                let glow = Rect::from_min_max(
                    pos2((x - 40.0).max(filled.left()), filled.top()),
                    pos2((x + 40.0).min(filled.right()), filled.bottom()),
                );
                gradient(
                    painter,
                    glow.with_max_x(x.max(glow.left())),
                    Color32::TRANSPARENT,
                    Color32::from_white_alpha(90),
                    false,
                );
                gradient(
                    painter,
                    glow.with_min_x(x.min(glow.right())),
                    Color32::from_white_alpha(90),
                    Color32::TRANSPARENT,
                    false,
                );
                ui.ctx().request_repaint_after(Duration::from_millis(33));
            }
        }

        painter.text(
            pos2(track.left(), track.bottom() + 12.0),
            Align2::LEFT_TOP,
            &bar.detail,
            FontId::proportional(13.0),
            TEXT_DIM,
        );
        painter.text(
            pos2(track.right(), track.bottom() + 12.0),
            Align2::RIGHT_TOP,
            &bar.elapsed,
            FontId::proportional(13.0),
            TEXT_DIM,
        );
    }

    fn update_bar_state(&self) -> UpdateBar {
        let complete = self.setup_complete();
        if let Some(job) = &self.runner.current {
            let mut total = self.batch_done + 1 + self.queue.len();
            if self.update == UpdateState::Pulling {
                total += crate::jobs::update_build_plan(&self.settings).len();
            }
            let verb = match self.update {
                UpdateState::Pulling => "Checking for updates",
                UpdateState::Building if !complete => "Installing",
                UpdateState::Building => "Updating",
                _ => "Working",
            };
            let note = if self.runner.progress_note.is_empty() {
                &self.job_line
            } else {
                &self.runner.progress_note
            };
            return UpdateBar {
                status: format!("{verb}… {}", job.label),
                color: TEXT,
                fraction: (self.batch_done as f32 + self.job_fraction()) / total as f32,
                detail: shorten(
                    &format!("Step {} of {total} · {note}", self.batch_done + 1),
                    96,
                ),
                elapsed: self
                    .job_started
                    .map(|started| {
                        let seconds = started.elapsed().as_secs();
                        format!("{}:{:02}", seconds / 60, seconds % 60)
                    })
                    .unwrap_or_default(),
            };
        }
        let (status, color, fraction, detail) = match &self.update {
            UpdateState::Failed(error) => (
                shorten(&format!("Update failed — {error}"), 90),
                BAD,
                0.0,
                if complete {
                    "Details in Logs. PLAY starts the version that is installed."
                } else {
                    "Details in Logs. INSTALL tries again."
                },
            ),
            _ if !complete && self.settings.assets_dir.as_os_str().is_empty() => (
                "Choose your Ragnarok Online folder in Settings, then press INSTALL.".into(),
                WARN,
                0.0,
                "It is the folder that holds DATA.INI and the .grf archives.",
            ),
            _ if !complete => (
                "The game is not fully installed yet.".into(),
                WARN,
                0.0,
                "INSTALL finishes it. Settings → Components shows what is missing.",
            ),
            UpdateState::Done { changed: true } => (
                "Update installed. Your game is up to date.".into(),
                TEXT,
                1.0,
                "",
            ),
            UpdateState::Done { changed: false } => (
                "No update required. Your game is up to date.".into(),
                TEXT,
                1.0,
                "",
            ),
            UpdateState::Skipped | UpdateState::Idle if self.settings.skip_update => (
                "Playing without updating.".into(),
                TEXT,
                0.0,
                "Untick \"Play without updating\" to update now.",
            ),
            UpdateState::Skipped | UpdateState::Idle => (
                "Updates wait until the game is closed.".into(),
                TEXT,
                0.0,
                "",
            ),
            UpdateState::Pulling | UpdateState::Building => {
                ("Checking for updates…".into(), TEXT, 0.0, "")
            }
        };
        UpdateBar {
            status,
            color,
            fraction,
            detail: detail.into(),
            elapsed: String::new(),
        }
    }

    // -- PLAY ------------------------------------------------------------------------

    fn play_area(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let complete = self.setup_complete();

        let checkbox =
            Rect::from_min_size(rect.left_top() - vec2(0.0, 30.0), vec2(rect.width(), 24.0));
        ui.allocate_new_ui(UiBuilder::new().max_rect(checkbox), |ui| {
            let response = ui
                .add_enabled(
                    complete,
                    egui::Checkbox::new(
                        &mut self.settings.skip_update,
                        RichText::new("Play without updating")
                            .size(13.0)
                            .color(TEXT),
                    ),
                )
                .on_hover_text("Skips pulling and rebuilding the server, client and bots")
                .on_disabled_hover_text("The game has to be installed first");
            if response.changed() && self.settings.skip_update {
                self.skip_update_now();
            }
        });

        let (label, action) = if self.update.running() {
            (
                if complete {
                    "UPDATING…"
                } else {
                    "INSTALLING…"
                },
                PlayAction::Nothing,
            )
        } else if self.server_stopping {
            ("STOPPING…", PlayAction::Nothing)
        } else if self.client_pid.is_some() {
            ("PLAYING", PlayAction::Nothing)
        } else if !complete {
            if self.runner.is_running() {
                ("INSTALLING…", PlayAction::Nothing)
            } else {
                ("INSTALL", PlayAction::Install)
            }
        } else if self.client_waiting {
            ("STARTING…", PlayAction::Nothing)
        } else {
            ("PLAY", PlayAction::Play)
        };

        let enabled = action != PlayAction::Nothing;
        let response = ui.interact(
            rect,
            egui::Id::new("play"),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let painter = ui.painter();
        if enabled {
            let glow = egui::epaint::Shadow {
                offset: egui::Vec2::ZERO,
                blur: if response.hovered() { 26.0 } else { 18.0 },
                spread: 0.0,
                color: Color32::from_rgba_unmultiplied(240, 194, 60, 70),
            };
            painter.add(glow.as_shape(rect, 6.0));
        }
        let (top, bottom, text) = if !enabled {
            (
                Color32::from_rgb(0x6A, 0x5E, 0x3E),
                Color32::from_rgb(0x4E, 0x44, 0x2C),
                Color32::from_rgb(0x2E, 0x28, 0x1A),
            )
        } else if response.is_pointer_button_down_on() {
            (GOLD_DEEP, GOLD_DEEP, INK)
        } else if response.hovered() {
            (Color32::from_rgb(0xFF, 0xD6, 0x5A), GOLD, INK)
        } else {
            (GOLD, GOLD_DEEP, INK)
        };
        painter.rect_filled(rect, 6.0, bottom);
        gradient(painter, rect.shrink(1.0), top, bottom, true);
        painter.rect_stroke(
            rect,
            6.0,
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 230, 150, 120)),
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(22.0),
            text,
        );
        if enabled {
            response
                .clone()
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }

        if !self.heads.is_empty() {
            painter.text(
                pos2(rect.right(), rect.bottom() + 10.0),
                Align2::RIGHT_TOP,
                &self.heads,
                FontId::proportional(11.5),
                TEXT_DIM,
            );
        }

        if response.clicked() {
            match action {
                PlayAction::Play => self.launch_game(),
                PlayAction::Install if self.settings.assets_dir.as_os_str().is_empty() => {
                    self.menu = Some(Page::General);
                }
                PlayAction::Install => self.start_update(),
                PlayAction::Nothing => {}
            }
        }
    }

    // -- settings menu -----------------------------------------------------------------

    fn menu_ui(&mut self, ui: &mut egui::Ui, body: Rect, page: Page) {
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.menu = None;
            return;
        }
        let panel = Rect::from_min_max(body.min + vec2(24.0, 16.0), body.max - vec2(24.0, 24.0));
        let painter = ui.painter();
        painter.add(
            egui::epaint::Shadow {
                offset: vec2(0.0, 8.0),
                blur: 30.0,
                spread: 0.0,
                color: Color32::from_black_alpha(160),
            }
            .as_shape(panel, 10.0),
        );
        painter.rect_filled(
            panel,
            10.0,
            Color32::from_rgba_unmultiplied(20, 21, 27, 248),
        );
        painter.rect_stroke(
            panel,
            10.0,
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(240, 194, 60, 60)),
        );

        let inner = panel.shrink(18.0);
        let nav = Rect::from_min_max(inner.min, pos2(inner.left() + 180.0, inner.bottom()));
        let content = Rect::from_min_max(pos2(nav.right() + 22.0, inner.top()), inner.max);
        painter.vline(
            nav.right() + 11.0,
            nav.y_range(),
            Stroke::new(1.0_f32, Color32::from_white_alpha(20)),
        );

        ui.allocate_new_ui(UiBuilder::new().max_rect(nav), |ui| {
            ui.label(RichText::new("Settings").size(20.0).color(GOLD));
            ui.add_space(10.0);
            for item in Page::ALL {
                let selected = item == page;
                let text = RichText::new(item.title()).size(14.5);
                if ui
                    .add_sized(
                        [nav.width(), 28.0],
                        egui::SelectableLabel::new(selected, text),
                    )
                    .clicked()
                {
                    self.menu = Some(item);
                }
            }
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);
            self.status_overview(ui);
        });

        ui.allocate_new_ui(UiBuilder::new().max_rect(content), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(page.title()).size(18.0).color(TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Back to launcher").on_hover_text("Esc").clicked() {
                        self.menu = None;
                    }
                });
            });
            ui.add_space(6.0);
            if page == Page::Logs {
                self.logs_tab(ui);
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt(("menu", page.title()))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    match page {
                        Page::General => self.general_page(ui),
                        Page::Server => self.server_page(ui),
                        Page::Components => self.components_page(ui),
                        Page::Processes => self.dashboard_tab(ui),
                        Page::Logs => {}
                    }
                    ui.add_space(12.0);
                });
        });
    }
}

#[derive(Clone, Copy)]
enum Chrome {
    Minimize,
    Close,
}

fn chrome_button(ui: &mut egui::Ui, rect: Rect, id: &str, kind: Chrome) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new(("chrome", id)), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        let fill = match kind {
            Chrome::Close => Color32::from_rgb(0xC4, 0x2B, 0x1C),
            Chrome::Minimize => Color32::from_white_alpha(24),
        };
        painter.rect_filled(rect, 0.0, fill);
    }
    let center = rect.center();
    let stroke = Stroke::new(1.2_f32, TEXT);
    match kind {
        Chrome::Minimize => {
            painter.hline((center.x - 6.0)..=(center.x + 6.0), center.y, stroke);
        }
        Chrome::Close => {
            let d = 5.5;
            painter.line_segment([center - vec2(d, d), center + vec2(d, d)], stroke);
            painter.line_segment([center + vec2(-d, d), center + vec2(d, -d)], stroke);
        }
    }
    response
}

/// The part of an image of `image` size that covers `area` without stretching, centred.
fn cover_uv(image: egui::Vec2, area: egui::Vec2) -> Rect {
    let image_aspect = image.x / image.y;
    let area_aspect = area.x / area.y;
    if area_aspect > image_aspect {
        let height = image_aspect / area_aspect;
        Rect::from_min_max(
            pos2(0.0, (1.0 - height) / 2.0),
            pos2(1.0, (1.0 + height) / 2.0),
        )
    } else {
        let width = area_aspect / image_aspect;
        Rect::from_min_max(
            pos2((1.0 - width) / 2.0, 0.0),
            pos2((1.0 + width) / 2.0, 1.0),
        )
    }
}

/// Fills `rect` with a linear gradient, top to bottom (`vertical`) or left to right.
fn gradient(painter: &egui::Painter, rect: Rect, from: Color32, to: Color32, vertical: bool) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let (top_right, bottom_left) = if vertical { (from, to) } else { (to, from) };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), from);
    mesh.colored_vertex(rect.right_top(), top_right);
    mesh.colored_vertex(rect.left_bottom(), bottom_left);
    mesh.colored_vertex(rect.right_bottom(), to);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(mesh);
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut short: String = text.chars().take(max - 1).collect();
    short.push('…');
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_background_covers_wider_and_taller_windows() {
        let wide = cover_uv(vec2(686.0, 386.0), vec2(1372.0, 386.0));
        assert_eq!(wide.width(), 1.0);
        assert!((wide.height() - 0.5).abs() < 1e-4 && (wide.center().y - 0.5).abs() < 1e-4);

        let tall = cover_uv(vec2(686.0, 386.0), vec2(343.0, 386.0));
        assert_eq!(tall.height(), 1.0);
        assert!((tall.width() - 0.5).abs() < 1e-4 && (tall.center().x - 0.5).abs() < 1e-4);
    }

    #[test]
    fn the_background_art_decodes() {
        let bytes = include_bytes!("../../res/background.webp");
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP).unwrap();
        assert_eq!((image.width(), image.height()), (686, 386));
    }

    #[test]
    fn long_messages_are_shortened() {
        assert_eq!(shorten("abc", 5), "abc");
        assert_eq!(shorten("abcdefgh", 5), "abcd…");
    }
}
