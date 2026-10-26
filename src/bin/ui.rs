use eframe::egui::{self, Color32, RichText};
use fiio_monitor::recorder::{Recorder, RecordingSummary};
use fiio_monitor::{devices, Devices, Monitor, Readings};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ACCENT: Color32 = Color32::from_rgb(112, 225, 183);
const MUTED: Color32 = Color32::from_rgb(151, 164, 180);
const WARNING: Color32 = Color32::from_rgb(247, 186, 106);
const RECORDING: Color32 = Color32::from_rgb(246, 124, 129);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Choose,
    Listen,
    Record,
}

struct MonitorApp {
    mode: Mode,
    devices: Devices,
    input: String,
    output: String,
    volume_percent: f32,
    latency_ms: usize,
    monitor: Option<Monitor>,
    recorder: Option<Recorder>,
    record_path: Option<PathBuf>,
    last_recording: Option<RecordingSummary>,
    readings: Readings,
    peak_db: f32,
    error: Option<String>,
    last_meter: Instant,
    last_refresh: Instant,
    started: Option<Instant>,
}

impl MonitorApp {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(18, 23, 31);
        visuals.extreme_bg_color = Color32::from_rgb(12, 16, 22);
        visuals.selection.bg_fill = Color32::from_rgb(36, 86, 72);
        visuals.widgets.active.bg_fill = Color32::from_rgb(43, 103, 85);
        context.egui_ctx.set_visuals(visuals);
        context.egui_ctx.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(12.0, 12.0);
            style.spacing.button_padding = egui::vec2(16.0, 10.0);
        });
        let mut app = Self {
            mode: Mode::Choose,
            devices: Devices {
                inputs: vec![],
                outputs: vec![],
                default_output: None,
            },
            input: "KT-USB-Audio".to_owned(),
            output: String::new(),
            volume_percent: 50.0,
            latency_ms: 150,
            monitor: None,
            recorder: None,
            record_path: None,
            last_recording: None,
            readings: Readings::default(),
            peak_db: -60.0,
            error: None,
            last_meter: Instant::now(),
            last_refresh: Instant::now(),
            started: None,
        };
        app.refresh();
        app
    }

    fn active(&self) -> bool {
        self.monitor.is_some() || self.recorder.is_some()
    }

    fn refresh(&mut self) {
        self.last_refresh = Instant::now();
        match devices() {
            Ok(devices) => {
                if self.output.is_empty() {
                    self.output = devices
                        .outputs
                        .iter()
                        .find(|name| name.as_str() == "External Headphones")
                        .cloned()
                        .or(devices.default_output.clone())
                        .unwrap_or_default();
                }
                let input_missing = !devices.inputs.contains(&self.input);
                let output_missing =
                    self.monitor.is_some() && !devices.outputs.contains(&self.output);
                if self.active() && (input_missing || output_missing) {
                    self.stop();
                    if self.error.is_none() {
                        self.error = Some(
                            "Audio device disconnected. Reconnect it and press Start to retry."
                                .to_owned(),
                        );
                    }
                }
                self.devices = devices;
            }
            Err(error) => {
                self.stop();
                if self.error.is_none() {
                    self.error = Some(format!("Cannot list devices: {error}"));
                }
            }
        }
    }

    fn stop(&mut self) {
        self.monitor = None;
        if let Some(recorder) = self.recorder.take() {
            match recorder.stop() {
                Ok(summary) => {
                    if let Some(warning) = &summary.warning {
                        self.error = Some(warning.clone());
                    }
                    self.last_recording = Some(summary);
                }
                Err(error) => self.error = Some(error.to_string()),
            }
            self.record_path = None;
        }
        self.peak_db = -60.0;
        self.started = None;
    }

    fn back(&mut self) {
        self.stop();
        self.mode = Mode::Choose;
    }

    fn reset_meter(&mut self) {
        self.readings = Readings::default();
        self.peak_db = -60.0;
        self.last_meter = Instant::now();
        self.error = None;
        self.started = Some(Instant::now());
    }

    fn start_listening(&mut self) {
        match Monitor::start_with_volume(
            &self.input,
            &self.output,
            self.volume_percent,
            self.latency_ms,
        ) {
            Ok(monitor) => {
                self.reset_meter();
                self.monitor = Some(monitor);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn choose_file(&mut self) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save FiiO recording")
            .add_filter("WAV audio", &["wav"])
            .set_file_name(format!("fiio-recording-{timestamp}.wav"));
        if let Some(parent) = self.record_path.as_ref().and_then(|path| path.parent()) {
            dialog = dialog.set_directory(parent);
        }
        if let Some(path) = dialog.save_file() {
            self.record_path = Some(path.with_extension("wav"));
            self.error = None;
        }
    }

    fn start_recording(&mut self) {
        let Some(path) = &self.record_path else {
            return;
        };
        if path.exists() {
            self.error = Some("That file already exists. Choose a new filename.".to_owned());
            return;
        }
        match Recorder::start(&self.input, path) {
            Ok(recorder) => {
                self.reset_meter();
                self.last_recording = None;
                self.recorder = Some(recorder);
            }
            Err(error) => self.error = Some(format!("Could not start recording: {error}")),
        }
    }

    fn update_meter(&mut self) {
        if self.last_meter.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.last_meter = Instant::now();
        let readings = if let Some(monitor) = &self.monitor {
            Some(monitor.readings())
        } else {
            self.recorder.as_ref().map(|recorder| recorder.readings())
        };
        if let Some(readings) = readings {
            self.readings = readings;
            self.peak_db = (20.0 * readings.peak.max(0.000001).log10()).clamp(-60.0, 0.0);
            if readings.errors > 0 {
                self.stop();
                if self.error.is_none() {
                    self.error = Some(
                        "Audio stream failed. Check the connection and press Start to retry."
                            .to_owned(),
                    );
                }
            }
        }
    }

    fn route(ui: &mut egui::Ui, label: &str, selected: &mut String, choices: &[String]) {
        ui.label(RichText::new(label).small().color(MUTED));
        let caption = if selected.is_empty() {
            "Select a device".to_owned()
        } else if choices.contains(selected) {
            selected.clone()
        } else {
            format!("{selected} (disconnected)")
        };
        egui::ComboBox::from_id_salt(label)
            .width(ui.available_width() - 8.0)
            .selected_text(caption)
            .show_ui(ui, |ui| {
                for name in choices {
                    ui.selectable_value(selected, name.clone(), name);
                }
            });
    }

    fn choose_mode(&mut self, ui: &mut egui::Ui) {
        ui.add_space(30.0);
        ui.label(
            RichText::new("FiiO / AUDIO STUDIO")
                .size(12.0)
                .color(ACCENT)
                .strong(),
        );
        ui.add_space(12.0);
        ui.label(
            RichText::new("What would you like to do?")
                .size(29.0)
                .strong(),
        );
        ui.label(RichText::new("Choose a mode to get started.").color(MUTED));
        ui.add_space(24.0);
        let width = ui.available_width();
        if ui
            .add_sized(
                [width, 90.0],
                egui::Button::new(RichText::new("Listen to music").size(23.0))
                    .fill(Color32::from_rgb(30, 58, 52)),
            )
            .clicked()
        {
            self.mode = Mode::Listen;
            self.readings = Readings::default();
            self.peak_db = -60.0;
        }
        ui.label(
            RichText::new(
                "Send FiiO audio to your headphones or Mac speakers.
Adjust the listening level with the volume slider.",
            )
            .color(MUTED),
        );
        ui.add_space(18.0);
        if ui
            .add_sized(
                [width, 90.0],
                egui::Button::new(RichText::new("Record from FiiO").size(23.0))
                    .fill(Color32::from_rgb(62, 37, 45)),
            )
            .clicked()
        {
            self.mode = Mode::Record;
            self.readings = Readings::default();
            self.peak_db = -60.0;
        }
        ui.label(
            RichText::new(
                "Capture USB audio to a WAV file on your Mac.
Stereo · 48 kHz / 16-bit · original input level.",
            )
            .color(MUTED),
        );
        ui.add_space(24.0);
        ui.separator();
        ui.label(
            RichText::new(
                "Connect USB REC and select USB OUT on the DM13.
Nothing starts until you press Start in the selected mode.",
            )
            .small()
            .color(MUTED),
        );
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        if ui
            .button("← Back")
            .on_hover_text("Stop audio, finish any recording, and choose a mode")
            .clicked()
        {
            self.back();
            return;
        }
        let recording = self.mode == Mode::Record;
        let color = if recording { RECORDING } else { ACCENT };
        ui.label(
            RichText::new("FiiO / AUDIO STUDIO")
                .size(12.0)
                .color(color)
                .strong(),
        );
        ui.label(
            RichText::new(if recording {
                "Record from FiiO"
            } else {
                "Listen to music"
            })
            .size(29.0)
            .strong(),
        );
        ui.label(
            RichText::new(if recording {
                "WAV · stereo / 48 kHz / 16-bit · original input level"
            } else {
                "Live USB audio · stereo / 48 kHz · no recording"
            })
            .color(MUTED),
        );
        ui.separator();
    }

    fn source_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(!self.active(), |ui| {
            Self::route(ui, "SOURCE", &mut self.input, &self.devices.inputs);
            if self.mode == Mode::Listen {
                Self::route(ui, "LISTEN ON", &mut self.output, &self.devices.outputs);
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Refresh devices").clicked() {
                self.refresh();
            }
            ui.label(
                RichText::new("Updates every 2 seconds")
                    .small()
                    .color(MUTED),
            );
        });
        ui.separator();
    }

    fn listening_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("FiiO VOLUME").small().color(MUTED));
        let previous_volume = self.volume_percent;
        ui.spacing_mut().slider_width = (ui.available_width() - 105.0).max(120.0);
        let response = ui.add(
            egui::Slider::new(&mut self.volume_percent, 0.0..=100.0)
                .step_by(1.0)
                .fixed_decimals(0)
                .suffix(" %"),
        );
        if response.hovered() {
            let (scroll, fine) = ui.input_mut(|input| {
                let scroll = input.smooth_scroll_delta.y;
                input.smooth_scroll_delta = egui::Vec2::ZERO;
                (scroll, input.modifiers.shift)
            });
            self.volume_percent =
                (self.volume_percent + scroll * if fine { 0.01 } else { 0.1 }).clamp(0.0, 100.0);
        }
        if response.double_clicked() {
            self.volume_percent = 50.0;
        }
        response.on_hover_text(
            "Scroll to adjust; hold Shift for finer control. Double-click resets to 50%.",
        );
        if self.volume_percent != previous_volume {
            if let Some(monitor) = &self.monitor {
                monitor.set_volume_percent(self.volume_percent);
            }
        }
        ui.add_enabled_ui(!self.active(), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("BUFFER").small().color(MUTED));
                ui.spacing_mut().slider_width = (ui.available_width() - 110.0).max(100.0);
                ui.add(
                    egui::Slider::new(&mut self.latency_ms, 50..=1000)
                        .suffix(" ms")
                        .step_by(25.0),
                );
            });
        });
    }

    fn recording_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("SAVE RECORDING TO").small().color(MUTED));
        if let Some(path) = &self.record_path {
            ui.label(path.display().to_string());
        } else {
            ui.label(RichText::new("Choose a WAV file before recording").color(MUTED));
        }
        if ui
            .add_enabled(!self.active(), egui::Button::new("Choose WAV file…"))
            .clicked()
        {
            self.choose_file();
        }
        ui.label(
            RichText::new(
                "Recording uses the original FiiO input level.
The listening volume slider does not affect this file.",
            )
            .small()
            .color(MUTED),
        );
        ui.label(
            RichText::new("Audio is saved without playback to speakers or headphones.")
                .small()
                .color(MUTED),
        );
        let duration = self
            .recorder
            .as_ref()
            .map(|recorder| recorder.duration())
            .or_else(|| {
                self.last_recording
                    .as_ref()
                    .map(|summary| summary.duration())
            })
            .unwrap_or_default();
        ui.label(
            RichText::new(format!(
                "{:02}:{:02}.{:01}",
                duration.as_secs() / 60,
                duration.as_secs() % 60,
                duration.subsec_millis() / 100
            ))
            .size(36.0)
            .monospace()
            .color(if self.recorder.is_some() {
                RECORDING
            } else {
                MUTED
            }),
        );
    }

    fn meter(&self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("INPUT SIGNAL").small().color(MUTED));
            let text = if self.peak_db <= -60.0 {
                "< -60 dBFS".to_owned()
            } else {
                format!("{:.1} dBFS", self.peak_db)
            };
            ui.label(RichText::new(text).monospace());
        });
        ui.add(
            egui::ProgressBar::new((self.peak_db + 60.0) / 60.0)
                .fill(if self.peak_db > -1.0 { WARNING } else { ACCENT })
                .desired_height(14.0),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Near full scale: {}", self.readings.clipped))
                .on_hover_text("Frames close to 0 dBFS; this alone does not prove clipping.");
            ui.label(format!("Dropped: {}", self.readings.overflow));
            if self.mode == Mode::Listen {
                ui.label(format!("Gaps: {}", self.readings.underrun));
            }
            ui.label(format!("Errors: {}", self.readings.errors));
        });
        if self.readings.clipped > 0 {
            ui.colored_label(WARNING, "Input came close to full scale. Source distortion cannot be repaired by lowering the listening volume.");
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        let recording = self.mode == Mode::Record;
        let active = self.active();
        ui.horizontal(|ui| {
            if active {
                if ui
                    .add_sized(
                        [180.0, 46.0],
                        egui::Button::new(if recording {
                            "Stop and save"
                        } else {
                            "Stop listening"
                        }),
                    )
                    .clicked()
                {
                    self.stop();
                }
            } else {
                let ready = self.devices.inputs.contains(&self.input)
                    && if recording {
                        self.record_path.is_some()
                    } else {
                        self.devices.outputs.contains(&self.output)
                    };
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(
                            RichText::new(if recording {
                                "Start recording"
                            } else {
                                "Start listening"
                            })
                            .color(Color32::from_rgb(20, 28, 28)),
                        )
                        .fill(if recording { RECORDING } else { ACCENT })
                        .min_size(egui::vec2(180.0, 46.0)),
                    )
                    .clicked()
                {
                    if recording {
                        self.start_recording();
                    } else {
                        self.start_listening();
                    }
                }
            }
            let status = if self.recorder.is_some() {
                "RECORDING"
            } else if self.monitor.is_some() {
                "LIVE"
            } else {
                "STOPPED"
            };
            ui.label(
                RichText::new(status)
                    .monospace()
                    .color(if self.recorder.is_some() {
                        RECORDING
                    } else if self.monitor.is_some() {
                        ACCENT
                    } else {
                        MUTED
                    }),
            );
        });
        ui.add_space(8.0);
        ui.label(RichText::new(if recording {
            "Start playback on the DM13. Stop and save, Back, or closing this window finishes the WAV file."
        } else { "Start playback on the DM13. Stop before changing the route." }).small().color(MUTED));
    }

    fn messages(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            ui.colored_label(WARNING, error);
        }
        if self.mode != Mode::Listen {
            if let Some(summary) = &self.last_recording {
                ui.add_space(8.0);
                ui.label(RichText::new("SAVED RECORDING").small().color(ACCENT));
                ui.label(summary.path.display().to_string());
                ui.label(format!(
                    "{:.1} seconds · WAV / 48 kHz / 16-bit stereo",
                    summary.duration().as_secs_f64()
                ));
            }
        }
    }
}

impl eframe::App for MonitorApp {
    fn update(&mut self, context: &egui::Context, _: &mut eframe::Frame) {
        if self.last_refresh.elapsed() >= Duration::from_secs(2) {
            self.refresh();
        }
        self.update_meter();
        context.request_repaint_after(Duration::from_millis(100));
        egui::CentralPanel::default().show(context, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(14.0);
                if self.mode == Mode::Choose {
                    self.choose_mode(ui);
                } else {
                    self.header(ui);
                    if self.mode != Mode::Choose {
                        self.source_controls(ui);
                        if self.mode == Mode::Listen {
                            self.listening_controls(ui);
                        } else {
                            self.recording_controls(ui);
                        }
                        self.meter(ui);
                        self.transport(ui);
                    }
                }
                self.messages(ui);
                ui.add_space(12.0);
            });
        });
    }
}

impl Drop for MonitorApp {
    fn drop(&mut self) {
        self.stop();
        if let Some(error) = &self.error {
            eprintln!("{error}");
        }
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 800.0])
            .with_min_inner_size([460.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "FiiO Audio Studio",
        options,
        Box::new(|context| Ok(Box::new(MonitorApp::new(context)))),
    )
}
