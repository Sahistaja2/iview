//! Seaded (P): lehed — avamine, vaatamine,
//! vähendamine, salvestamine, failid, liikumine, täisekraan. Muudatus jõuab
//! programmi alles „Salvesta" (Enter) peale; Esc loobub. Lehti vahetavad
//! ↑ ↓, kui fookus pole tekstiväljal.

use eframe::egui::{self, FontId, Key, RichText};

use crate::app::Palette;
use crate::config::{AtEnd, Background, Config, Corner, Enlarge, FullText, OpenView, RawSource, SortBy, TextSize, Transparent, Unsaved, WheelMode, ZoomStep};

const PAGES: [&str; 7] = ["Opening", "Viewing", "Resizing", "Saving", "Files", "Moving around", "Full screen"];

/// Muudetav koopia seadetest ja avatud leht.
pub struct Draft {
    pub config: Config,
    page: usize,
    vars_open: bool,
}

impl Draft {
    pub fn new(config: Config) -> Self {
        Self { config, page: 0, vars_open: false }
    }
}

/// Malli muutujad (täisekraani tekst).
const TEXT_VARS: [(&str, &str); 28] = [
    ("{name}", "name without the extension"),
    ("{ext}", "extension"),
    ("{folder}", "the picture's folder name"),
    ("{path}", "the whole path"),
    ("{date_folder}", "nearest folder with a date in its name"),
    ("{title}", "the date folder's name without the date and camera mark"),
    ("{index}", "place in the list"),
    ("{total}", "pictures in the list"),
    ("{width}", "width in pixels"),
    ("{height}", "height in pixels"),
    ("{size}", "file size → 8.4 MB"),
    ("{mp}", "megapixels → 24"),
    ("{ratio}", "aspect ratio → 3:2"),
    ("{zoom}", "zoom → 19 %"),
    ("{modified:%Y-%m-%d}", "file's modified time"),
    ("{now:%H:%M}", "time now"),
    ("{taken:%Y-%m-%d %H:%M}", "time taken (EXIF)"),
    ("{camera}", "camera (make and model)"),
    ("{lens}", "lens"),
    ("{focal}", "focal length → 35 mm"),
    ("{focal35}", "focal length as on 35 mm film"),
    ("{aperture}", "aperture → f/2.8"),
    ("{exposure}", "exposure → 1/250 s"),
    ("{iso}", "ISO → 400"),
    ("{position}", "GPS position"),
    ("{altitude} {yaw} {pitch}", "drone: altitude, direction, tilt"),
    ("{comment}", "picture comment / description"),
    ("{exif:Artist}", "any EXIF tag by name (Artist, Copyright …)"),
];

/// Seadete aken pildi kohal. `Some(true)`: salvesta, `Some(false)`: loobu.
pub fn show(ctx: &egui::Context, palette: &Palette, d: &mut Draft) -> Option<bool> {
    let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
    if escape {
        if d.vars_open {
            d.vars_open = false;
        } else {
            return Some(false);
        }
    }
    let typing = ctx.memory(|m| m.focused()).is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
    if !typing {
        let key = |k: Key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
        if key(Key::ArrowDown) {
            d.page = (d.page + 1) % PAGES.len();
        } else if key(Key::ArrowUp) {
            d.page = (d.page + PAGES.len() - 1) % PAGES.len();
        }
    }
    // Enter salvestab (ka tekstiväljal: üherealine väli, põhitoiming).
    let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
    let screen = ctx.content_rect();
    let width = (screen.width() - 40.0).clamp(420.0, 760.0);
    let body_h = (screen.height() - 200.0).clamp(240.0, 460.0);
    let mut result = None;
    egui::Window::new("Settings")
        .collapsible(false)
        .resizable(false)
        .title_bar(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([width, body_h + 60.0])
        .frame(egui::Frame::window(&ctx.global_style()).fill(palette.panel).stroke(egui::Stroke::new(1.0, palette.border)).corner_radius(4.0).inner_margin(14.0))
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(140.0);
                    for (i, title) in PAGES.iter().enumerate() {
                        let selected = i == d.page;
                        let text = RichText::new(*title).color(if selected { palette.text } else { palette.text_dim });
                        if ui.add_sized([140.0, 28.0], egui::Button::selectable(selected, text)).clicked() {
                            d.page = i;
                        }
                    }
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.set_min_height(body_h);
                    ui.label(RichText::new(PAGES[d.page]).size(17.0).color(palette.text));
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().id_salt(("settings-page", d.page)).max_height(body_h - 30.0).auto_shrink([false, false]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let c = &mut d.config;
                        match d.page {
                            0 => page_opening(ui, palette, c),
                            1 => page_viewing(ui, palette, c),
                            2 => page_resizing(ui, palette, c),
                            3 => page_saving(ui, palette, c),
                            4 => page_files(ui, palette, c),
                            5 => page_moving(ui, palette, c),
                            _ => page_fullscreen(ui, palette, c, &mut d.vars_open),
                        }
                    });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let save = ui.add(egui::Button::new(RichText::new("Save").color(egui::Color32::WHITE)).fill(palette.accent).min_size([96.0, 30.0].into()));
                    if save.clicked() {
                        result = Some(true);
                    }
                    if ui.add(egui::Button::new("Cancel").min_size([80.0, 30.0].into())).clicked() {
                        result = Some(false);
                    }
                });
            });
        });
    if enter && result.is_none() {
        result = Some(true);
    }
    result
}

/// Rida: silt vasakul (ühes tulbas), väli paremal.
fn row(ui: &mut egui::Ui, palette: &Palette, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 24.0], egui::Label::new(RichText::new(label).color(palette.text)).halign(egui::Align::RIGHT));
        ui.add_space(6.0);
        add(ui);
    });
}

fn note(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(156.0);
        ui.label(RichText::new(text).color(palette.text_faint).small());
    });
}

/// Valikuriba: võrdsed lahtrid, valitud esile tõstetud.
fn choice(ui: &mut egui::Ui, labels: &[&str], current: usize) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, label) in labels.iter().enumerate() {
            if ui.add(egui::Button::selectable(i == current, *label).min_size([72.0, 26.0].into())).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

fn switch(ui: &mut egui::Ui, palette: &Palette, on: &mut bool, label: &str, hint: &str) {
    ui.horizontal(|ui| {
        ui.add_space(156.0);
        ui.checkbox(on, RichText::new(label).color(palette.text));
        if !hint.is_empty() {
            ui.label(RichText::new(hint).color(palette.text_faint).small());
        }
    });
}

fn text_field(ui: &mut egui::Ui, text: &mut String, mono: bool) {
    let font = if mono { FontId::monospace(13.0) } else { FontId::proportional(14.0) };
    ui.add(egui::TextEdit::singleline(text).font(font).desired_width(ui.available_width().min(420.0)));
}

fn page_opening(ui: &mut egui::Ui, palette: &Palette, c: &mut Config) {
    row(ui, palette, "Extensions", |ui| text_field(ui, &mut c.exts, true));
    note(ui, palette, "pictures of the folder that go into the list; separated by spaces");
    row(ui, palette, "RAW files", |ui| {
        if let Some(i) = choice(ui, &["Camera JPEG", "Windows codec"], c.raw.index()) {
            c.raw = RawSource::nth(i);
        }
    });
    note(ui, palette, "the camera JPEG is fast and always there; the codec decodes the raw image when Windows has one");
    row(ui, palette, "Order", |ui| {
        if let Some(i) = choice(ui, &["Name", "Date", "Size"], c.sort.index()) {
            c.sort = SortBy::nth(i);
        }
    });
    switch(ui, palette, &mut c.single_instance, "Single window", "opening another picture uses the running window (fastest)");
    switch(ui, palette, &mut c.open_fullscreen, "Open in full screen", "one Esc closes the window");
}

fn page_viewing(ui: &mut egui::Ui, palette: &Palette, c: &mut Config) {
    row(ui, palette, "When opened", |ui| {
        if let Some(i) = choice(ui, &["Fit", "1:1", "Like last time"], c.open_view.index()) {
            c.open_view = OpenView::nth(i);
        }
    });
    row(ui, palette, "Wheel", |ui| {
        if let Some(i) = choice(ui, &["Pictures", "Zoom"], c.wheel.index()) {
            c.wheel = WheelMode::nth(i);
        }
    });
    note(ui, palette, "Ctrl+wheel does the other");
    row(ui, palette, "Step + −", |ui| {
        if let Some(i) = choice(ui, &["Steps", "10 %", "25 %"], c.zoom_step.index()) {
            c.zoom_step = ZoomStep::nth(i);
        }
    });
    note(ui, palette, "steps: 5 … 50, 67, 100, 150, 200 … 1200 %");
    row(ui, palette, "Backdrop", |ui| {
        if let Some(i) = choice(ui, &["Dark", "Black", "Gray"], c.background.index()) {
            c.background = Background::nth(i);
        }
    });
    row(ui, palette, "Transparent area", |ui| {
        if let Some(i) = choice(ui, &["Backdrop", "Checkerboard"], c.transparent.index()) {
            c.transparent = Transparent::nth(i);
        }
    });
    row(ui, palette, "Font size", |ui| {
        ui.add(egui::Slider::new(&mut c.font_size, 10.0..=24.0).step_by(1.0).suffix(" px"));
    });
    switch(ui, palette, &mut c.full_path_title, "Full path in the title", "");
    switch(ui, palette, &mut c.enlarge_small, "Enlarge small pictures when fitting", "");
    switch(ui, palette, &mut c.exif_rotate, "Rotate by EXIF", "off: as the camera saved it");
    switch(ui, palette, &mut c.hints, "Key row under the picture", "");
    switch(ui, palette, &mut c.meta_bar, "Bar above the picture (name, dimensions, size)", "");
}

fn page_resizing(ui: &mut egui::Ui, palette: &Palette, c: &mut Config) {
    row(ui, palette, "Quick choices", |ui| text_field(ui, &mut c.quick_sides, false));
    note(ui, palette, "long side in pixels, separated by spaces");
    row(ui, palette, "Enlarging", |ui| {
        if let Some(i) = choice(ui, &["Not allowed", "With a warning", "Freely"], c.enlarge.index()) {
            c.enlarge = Enlarge::nth(i);
        }
    });
    note(ui, palette, "never beyond twice the original or 16 000 px");
}

fn page_saving(ui: &mut egui::Ui, palette: &Palette, c: &mut Config) {
    row(ui, palette, "Name", |ui| text_field(ui, &mut c.save_template, true));
    note(ui, palette, "{name} {long_side} {width} {height} {taken:%Y-%m-%d} · always in the picture's own folder");
    switch(ui, palette, &mut c.quality_from_original, "Quality from the original (JPEG)", "");
    row(ui, palette, "Otherwise", |ui| {
        let mut q = u32::from(c.quality);
        if ui.add(egui::Slider::new(&mut q, 1..=100)).changed() {
            c.quality = q as u8;
        }
    });
    note(ui, palette, "PNG, TIFF, HEIC, RAW, or the switch off");
    switch(ui, palette, &mut c.keep_exif, "Keep EXIF", "the orientation is reset");
    switch(ui, palette, &mut c.goto_saved, "Go to the saved file after saving", "");
}

fn page_files(ui: &mut egui::Ui, palette: &Palette, c: &mut Config) {
    switch(ui, palette, &mut c.ask_delete, "Del asks before the Recycle Bin", "");
    row(ui, palette, "Sidecar files", |ui| text_field(ui, &mut c.sidecars, true));
    note(ui, palette, "files with the same name that go along with F2 and Del");
    row(ui, palette, "Editor (F4)", |ui| text_field(ui, &mut c.editor, true));
    note(ui, palette, "picture editor .exe, e.g. Photoshop; empty = the Windows “Edit” action");
}

fn page_moving(ui: &mut egui::Ui, palette: &Palette, c: &mut Config) {
    row(ui, palette, "Unsaved change", |ui| {
        if let Some(i) = choice(ui, &["Drop it", "Ask"], c.unsaved.index()) {
            c.unsaved = Unsaved::nth(i);
        }
    });
    note(ui, palette, "when moving to another picture or closing");
    row(ui, palette, "At the end of the folder", |ui| {
        if let Some(i) = choice(ui, &["Stop", "Wrap around", "Next folder"], c.at_end.index()) {
            c.at_end = AtEnd::nth(i);
        }
    });
}

fn page_fullscreen(ui: &mut egui::Ui, palette: &Palette, c: &mut Config, vars_open: &mut bool) {
    row(ui, palette, "Text", |ui| {
        if let Some(i) = choice(ui, &["Hidden", "Shown", "3 s on change"], c.text.index()) {
            c.text = FullText::nth(i);
        }
    });
    row(ui, palette, "Line 1", |ui| text_field(ui, &mut c.text_line1, true));
    row(ui, palette, "Line 2", |ui| text_field(ui, &mut c.text_line2, true));
    note(ui, palette, "an empty line is left out; a variable without a value disappears with its separator");
    row(ui, palette, "Corner", |ui| {
        if let Some(i) = choice(ui, &["Top left", "Top right", "Bottom left", "Bottom right"], c.text_corner.index()) {
            c.text_corner = Corner::nth(i);
        }
    });
    row(ui, palette, "Font", |ui| {
        if let Some(i) = choice(ui, &["Small", "Medium", "Large"], c.text_size.index()) {
            c.text_size = TextSize::nth(i);
        }
    });
    ui.horizontal(|ui| {
        ui.add_space(156.0);
        if ui.button(if *vars_open { "Hide variables" } else { "Variables…" }).clicked() {
            *vars_open = !*vars_open;
        }
    });
    if *vars_open {
        egui::Grid::new("text-vars").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            for (var, what) in TEXT_VARS {
                ui.label(RichText::new(var).monospace().color(palette.text));
                ui.label(RichText::new(what).color(palette.text_dim).small());
                ui.end_row();
            }
        });
    }
}
