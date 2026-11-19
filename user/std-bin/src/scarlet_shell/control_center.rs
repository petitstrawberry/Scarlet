//! ScarletUI Control Center scene content.
//!
//! The shell owns provider sampling. This module only maps immutable provider
//! snapshots to ordinary ScarletUI views and typed actions. Rendering, layout,
//! hit testing, hover, press, and slider capture remain owned by ScarletUI.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use scarlet_ui::color::ColorPalette;
use scarlet_ui::element::{ComponentElement, Element};
use scarlet_ui::geometry::{Alignment, Point, Rect, Size};
use scarlet_ui::state::State;
use scarlet_ui::view::{View, ViewExt};
use scarlet_ui::views::containers::ViewTuple;
use scarlet_ui::views::{
    Button, ElevationRole, GlassMaterial, HStack, Slider, Spacer, Surface, SurfaceRole,
    SurfaceStyle, Text, TextStyle, VStack,
};
use scarlet_ui::{Icon, IconSize};

/// The shell presentation used for the Control Center window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlCenterPresentation {
    /// A compact popover anchored below the laptop StatusBar status item.
    LaptopPopover,
    /// A wider touch-first sheet.
    TabletSheet,
}

/// One selectable audio output supplied by the audio provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioOutputSnapshot {
    /// Stable provider-defined output identifier.
    pub id: String,
    /// User-facing output name.
    pub name: String,
    /// Whether the output can currently be selected.
    pub available: bool,
}

/// Read-only audio state plus selectable outputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSnapshot {
    /// Master volume percentage, or `None` when unavailable.
    pub volume_percent: Option<u8>,
    /// Master mute state, or `None` when unavailable.
    pub muted: Option<bool>,
    /// Outputs reported by the audio service.
    pub outputs: Vec<AudioOutputSnapshot>,
    /// Stable identifier of the selected output, when known.
    pub current_output_id: Option<String>,
}

impl AudioSnapshot {
    /// Build audio state from the same sample used by StatusBar status.
    ///
    /// # Arguments
    ///
    /// * `volume_percent` - Sampled master volume percentage.
    /// * `muted` - Sampled master mute state.
    ///
    /// # Returns
    ///
    /// An audio snapshot with no output enumeration yet.
    pub fn from_status(volume_percent: u8, muted: bool) -> Self {
        Self {
            volume_percent: Some(volume_percent.min(100)),
            muted: Some(muted),
            outputs: Vec::new(),
            current_output_id: None,
        }
    }

    /// Build an explicitly unavailable audio snapshot.
    ///
    /// # Returns
    ///
    /// A snapshot which renders unavailable state instead of invented values.
    pub fn unavailable() -> Self {
        Self {
            volume_percent: None,
            muted: None,
            outputs: Vec::new(),
            current_output_id: None,
        }
    }
}

/// Connection state for one read-only network interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkInterfaceState {
    /// The interface has an active connection.
    Connected,
    /// Carrier/authentication is ready; IP configuration is still pending.
    Configuring,
    /// The interface exists but has no active connection.
    Disconnected,
    /// The interface state could not be determined.
    Unknown,
}

/// Read-only network interface status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkInterfaceSnapshot {
    /// Stable provider-defined interface name.
    pub name: String,
    /// Current connection state.
    pub state: NetworkInterfaceState,
    /// Optional address or concise provider-supplied detail.
    pub detail: Option<String>,
}

/// Read-only network provider snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkSnapshot {
    /// Whether the network provider answered this sample.
    pub available: bool,
    /// Interfaces reported by the provider.
    pub interfaces: Vec<NetworkInterfaceSnapshot>,
}

/// Read-only CPU and task summary sampled by the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemSnapshot {
    /// CPU utilization percentage, or `None` when unavailable.
    pub cpu_percent: Option<u8>,
    /// Current task count, or `None` when unavailable.
    pub task_count: Option<u32>,
}

/// SWS input-environment state shown by the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEnvironmentSnapshot {
    /// Whether SWS supplied an input-environment sample.
    pub available: bool,
    /// Whether SWS currently requests tablet presentation.
    pub tablet_mode: Option<bool>,
    /// Whether touch input is present, when known.
    pub touch_present: Option<bool>,
    /// Whether a physical keyboard is present, when known.
    pub keyboard_present: Option<bool>,
    /// Whether a pointing device is present, when known.
    pub pointer_present: Option<bool>,
}

/// Complete provider snapshot consumed by Control Center.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlCenterSnapshot {
    /// Battery and input-power state from the shared background sample.
    pub power: super::power::PowerStatus,
    /// Audio state sourced from the StatusBar's shared sample.
    pub audio: AudioSnapshot,
    /// Display backlight percentage, or `None` when unsupported/unavailable.
    pub brightness_percent: Option<u8>,
    /// Read-only network state.
    pub network: NetworkSnapshot,
    /// CPU and task state sourced from the shared system sample.
    pub system: SystemSnapshot,
    /// SWS input-environment state.
    pub input_environment: InputEnvironmentSnapshot,
}

impl ControlCenterSnapshot {
    /// Initial view before the background providers have answered.
    pub fn unavailable() -> Self {
        Self {
            power: Default::default(),
            audio: AudioSnapshot::unavailable(),
            brightness_percent: None,
            network: NetworkSnapshot {
                available: false,
                interfaces: Vec::new(),
            },
            system: SystemSnapshot {
                cpu_percent: None,
                task_count: None,
            },
            input_environment: InputEnvironmentSnapshot {
                available: false,
                tablet_mode: None,
                touch_present: None,
                keyboard_present: None,
                pointer_present: None,
            },
        }
    }
}

impl Default for ControlCenterSnapshot {
    fn default() -> Self {
        Self::unavailable()
    }
}

/// One requested opening and its completed provider sample. A cancelled
/// opening cannot publish an in-flight sample into a later opening.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlCenterSession {
    pub open: bool,
    pub snapshot: Option<ControlCenterSnapshot>,
    pub page: ControlCenterPage,
    generation: u64,
}

impl ControlCenterSession {
    pub fn request_open(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.snapshot = None;
        self.page = ControlCenterPage::Controls;
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn publish(&mut self, generation: u64, snapshot: ControlCenterSnapshot) {
        if self.open && self.generation == generation {
            self.snapshot = Some(snapshot);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControlCenterPage {
    #[default]
    Controls,
    AudioOutputs(usize),
    System,
}

/// Settings destination requested by Control Center.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlCenterSettingsLink {
    /// Open network settings.
    Network,
    /// Open the settings application root.
    AllSettings,
}

/// Typed command emitted by standard ScarletUI controls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlCenterAction {
    /// Commit a master volume percentage.
    SetVolume(u8),
    /// Commit a display backlight percentage through the display control IF.
    SetBrightness(u8),
    /// Toggle master mute.
    ToggleMute,
    /// Select an available audio output by stable identifier.
    SelectOutput(String),
    /// Open a settings destination.
    OpenSettings(ControlCenterSettingsLink),
    /// Confirm a previously armed power-off request.
    ConfirmPowerOff,
    /// Confirm a previously armed reboot request.
    ConfirmReboot,
}

/// Suppress repeated integer writes only while their acknowledged provider
/// value remains current. Keep this in the provider worker, not the Slider:
/// input remains continuous and a failed request can be retried unchanged.
#[derive(Default)]
pub(super) struct PercentWriteState {
    acknowledged: Option<(u8, u8)>,
}

impl PercentWriteState {
    pub fn needs_write(&self, requested: u8, observed: Option<u8>) -> bool {
        let Some(actual) = observed else {
            return true;
        };
        self.acknowledged != Some((requested, actual))
    }

    pub fn complete(&mut self, requested: u8, applied: Option<u8>) {
        self.acknowledged = applied.map(|actual| (requested, actual));
    }
}

/// Stable dimensions shared by the scene declaration and its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlCenterMetrics {
    /// Visible Control Center surface width, excluding shadow outsets.
    pub width: u32,
    /// Visible Control Center surface height, excluding shadow outsets.
    pub height: u32,
    margin: u32,
    gap: u32,
    target_height: u32,
    page: ControlCenterPage,
    details_height: u32,
    audio_rows: usize,
    audio_pages: usize,
    audio_rows_per_page: usize,
}

impl ControlCenterMetrics {
    /// Corner radius shared by the managed window body and floating surface.
    pub const CORNER_RADIUS: f32 = 16.0;

    /// Fit the current page within the logical space below the status bar.
    pub fn resolve(
        presentation: ControlCenterPresentation,
        available: Size,
        page: ControlCenterPage,
        output_count: usize,
    ) -> Self {
        let (preferred_width, preferred_margin, preferred_target, preferred_details) =
            match presentation {
                ControlCenterPresentation::LaptopPopover => (304, 8, 32, 84),
                ControlCenterPresentation::TabletSheet => (392, 10, 44, 96),
            };
        let width = preferred_width.min((available.width as u32).max(1));
        let height_limit = (available.height as u32).max(1);
        let margin = preferred_margin.min((width.min(height_limit) - 1) / 6);
        let inner_height = height_limit - 2 * margin;
        let gap = 6.min(inner_height / 12);
        let target_height = preferred_target.min(((inner_height - 3 * gap) / 4).max(1));
        let row_extent = target_height + gap;
        let details_height = preferred_details.min(inner_height.saturating_sub(2 * row_extent));
        let audio_rows_per_page = ((inner_height + gap) / row_extent)
            .saturating_sub(2)
            .clamp(1, 5) as usize;
        let audio_rows = output_count.clamp(1, audio_rows_per_page);
        let audio_pages = output_count.div_ceil(audio_rows_per_page).max(1);
        let (page, content_height) = match page {
            ControlCenterPage::Controls => (page, 4 * row_extent - gap),
            ControlCenterPage::AudioOutputs(index) => (
                ControlCenterPage::AudioOutputs(index.min(audio_pages - 1)),
                (2 + audio_rows as u32) * row_extent - gap,
            ),
            ControlCenterPage::System => (page, 2 * row_extent + details_height),
        };
        Self {
            width,
            height: (content_height + 2 * margin).min(height_limit),
            margin,
            gap,
            target_height,
            page,
            details_height,
            audio_rows,
            audio_pages,
            audio_rows_per_page,
        }
    }

    /// Return the managed Control Center body size.
    ///
    /// # Returns
    ///
    /// The visible logical size. ScarletUI's top-level window adds the
    /// semantic shadow surface outside this rectangle.
    pub const fn body_size(&self) -> Size {
        Size::new(self.width as f32, self.height as f32)
    }

    fn content_width(&self) -> f32 {
        (self.width - self.margin * 2) as f32
    }

    fn column_width(&self) -> f32 {
        ((self.content_width() - self.gap as f32) / 2.0).max(1.0)
    }
}

/// Reading material shared by the native backdrop and the in-app glass tint.
/// A menu's denser wash keeps secondary text legible over arbitrary desktops.
fn panel_material() -> GlassMaterial {
    GlassMaterial::menu()
}

/// One desktop backdrop for the managed body, excluding the window shadow.
/// ScarletUI platform coordinates include the floating paint outsets; SWS
/// converts logical dimensions and radius to physical pixels once and caches
/// unchanged region metadata. Foreground labels and controls stay sharp.
pub(super) fn backdrop_region(body: Size) -> scarlet_ui::platform::SurfaceRegion {
    let outsets = ElevationRole::Floating.paint_outsets();
    scarlet_ui::platform::SurfaceRegion {
        rect: Rect::new(Point::new(outsets.left, outsets.top), body),
        corner_radius: ControlCenterMetrics::CORNER_RADIUS,
        blur_radius: if scarlet_ui::glass_quality() == scarlet_ui::GlassQuality::TintOnly {
            0.0
        } else {
            panel_material().blur_radius
        },
        accepts_input: true,
    }
}

/// Build the active Control Center page with a shared Settings button.
pub fn build_control_center_view(
    metrics: ControlCenterMetrics,
    snapshot: ControlCenterSnapshot,
    volume: State<f32>,
    brightness: State<f32>,
    action: State<Option<ControlCenterAction>>,
    session: State<ControlCenterSession>,
) -> impl View + Clone {
    let gap = metrics.gap as f32;
    let mut rows = match metrics.page {
        ControlCenterPage::Controls => {
            controls_page(metrics, &snapshot, volume, brightness, &action, &session)
        }
        ControlCenterPage::AudioOutputs(index) => {
            audio_outputs_page(metrics, index, &snapshot, &action, &session)
        }
        ControlCenterPage::System => system_page(metrics, &snapshot, &action, &session),
    };
    rows.push(settings_button(metrics, action));
    let content = VStack::new(DynamicViews::new(rows))
        .spacing(gap)
        .alignment(Alignment::Leading)
        .padding(metrics.margin as f32);

    Surface::new(content, SurfaceRole::Floating)
        // SWS filters the desktop behind the managed body. Sliders can
        // still sample this surface through their ordinary local materials.
        .surface_style(SurfaceStyle::bubble_glass().material(panel_material().blur_radius(0.0)))
        .corner_radius(ControlCenterMetrics::CORNER_RADIUS)
        .frame(metrics.width as f32, metrics.height as f32)
}

fn page_button(
    label: &str,
    icon: Icon,
    page: ControlCenterPage,
    size: Size,
    session: &State<ControlCenterSession>,
) -> Box<dyn View> {
    let session = session.clone();
    boxed(
        Button::new(label)
            .icon(icon)
            .icon_size(IconSize::Small)
            .header_style()
            .font_size(12.0)
            .padding(0.0)
            .on_click(move || session.update(|session| session.page = page))
            .frame(size.width, size.height)
            .clip(),
    )
}

fn back_button(size: Size, session: &State<ControlCenterSession>) -> Box<dyn View> {
    page_button(
        "Back",
        Icon::ChevronLeft,
        ControlCenterPage::Controls,
        size,
        session,
    )
}

fn controls_page(
    metrics: ControlCenterMetrics,
    snapshot: &ControlCenterSnapshot,
    volume: State<f32>,
    brightness: State<f32>,
    action: &State<Option<ControlCenterAction>>,
    session: &State<ControlCenterSession>,
) -> Vec<Box<dyn View>> {
    let palette = ColorPalette::default();
    let control_row_width = metrics.content_width();
    let detail_size = 12.0;
    let target_height = metrics.target_height as f32;
    // Keep both slider tracks aligned when the brightness percentage grows.
    let control_label_width = 84.0_f32.min(control_row_width / 2.0);
    let control_slider_width =
        (control_row_width - control_label_width - metrics.gap as f32).max(1.0);

    let mute_action = action.clone();
    let mute = Button::new(if snapshot.audio.volume_percent.is_none() {
        "Unavailable"
    } else if snapshot.audio.muted == Some(true) {
        "Muted"
    } else {
        "Volume"
    })
    .header_style()
    .font_size(detail_size)
    .enabled(snapshot.audio.muted.is_some())
    .padding(0.0)
    .on_click(move || mute_action.set(Some(ControlCenterAction::ToggleMute)));

    let slider_action = action.clone();
    let slider = Slider::new(volume)
        .enabled(snapshot.audio.volume_percent.is_some())
        .min(0.0)
        .max(100.0)
        .on_change(move |value| {
            slider_action.set(Some(ControlCenterAction::SetVolume(
                value.clamp(0.0, 100.0).round() as u8,
            )));
        });

    let mut rows: Vec<Box<dyn View>> = vec![boxed(
        HStack::new(DynamicViews::new(vec![
            boxed(
                HStack::new(DynamicViews::new(vec![boxed(mute), boxed(Spacer::new())]))
                    .spacing(0.0)
                    .frame_width(control_label_width),
            ),
            boxed(slider.frame_width(control_slider_width)),
        ]))
        .spacing(metrics.gap as f32)
        .alignment(Alignment::Center)
        .frame(control_row_width, target_height),
    )];
    let brightness_row = if snapshot.brightness_percent.is_some() {
        // Only the percentage text rebuilds during a drag. The row and the
        // independently subscribed Slider keep their elements and capture.
        boxed(BrightnessRow {
            value: brightness,
            action: action.clone(),
            width: control_row_width,
            label_width: control_label_width,
            slider_width: control_slider_width,
            gap: metrics.gap as f32,
            height: target_height,
        })
    } else {
        boxed(
            Text::new("Brightness unavailable")
                .text_style(TextStyle::Caption)
                .color(palette.text_secondary())
                .frame(control_row_width, target_height),
        )
    };
    rows.insert(0, brightness_row);
    let multiple_outputs = snapshot.audio.outputs.len() > 1;
    let nav_width = if multiple_outputs {
        metrics.column_width()
    } else {
        control_row_width
    };
    let nav_size = Size::new(nav_width, target_height);
    let system = page_button(
        if nav_width >= 108.0 {
            "System status"
        } else {
            "Status"
        },
        Icon::ChartBar,
        ControlCenterPage::System,
        nav_size,
        session,
    );
    if multiple_outputs {
        let audio = page_button(
            if nav_width >= 108.0 {
                "Audio outputs"
            } else {
                "Audio"
            },
            Icon::Volume2,
            ControlCenterPage::AudioOutputs(0),
            nav_size,
            session,
        );
        rows.push(control_row(
            vec![audio, system],
            control_row_width,
            target_height,
            metrics.gap as f32,
        ));
    } else {
        rows.push(system);
    }
    rows
}

fn audio_outputs_page(
    metrics: ControlCenterMetrics,
    index: usize,
    snapshot: &ControlCenterSnapshot,
    action: &State<Option<ControlCenterAction>>,
    session: &State<ControlCenterSession>,
) -> Vec<Box<dyn View>> {
    let width = metrics.content_width();
    let height = metrics.target_height as f32;
    let gap = metrics.gap as f32;
    let header = if metrics.audio_pages == 1 {
        back_button(Size::new(width, height), session)
    } else {
        let back_width = width * 0.35;
        let counter_width = 44.0_f32.min(width * 0.25);
        let arrow_size = Size::new(
            ((width - back_width - counter_width - 3.0 * gap) / 2.0).max(1.0),
            height,
        );
        let arrow = |icon, page: Option<usize>| {
            if let Some(page) = page {
                page_button(
                    "",
                    icon,
                    ControlCenterPage::AudioOutputs(page),
                    arrow_size,
                    session,
                )
            } else {
                boxed(Spacer::new().frame(arrow_size.width, height))
            }
        };
        control_row(
            vec![
                back_button(Size::new(back_width, height), session),
                arrow(Icon::ChevronLeft, index.checked_sub(1)),
                boxed(
                    Text::new(alloc::format!("{} / {}", index + 1, metrics.audio_pages))
                        .text_style(TextStyle::Caption)
                        .line_limit(Some(1))
                        .frame(counter_width, height),
                ),
                arrow(
                    Icon::ChevronRight,
                    (index + 1 < metrics.audio_pages).then_some(index + 1),
                ),
            ],
            width,
            height,
            gap,
        )
    };
    let mut rows = vec![header];
    let outputs = snapshot
        .audio
        .outputs
        .iter()
        .skip(index * metrics.audio_rows_per_page)
        .take(metrics.audio_rows_per_page);
    for output in outputs {
        let selected = snapshot.audio.current_output_id.as_ref() == Some(&output.id);
        let mut label = output.name.clone();
        if selected {
            label.push_str("  Selected");
        }
        let output_action = action.clone();
        let output_id = output.id.clone();
        let mut button = Button::new(label)
            .enabled(output.available)
            .header_style()
            .font_size(12.0)
            .icon(if selected {
                Icon::CircleCheck
            } else {
                Icon::Volume2
            });
        if output.available {
            button = button.on_click(move || {
                output_action.set(Some(ControlCenterAction::SelectOutput(output_id.clone())));
            });
        }
        rows.push(boxed(button.frame(width, height).clip()));
    }
    if snapshot.audio.outputs.is_empty() {
        rows.push(boxed(
            Text::new("No audio outputs")
                .text_style(TextStyle::Caption)
                .frame(width, height),
        ));
    }
    while rows.len() <= metrics.audio_rows {
        rows.push(boxed(Spacer::new().frame(width, height)));
    }
    rows
}

fn system_page(
    metrics: ControlCenterMetrics,
    snapshot: &ControlCenterSnapshot,
    action: &State<Option<ControlCenterAction>>,
    session: &State<ControlCenterSession>,
) -> Vec<Box<dyn View>> {
    let palette = ColorPalette::default();
    let width = metrics.content_width();
    let card_width = metrics.column_width();
    let height = metrics.details_height as f32;
    let network_detail = network_label(&snapshot.network);
    let system_detail = system_label(&snapshot.system);
    let network_action = action.clone();
    let network_settings = Button::new("Network settings")
        .padding(0.0)
        .header_style()
        .font_size(11.0)
        .on_click(move || {
            network_action.set(Some(ControlCenterAction::OpenSettings(
                ControlCenterSettingsLink::Network,
            )));
        });

    let caption = |label: String| {
        boxed(
            Text::new(label)
                .font_size(11.0)
                .line_limit(Some(1))
                .color(palette.text_secondary())
                .frame_width(card_width),
        )
    };
    let mut network = vec![caption(network_detail), boxed(network_settings.clip())];
    let mut system = vec![caption(system_detail), caption(snapshot.power.detail())];
    if height >= 78.0 {
        network.insert(0, boxed(Text::new("Network").font_size(14.0)));
        system.insert(0, boxed(Text::new("System").font_size(14.0)));
        system.push(caption(String::from(input_label(
            &snapshot.input_environment,
        ))));
    }
    let details = control_row(
        vec![
            content_section(card_width, height, network, 4.0),
            content_section(card_width, height, system, 4.0),
        ],
        width,
        height,
        metrics.gap as f32,
    );
    vec![
        back_button(Size::new(width, metrics.target_height as f32), session),
        details,
    ]
}

fn settings_button(
    metrics: ControlCenterMetrics,
    action: State<Option<ControlCenterAction>>,
) -> Box<dyn View> {
    boxed(
        Button::new("Settings")
            .icon(Icon::Settings)
            .header_style()
            .icon_size(IconSize::Small)
            .font_size(11.0)
            .padding(0.0)
            .on_click(move || {
                action.set(Some(ControlCenterAction::OpenSettings(
                    ControlCenterSettingsLink::AllSettings,
                )));
            })
            .frame(metrics.content_width(), metrics.target_height as f32),
    )
}

fn control_row(children: Vec<Box<dyn View>>, width: f32, height: f32, gap: f32) -> Box<dyn View> {
    boxed(
        HStack::new(DynamicViews::new(children))
            .spacing(gap)
            .alignment(Alignment::Center)
            .frame(width, height),
    )
}

fn content_section(
    width: f32,
    height: f32,
    children: Vec<Box<dyn View>>,
    spacing: f32,
) -> Box<dyn View> {
    boxed(
        VStack::new(DynamicViews::new(children))
            .spacing(spacing)
            .alignment(Alignment::Leading)
            .frame(width, height),
    )
}

#[derive(Clone)]
struct BrightnessRow {
    value: State<f32>,
    action: State<Option<ControlCenterAction>>,
    width: f32,
    label_width: f32,
    slider_width: f32,
    gap: f32,
    height: f32,
}

impl BrightnessRow {
    fn content(&self) -> impl View + Clone + use<> {
        let action = self.action.clone();
        let value = self.value.clone();
        HStack::new((
            VStack::new((
                Text::new("Brightness").text_style(TextStyle::Caption),
                BrightnessValueLabel(self.value.clone()),
            ))
            .spacing(0.0)
            .alignment(Alignment::Leading)
            .frame_width(self.label_width),
            Slider::new(self.value.clone())
                .min(0.0)
                .max(100.0)
                .on_change(move |next| {
                    action.set(Some(ControlCenterAction::SetBrightness(
                        next.clamp(0.0, 100.0).round() as u8,
                    )));
                    // Preserve the continuous input even if a concurrent idle
                    // sample landed between Slider's State.set and callback.
                    if value.get() != next {
                        value.set(next);
                    }
                })
                .frame_width(self.slider_width),
        ))
        .spacing(self.gap)
        .alignment(Alignment::Center)
        .frame(self.width, self.height)
    }
}

impl View for BrightnessRow {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new_with_builder(self.clone(), |row| {
            boxed(row.content())
        }))
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// Keep continuous slider updates out of the row's structural reconciliation.
/// Slider already subscribes to the same value to update its own render object.
#[derive(Clone)]
struct BrightnessValueLabel(State<f32>);

impl View for BrightnessValueLabel {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new_with_builder(self.clone(), |label| {
            boxed(
                Text::new(alloc::format!(
                    "{}%",
                    label.0.get().clamp(0.0, 100.0).round() as u8
                ))
                .font_size(10.0)
                .color(ColorPalette::default().text_secondary()),
            )
        }))
    }
    fn listenables(&self) -> Vec<&dyn scarlet_ui::state::Listenable> {
        vec![&self.0]
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// Reconcile a provider readback only when no input owns the continuous value.
/// Called by the sole hardware-command worker, after its synchronous write has
/// finished. Polling still records observations while dragging or queued.
pub(super) fn reconcile_brightness(
    value: &State<f32>,
    dragging: &State<bool>,
    observed: Option<u8>,
    pending: &State<Option<ControlCenterAction>>,
) {
    let Some(actual) = observed else {
        return;
    };
    let expected = value.get();
    if dragging.get()
        || matches!(pending.get(), Some(ControlCenterAction::SetBrightness(_)))
        || expected == actual as f32
    {
        return;
    }
    value.update(|current| {
        // Do not overwrite a newer input arriving after the initial check.
        if *current == expected
            && !dragging.get()
            && !matches!(pending.get(), Some(ControlCenterAction::SetBrightness(_)))
        {
            *current = actual as f32;
        }
    });
}

fn network_label(snapshot: &NetworkSnapshot) -> String {
    if !snapshot.available {
        return String::from("Unavailable");
    }
    let Some(interface) = snapshot.interfaces.first() else {
        return String::from("No connection");
    };
    let mut label = match interface.state {
        NetworkInterfaceState::Connected => String::from("Connected"),
        NetworkInterfaceState::Configuring => String::from("Obtaining IP"),
        NetworkInterfaceState::Disconnected => String::from("Disconnected"),
        NetworkInterfaceState::Unknown => String::from("Unknown"),
    };
    label.push_str(" / ");
    label.push_str(&interface.name);
    label
}

fn system_label(snapshot: &SystemSnapshot) -> String {
    let cpu = snapshot
        .cpu_percent
        .map(|value| alloc::format!("{}%", value))
        .unwrap_or_else(|| String::from("--"));
    let tasks = snapshot
        .task_count
        .map(|value| alloc::format!("{}", value))
        .unwrap_or_else(|| String::from("--"));
    alloc::format!("CPU {}   Tasks {}", cpu, tasks)
}

fn input_label(snapshot: &InputEnvironmentSnapshot) -> &'static str {
    if !snapshot.available {
        "Input unavailable"
    } else if snapshot.tablet_mode == Some(true) {
        "Tablet input"
    } else {
        "Laptop input"
    }
}

pub(super) fn boxed(view: impl View + Clone + 'static) -> Box<dyn View> {
    Box::new(view)
}

#[derive(Clone)]
pub(super) struct DynamicViews {
    children: Vec<Box<dyn View>>,
}

impl DynamicViews {
    pub(super) fn new(children: Vec<Box<dyn View>>) -> Self {
        Self { children }
    }
}

impl ViewTuple for DynamicViews {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        self.children
            .iter()
            .map(|child| child.create_element())
            .collect()
    }

    fn clone_views(&self) -> Vec<Box<dyn View>> {
        self.children.clone()
    }

    fn collect_listenables<'a>(
        &'a self,
        collector: &mut Vec<&'a dyn scarlet_ui::state::Listenable>,
    ) {
        for child in &self.children {
            collector.extend(child.listenables());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_ui::element::RenderElement;
    use scarlet_ui::state::StateId;
    use scarlet_ui::views::SliderRenderObject;

    fn controls_metrics(
        presentation: ControlCenterPresentation,
        output_count: usize,
    ) -> ControlCenterMetrics {
        ControlCenterMetrics::resolve(
            presentation,
            Size::new(800.0, 600.0),
            ControlCenterPage::Controls,
            output_count,
        )
    }

    #[test]
    fn percent_writes_deduplicate_acks_but_keep_changes_retries_and_final_targets() {
        let mut writes = PercentWriteState::default();
        assert!(
            writes.needs_write(42, Some(42)),
            "initial input still writes"
        );
        writes.complete(42, Some(41));
        assert!(
            !writes.needs_write(42, Some(41)),
            "rounded hardware ack may differ"
        );
        assert!(
            writes.needs_write(42, Some(20)),
            "external change permits same request"
        );
        assert!(
            writes.needs_write(42, None),
            "unknown provider cannot suppress writes"
        );
        assert!(
            writes.needs_write(43, Some(41)),
            "new final percentage must be sent"
        );
        writes.complete(43, None);
        assert!(
            writes.needs_write(43, Some(41)),
            "failure permits identical retry"
        );
        assert!(
            writes.needs_write(42, Some(41)),
            "failure also clears the old ack"
        );
        writes.complete(43, Some(43));
        assert!(!writes.needs_write(43, Some(43)));
        assert!(
            writes.needs_write(42, Some(43)),
            "reversing a drag must write again"
        );
    }

    fn brightness_slider(element: &dyn Element, id: StateId) -> Option<&Slider> {
        if let Some(slider) = element
            .as_any()
            .downcast_ref::<RenderElement<Slider, SliderRenderObject>>()
            && slider.view().get_value().id() == id
        {
            return Some(slider.view());
        }
        element
            .children()
            .iter()
            .find_map(|child| brightness_slider(child.as_ref(), id))
    }

    #[test]
    fn readback_waits_for_drag_and_latest_command_but_keeps_external_updates() {
        let value = State::new(scarlet_ui::state::generate_state_id(), 42.6);
        let dragging = Slider::new(value.clone()).get_dragging().clone();
        let pending = State::new(scarlet_ui::state::generate_state_id(), None);
        dragging.set(true);
        reconcile_brightness(&value, &dragging, Some(12), &pending);
        assert_eq!(value.get(), 42.6, "old poll cannot move a captured thumb");
        reconcile_brightness(&value, &dragging, Some(43), &pending);
        assert_eq!(
            value.get(),
            42.6,
            "integer hardware ack cannot quantize a drag"
        );
        value.set(80.3);
        pending.set(Some(ControlCenterAction::SetBrightness(80)));
        dragging.set(false);
        reconcile_brightness(&value, &dragging, Some(43), &pending);
        assert_eq!(
            value.get(),
            80.3,
            "release must wait for the final queued write"
        );
        pending.set(None);
        reconcile_brightness(&value, &dragging, Some(80), &pending);
        assert_eq!(value.get(), 80.0);
        reconcile_brightness(&value, &dragging, Some(22), &pending);
        assert_eq!(
            value.get(),
            22.0,
            "idle external changes and rejected writes still reconcile"
        );
        reconcile_brightness(&value, &dragging, None, &pending);
        assert_eq!(value.get(), 22.0, "unavailable is not a fictitious zero");
    }

    #[test]
    fn brightness_control_emits_percentages_only_when_available() {
        for (index, percent) in [None, Some(0), Some(59), Some(100)].into_iter().enumerate() {
            let brightness = State::new(
                StateId::new(2000 + index as u32),
                percent.unwrap_or(0) as f32,
            );
            let action = State::new(StateId::new(2010 + index as u32), None);
            let snapshot = ControlCenterSnapshot {
                power: Default::default(),
                audio: AudioSnapshot::unavailable(),
                brightness_percent: percent,
                network: NetworkSnapshot {
                    available: false,
                    interfaces: Vec::new(),
                },
                system: SystemSnapshot {
                    cpu_percent: None,
                    task_count: None,
                },
                input_environment: InputEnvironmentSnapshot {
                    available: false,
                    tablet_mode: None,
                    touch_present: None,
                    keyboard_present: None,
                    pointer_present: None,
                },
            };
            let view = build_control_center_view(
                controls_metrics(ControlCenterPresentation::LaptopPopover, 0),
                snapshot,
                State::new(StateId::new(2020 + index as u32), 0.0),
                brightness.clone(),
                action.clone(),
                State::new(
                    StateId::new(2040 + index as u32),
                    ControlCenterSession::default(),
                ),
            );
            let element = view.create_element();
            let slider = brightness_slider(element.as_ref(), brightness.id());
            assert_eq!(slider.is_some(), percent.is_some());
            if let Some(slider) = slider {
                assert_eq!(slider.get_value().get(), percent.unwrap() as f32);
                for (value, expected) in [(-1.0, 0), (42.6, 43), (101.0, 100)] {
                    slider.invoke_on_change(value);
                    assert_eq!(
                        action.get(),
                        Some(ControlCenterAction::SetBrightness(expected))
                    );
                }
            }
        }
    }

    #[test]
    fn controls_keep_natural_centers_and_stable_tracks_across_percentages() {
        use scarlet_ui::element::LayoutConstraints;
        use scarlet_ui::geometry::{Point, Rect};
        use scarlet_ui::views::{ButtonRenderObject, TextRenderObject};

        fn collect(element: &dyn Element, origin: Point, out: &mut Vec<(String, Rect)>) {
            let origin = Point::new(
                origin.x + element.position().x,
                origin.y + element.position().y,
            );
            let bounds = Rect {
                origin,
                size: element.bounds().size,
            };
            let name = if let Some(text) = element
                .as_any()
                .downcast_ref::<RenderElement<Text, TextRenderObject>>()
            {
                Some(String::from(text.view().content()))
            } else if element
                .render_object()
                .is_some_and(|r| r.as_any().is::<SliderRenderObject>())
            {
                Some(String::from("slider"))
            } else if element
                .render_object()
                .is_some_and(|r| r.as_any().is::<ButtonRenderObject>())
            {
                Some(String::from("button"))
            } else {
                None
            };
            if let Some(name) = name {
                out.push((name, bounds));
            }
            for child in element.children() {
                collect(child.as_ref(), origin, out);
            }
        }

        for presentation in [
            ControlCenterPresentation::LaptopPopover,
            ControlCenterPresentation::TabletSheet,
        ] {
            let mut previous_tracks = None;
            for percent in [0, 9, 10, 80, 99, 100] {
                let snapshot = ControlCenterSnapshot {
                    power: Default::default(),
                    audio: AudioSnapshot::unavailable(),
                    brightness_percent: Some(percent),
                    network: NetworkSnapshot {
                        available: false,
                        interfaces: Vec::new(),
                    },
                    system: SystemSnapshot {
                        cpu_percent: None,
                        task_count: None,
                    },
                    input_environment: InputEnvironmentSnapshot {
                        available: false,
                        tablet_mode: None,
                        touch_present: None,
                        keyboard_present: None,
                        pointer_present: None,
                    },
                };
                let mut element = build_control_center_view(
                    controls_metrics(presentation, 0),
                    snapshot,
                    State::new(scarlet_ui::state::generate_state_id(), 25.0),
                    State::new(scarlet_ui::state::generate_state_id(), percent as f32),
                    State::new(scarlet_ui::state::generate_state_id(), None),
                    State::new(
                        scarlet_ui::state::generate_state_id(),
                        ControlCenterSession::default(),
                    ),
                )
                .create_element();
                let size = element.layout(LayoutConstraints::unconstrained());
                fn captures(element: &dyn Element) -> usize {
                    usize::from(element.render_object().is_some_and(|r| r.reads_backdrop()))
                        + element
                            .children()
                            .iter()
                            .map(|child| captures(child.as_ref()))
                            .sum::<usize>()
                }
                assert_eq!(
                    captures(element.as_ref()),
                    2,
                    "only the two standard slider thumbs read local backdrop"
                );
                let mut rects = Vec::new();
                collect(element.as_ref(), Point::ZERO, &mut rects);
                let tracks: Vec<_> = rects
                    .iter()
                    .filter(|(name, _)| name == "slider")
                    .map(|(_, rect)| *rect)
                    .collect();
                assert_eq!(tracks.len(), 2);
                if let Some(previous) = previous_tracks {
                    assert_eq!(tracks, previous);
                }
                previous_tracks = Some(tracks.clone());
                assert_eq!(tracks[0].origin.x, tracks[1].origin.x);
                assert_eq!(tracks[0].size.width, tracks[1].size.width);
                for track in &tracks {
                    assert!(
                        track.origin.x >= 0.0 && track.origin.x + track.size.width <= size.width
                    );
                }
                let label = rects
                    .iter()
                    .find(|(name, _)| name == "Brightness")
                    .unwrap()
                    .1;
                let value = rects
                    .iter()
                    .find(|(name, _)| name == &alloc::format!("{}%", percent))
                    .unwrap()
                    .1;
                assert_eq!(label.origin.x, value.origin.x);
                assert!(value.origin.y >= label.origin.y + label.size.height);
                let label_center = (label.origin.y + value.origin.y + value.size.height) / 2.0;
                assert_eq!(
                    label_center,
                    tracks[0].origin.y + tracks[0].size.height / 2.0
                );
                let mute = rects.iter().find(|(name, _)| name == "button").unwrap().1;
                assert_eq!(mute.origin.x, label.origin.x);
                assert_eq!(
                    mute.origin.y + mute.size.height / 2.0,
                    tracks[1].origin.y + tracks[1].size.height / 2.0
                );
            }
        }
    }

    #[test]
    fn brightness_row_fits_pointer_and_touch_metrics() {
        let laptop = controls_metrics(ControlCenterPresentation::LaptopPopover, 1);
        let tablet = controls_metrics(ControlCenterPresentation::TabletSheet, 1);
        assert_eq!(laptop.width, 304);
        assert_eq!(
            laptop.height,
            2 * laptop.margin + 4 * laptop.target_height + 3 * laptop.gap
        );
        assert!(laptop.height <= 400);
        assert_eq!(tablet.width, 392);
        assert_eq!(
            tablet.height,
            2 * tablet.margin + 4 * tablet.target_height + 3 * tablet.gap
        );
        assert!(tablet.target_height > laptop.target_height);
    }

    #[test]
    fn scene_declares_only_the_managed_control_center_body() {
        let metrics = controls_metrics(ControlCenterPresentation::LaptopPopover, 1);
        let body = metrics.body_size();

        assert_eq!(body.width, metrics.width as f32);
        assert_eq!(body.height, metrics.height as f32);
        let region = backdrop_region(body);
        let outsets = ElevationRole::Floating.paint_outsets();
        assert_eq!(region.rect.origin, Point::new(outsets.left, outsets.top));
        assert_eq!(region.rect.size, body);
        assert_eq!(region.corner_radius, ControlCenterMetrics::CORNER_RADIUS);
        assert!(region.accepts_input);
        assert!(region.rect.origin.x + body.width <= body.width + outsets.left + outsets.right);
        assert!(region.rect.origin.y + body.height <= body.height + outsets.top + outsets.bottom);
    }

    #[test]
    fn multiple_audio_outputs_share_one_link_on_the_controls_page() {
        let two = controls_metrics(ControlCenterPresentation::LaptopPopover, 2);
        let three = controls_metrics(ControlCenterPresentation::LaptopPopover, 3);
        assert_eq!(three.height, two.height);
    }

    #[test]
    fn short_viewport_paginates_audio_outputs_with_all_navigation_visible() {
        let available = Size::new(280.0, 156.0);
        let metrics = ControlCenterMetrics::resolve(
            ControlCenterPresentation::TabletSheet,
            available,
            ControlCenterPage::AudioOutputs(99),
            12,
        );
        assert!(metrics.body_size().height <= available.height);
        assert_eq!(metrics.audio_rows, 2);
        assert_eq!(metrics.audio_pages, 6);
        assert_eq!(metrics.page, ControlCenterPage::AudioOutputs(5));
        assert_eq!(
            metrics.height,
            2 * metrics.margin + 4 * metrics.target_height + 3 * metrics.gap
        );
    }

    #[test]
    fn unavailable_audio_does_not_reserve_a_temporary_selector_row() {
        for presentation in [
            ControlCenterPresentation::LaptopPopover,
            ControlCenterPresentation::TabletSheet,
        ] {
            let none = controls_metrics(presentation, 0);
            let one = controls_metrics(presentation, 1);
            let two = controls_metrics(presentation, 2);
            assert_eq!(none.height, one.height);
            assert_eq!(two.height, one.height);
        }
    }

    #[test]
    fn opening_requires_its_own_completed_sample_including_unavailable_results() {
        let mut session = ControlCenterSession::default();
        session.request_open();
        let cancelled = session.generation();
        assert!(session.open && session.snapshot.is_none());
        session.close();
        session.publish(cancelled, ControlCenterSnapshot::unavailable());
        assert!(
            session.snapshot.is_none(),
            "a closed request cannot publish"
        );

        session.request_open();
        session.publish(cancelled, ControlCenterSnapshot::unavailable());
        assert!(
            session.snapshot.is_none(),
            "old IPC cannot complete a reopen"
        );
        session.publish(session.generation(), ControlCenterSnapshot::unavailable());
        assert!(
            session.snapshot.is_some(),
            "unavailable is a completed sample"
        );
        session.close();
        session.request_open();
        assert!(session.snapshot.is_none(), "reopen samples fresh topology");
    }

    #[test]
    fn network_labels_only_report_provider_state() {
        let unavailable = NetworkSnapshot {
            available: false,
            interfaces: Vec::new(),
        };
        assert_eq!(network_label(&unavailable), "Unavailable");
        let connected = NetworkSnapshot {
            available: true,
            interfaces: vec![NetworkInterfaceSnapshot {
                name: String::from("veth0"),
                state: NetworkInterfaceState::Connected,
                detail: None,
            }],
        };
        assert_eq!(network_label(&connected), "Connected / veth0");
    }
}
