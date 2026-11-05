//! Shared desktop styling; follows the operating system's light/dark preference.
use eframe::egui::{self, Color32, FontId, RichText, Stroke, TextStyle};

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub background: Color32,
    pub card: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub border: Color32,
    pub soft: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub positive: Color32,
    pub caution: Color32,
    pub danger: Color32,
}
impl Palette {
    pub fn new(dark: bool) -> Self {
        let rgb = Color32::from_rgb;
        if dark {
            Self {
                background: rgb(18, 23, 32),
                card: rgb(26, 33, 45),
                text: rgb(233, 238, 246),
                muted: rgb(153, 167, 187),
                border: rgb(47, 59, 77),
                soft: rgb(34, 43, 58),
                accent: rgb(88, 155, 255),
                on_accent: rgb(15, 26, 40),
                positive: rgb(88, 200, 153),
                caution: rgb(239, 190, 98),
                danger: rgb(251, 131, 134),
            }
        } else {
            Self {
                background: rgb(245, 247, 250),
                card: Color32::WHITE,
                text: rgb(26, 36, 51),
                muted: rgb(100, 112, 129),
                border: rgb(226, 232, 240),
                soft: rgb(241, 245, 249),
                accent: rgb(37, 99, 235),
                on_accent: Color32::WHITE,
                positive: rgb(22, 130, 90),
                caution: rgb(165, 104, 23),
                danger: rgb(190, 54, 58),
            }
        }
    }
}

pub(super) fn configure(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::System);
    ctx.all_styles_mut(|style| {
        let p = Palette::new(style.visuals.dark_mode);
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 9.0);
        style.spacing.interact_size.y = 34.0;
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(12.0));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(19.0));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(14.0));
        style.visuals.override_text_color = Some(p.text);
        style.visuals.panel_fill = p.background;
        style.visuals.extreme_bg_color = p.soft;
        style.visuals.faint_bg_color = p.soft;
        style.visuals.code_bg_color = p.soft;
        style.visuals.selection.bg_fill = p.accent.gamma_multiply(0.3);
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
        style.visuals.widgets.inactive.bg_fill = p.soft;
        style.visuals.widgets.inactive.weak_bg_fill = p.soft;
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
        style.visuals.widgets.hovered.bg_fill = p.soft;
        style.visuals.widgets.hovered.weak_bg_fill = p.soft;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
        for widget in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
        ] {
            widget.corner_radius = 8.into();
            widget.fg_stroke.color = p.text;
        }
    });
}

pub(super) fn card(ui: &mut egui::Ui, p: Palette, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(14)
        .inner_margin(20)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui);
        });
}

pub(super) fn caption(ui: &mut egui::Ui, text: &str, p: Palette) {
    ui.label(RichText::new(text).size(12.0).color(p.muted));
}

pub(super) fn divider(ui: &mut egui::Ui) {
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
}

pub(super) fn switch_row(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    value: &mut bool,
    p: Palette,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let text_width = (ui.available_width() - 58.0).max(0.0);
        ui.allocate_ui_with_layout(
            egui::vec2(text_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(RichText::new(title).strong());
                caption(ui, description, p);
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, mut response) =
                ui.allocate_exact_size(egui::vec2(40.0, 24.0), egui::Sense::click());
            if response.clicked() {
                *value = !*value;
                response.mark_changed();
            }
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *value, title)
            });
            let t = ui.ctx().animate_bool_with_time(response.id, *value, 0.14);
            let fill = if *value { p.accent } else { p.border };
            ui.painter().rect_filled(rect, 12, fill);
            ui.painter().circle_filled(
                egui::pos2(rect.left() + 12.0 + t * 16.0, rect.center().y),
                9.0,
                Color32::WHITE,
            );
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect.expand(3.0),
                    15,
                    Stroke::new(1.5, p.accent),
                    egui::StrokeKind::Outside,
                );
            }
            changed = response.changed();
            response.on_hover_text(title);
        });
    });
    changed
}

pub(super) fn header(ui: &mut egui::Ui, p: Palette) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 12, Color32::from_rgb(32, 130, 220));
        let origin = rect.min;
        for [x, y, w, h] in [
            [11.0, 10.0, 22.0, 5.0],
            [11.0, 20.0, 22.0, 5.0],
            [11.0, 30.0, 22.0, 5.0],
            [11.0, 10.0, 5.0, 15.0],
            [28.0, 20.0, 5.0, 15.0],
        ] {
            painter.rect_filled(
                egui::Rect::from_min_size(origin + egui::vec2(x, y), egui::vec2(w, h)),
                1,
                Color32::WHITE,
            );
        }
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new("Sidekick").size(26.0).strong());
            caption(ui, "Your computer. Your controls.", p);
        });
    });
}
