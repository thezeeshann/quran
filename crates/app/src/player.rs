//! Recitation: plays one ayah, fetched from EveryAyah when asked (nothing is cached, so it needs
//! an internet connection), with a small player floating at the bottom of the reader. The player
//! goes away by itself when the ayah ends.

use std::io::Cursor;
use std::sync::mpsc::Sender;
use std::time::Duration;

use eframe::egui::{self, Align2, Color32, CursorIcon, Id, Rect, Sense, pos2, vec2};
use quran_core::AyahRef;
use quran_core::recitation::{self, FetchError, Reciter};

use crate::actions::Action;
use crate::app::TaskResult;
use crate::icons;
use crate::theme::{self, Palette};

/// The player's size and the gap under it.
const WIDTH: f32 = 340.0;
const HEIGHT: f32 = 56.0;
const BOTTOM: f32 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Loading,
    Playing,
    Paused,
}

struct Track {
    at: AyahRef,
    reciter: Reciter,
    state: State,
    player: Option<rodio::Player>,
    duration: Option<Duration>,
}

#[derive(Default)]
pub struct Player {
    /// The audio device, opened on first use and kept (opening it takes a moment).
    device: Option<rodio::MixerDeviceSink>,
    track: Option<Track>,
    /// Counts requests, so a fetch that was overtaken by another ayah is dropped.
    generation: u64,
    /// What the player showed last, kept while it fades out.
    shown: Option<(AyahRef, Reciter, State, f32)>,
}

impl Player {
    /// The ayah being recited (or loading).
    pub fn current(&self) -> Option<AyahRef> {
        self.track.as_ref().map(|t| t.at)
    }

    pub fn state(&self) -> Option<State> {
        self.track.as_ref().map(|t| t.state)
    }

    pub fn active(&self) -> bool {
        self.track.is_some()
    }

    /// Fetches `at` in `reciter`'s voice and plays it when it arrives, replacing what is playing.
    pub fn start(&mut self, ctx: &egui::Context, at: AyahRef, reciter: Reciter, tx: &Sender<TaskResult>) {
        self.stop();
        self.generation += 1;
        self.track = Some(Track { at, reciter, state: State::Loading, player: None, duration: None });
        let (generation, tx, ctx) = (self.generation, tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = recitation::fetch(reciter, at);
            let _ = tx.send(TaskResult::Recitation(generation, r));
            ctx.request_repaint();
        });
    }

    /// Plays the ayah fetched by request `generation`; returns a message for the user if it
    /// can't be played.
    pub fn on_audio(&mut self, generation: u64, r: Result<Vec<u8>, FetchError>) -> Option<String> {
        if generation != self.generation || self.track.as_ref().is_none_or(|t| t.state != State::Loading) {
            return None;
        }
        let bytes = match r {
            Ok(b) => b,
            Err(e) => {
                self.track = None;
                return Some(e.to_string());
            }
        };
        if self.device.is_none() {
            // The default device with any configuration it supports (Bluetooth headphones
            // often refuse rodio's default one), then any device.
            let opened = rodio::DeviceSinkBuilder::from_default_device()
                .and_then(|b| b.open_sink_or_fallback())
                .or_else(|_| rodio::DeviceSinkBuilder::open_default_sink());
            match opened {
                Ok(mut d) => {
                    d.log_on_drop(false);
                    self.device = Some(d);
                }
                Err(e) => {
                    self.track = None;
                    return Some(format!("Couldn't open the sound output ({e})"));
                }
            }
        }
        let len = bytes.len();
        let source = match rodio::Decoder::new(Cursor::new(bytes)) {
            Ok(s) => s,
            Err(e) => {
                self.track = None;
                return Some(format!("Couldn't play the recitation ({e})"));
            }
        };
        use rodio::Source as _;
        // The files are 128 kbit/s MP3s: their size gives the length when the decoder can't.
        let duration =
            source.total_duration().or(Some(Duration::from_secs_f64(len as f64 * 8.0 / 128_000.0)));
        let device = self.device.as_ref()?;
        let player = rodio::Player::connect_new(device.mixer());
        player.append(source);
        if let Some(t) = &mut self.track {
            t.player = Some(player);
            t.duration = duration;
            t.state = State::Playing;
        }
        None
    }

    pub fn toggle_pause(&mut self) {
        let Some(t) = &mut self.track else { return };
        let Some(p) = &t.player else { return };
        if t.state == State::Paused {
            p.play();
            t.state = State::Playing;
        } else {
            p.pause();
            t.state = State::Paused;
        }
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.track.take().and_then(|t| t.player) {
            p.stop();
        }
        // A fetch still under way is dropped when it arrives.
        self.generation += 1;
    }

    /// How far through the ayah (0–1).
    fn progress(&self) -> f32 {
        let Some(t) = &self.track else { return 0.0 };
        match (&t.player, t.duration) {
            (Some(p), Some(d)) if !d.is_zero() => {
                (p.get_pos().as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    /// Ends the track when the ayah is over (which removes the player) and keeps the progress
    /// moving while it plays.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let done = self
            .track
            .as_ref()
            .is_some_and(|t| t.state == State::Playing && t.player.as_ref().is_some_and(|p| p.empty()));
        if done {
            self.track = None;
        }
        match self.state() {
            Some(State::Playing) => ctx.request_repaint_after(Duration::from_millis(33)),
            Some(State::Loading) => ctx.request_repaint_after(Duration::from_millis(50)),
            _ => {}
        }
    }

    /// Paints the player at the bottom centre of `area`; returns the height it takes there (for
    /// the toasts to sit above it).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        dark: bool,
        area: Rect,
        reduce_motion: bool,
        actions: &mut Vec<Action>,
    ) -> f32 {
        if let Some(t) = &self.track {
            self.shown = Some((t.at, t.reciter, t.state, self.progress()));
        }
        let secs = if reduce_motion { 0.0 } else { theme::motion::FAST_MS / 1000.0 };
        let shown = ctx.animate_bool_with_time(Id::new("recitation-player"), self.track.is_some(), secs);
        if shown <= 0.0 {
            self.shown = None;
            return 0.0;
        }
        let Some((at, reciter, state, progress)) = self.shown else { return 0.0 };
        let interactive = self.track.is_some();

        // The toasts' material: ink on paper turned around.
        let [r, g, b, _] = if dark { [0xF2, 0xF0, 0xEB, 0xFF] } else { p.ink.to_array() };
        let fill = Color32::from_rgba_unmultiplied(r, g, b, 0xF5).gamma_multiply(shown);
        let ink = p.paper.gamma_multiply(shown);
        let ink_2 = p.paper.gamma_multiply(0.62 * shown);
        let rise = (1.0 - shown) * 12.0;
        let y = area.bottom() - BOTTOM + rise;
        egui::Area::new(Id::new("recitation-player-area"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(area.center().x, y))
            .pivot(Align2::CENTER_BOTTOM)
            .interactable(interactive)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, HEIGHT), Sense::hover());
                let painter = ui.painter().clone();
                painter.add(
                    egui::epaint::RectShape::filled(
                        rect.translate(vec2(0.0, 8.0)),
                        HEIGHT / 2.0,
                        Color32::from_black_alpha((0x2E as f32 * shown) as u8),
                    )
                    .with_blur_width(48.0),
                );
                painter.rect_filled(rect, HEIGHT / 2.0, fill);

                // Play / pause.
                let button = Rect::from_center_size(
                    pos2(rect.left() + 10.0 + 18.0, rect.center().y),
                    vec2(36.0, 36.0),
                );
                let resp = ui.interact(button, Id::new("recitation-toggle"), Sense::click());
                let hovered = interactive && resp.hovered();
                painter.circle_filled(
                    button.center(),
                    18.0,
                    p.accent.gamma_multiply(if hovered { 1.0 } else { 0.92 } * shown),
                );
                let glyph = Rect::from_center_size(button.center(), vec2(16.0, 16.0));
                let on_accent = p.on_accent.gamma_multiply(shown);
                match state {
                    State::Loading => {
                        // Three dots breathing in turn.
                        let t = ctx.input(|i| i.time) as f32;
                        for k in 0..3 {
                            let a = 0.35 + 0.65 * ((t * 5.0 - k as f32 * 0.9).sin() * 0.5 + 0.5);
                            icons::dot(
                                &painter,
                                button.center() + vec2((k as f32 - 1.0) * 6.0, 0.0),
                                2.0,
                                on_accent.gamma_multiply(a),
                            );
                        }
                    }
                    State::Playing => icons::pause(&painter, glyph.translate(vec2(0.0, 0.0)), on_accent),
                    State::Paused => icons::play(&painter, glyph.translate(vec2(1.0, 0.0)), on_accent),
                }
                if interactive && state != State::Loading {
                    if resp.clicked() {
                        actions.push(Action::TogglePlayback);
                    }
                    if hovered {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                    }
                }

                // Close.
                let close = Rect::from_center_size(
                    pos2(rect.right() - 14.0 - 12.0, rect.center().y),
                    vec2(24.0, 24.0),
                );
                let cresp = ui.interact(close, Id::new("recitation-close"), Sense::click());
                if interactive && cresp.hovered() {
                    painter.circle_filled(close.center(), 12.0, p.paper.gamma_multiply(0.14 * shown));
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }
                icons::close(&painter, Rect::from_center_size(close.center(), vec2(14.0, 14.0)), ink_2);
                if interactive && cresp.clicked() {
                    actions.push(Action::StopPlayback);
                }

                // Title, reciter and progress.
                let x0 = button.right() + 12.0;
                let x1 = close.left() - 10.0;
                let title = format!("{} · {}:{}", at.surah_id().name(), at.surah, at.ayah);
                let title = crate::pages::elide(&painter, &title, &theme::ui_medium(13.0), x1 - x0);
                let title = painter.layout_no_wrap(title, theme::ui_medium(13.0), ink);
                let sub = if state == State::Loading {
                    "Loading…".to_string()
                } else {
                    reciter.name().to_string()
                };
                let sub = painter.layout_no_wrap(sub, theme::ui_font(11.5), ink_2);
                let top = rect.center().y - 15.0;
                painter.galley(pos2(x0, top), title, ink);
                painter.galley(pos2(x0, top + 17.0), sub, ink_2);
                let track = Rect::from_min_max(pos2(x0, top + 34.0), pos2(x1, top + 36.0));
                painter.rect_filled(track, 1.0, p.paper.gamma_multiply(0.2 * shown));
                if progress > 0.0 {
                    let done = Rect::from_min_max(
                        track.min,
                        pos2(track.left() + track.width() * progress, track.bottom()),
                    );
                    painter.rect_filled(done, 1.0, ink);
                }
            });
        (HEIGHT + 8.0) * shown
    }
}
