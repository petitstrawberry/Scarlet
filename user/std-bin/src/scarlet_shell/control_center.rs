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
use scarlet_ui::element::Element;
use scarlet_ui::geometry::{Alignment, Size};
use scarlet_ui::state::State;
use scarlet_ui::view::{View, ViewExt};
use scarlet_ui::views::containers::ViewTuple;
use scarlet_ui::views::{Button, HStack, Slider, Spacer, Surface, SurfaceRole, Text, VStack};
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
    /// Arm power-off confirmation.
    ArmPowerOff,
    /// Confirm a previously armed power-off request.
    ConfirmPowerOff,
    /// Arm reboot confirmation.
    ArmReboot,
    /// Confirm a previously armed reboot request.
    ConfirmReboot,
}

/// Power action awaiting a confirming second press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmedPowerAction {
    /// Power-off is armed.
    PowerOff,
    /// Reboot is armed.
    Reboot,
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
    controls_height: u32,
    details_height: u32,
}

impl ControlCenterMetrics {
    /// Corner radius shared by the managed window body and floating surface.
    pub const CORNER_RADIUS: f32 = 8.0;

    /// Resolve the approved compact shell dimensions.
    ///
    /// # Arguments
    ///
    /// * `presentation` - Current shell presentation.
    /// * `output_count` - Number of real audio-output rows.
    ///
    /// # Returns
    ///
    /// Presentation-specific dimensions for pointer and touch targets.
    pub fn resolve(presentation: ControlCenterPresentation, output_count: usize) -> Self {
        let (width, margin, gap, target_height, details_height) = match presentation {
            ControlCenterPresentation::LaptopPopover => (304, 8, 6, 32, 116),
            ControlCenterPresentation::TabletSheet => (392, 16, 10, 44, 144),
        };
        let output_rows = if output_count > 1 {
            output_count as u32
        } else if output_count == 0 {
            1
        } else {
            0
        };
        let controls_height = (2 + output_rows) * target_height + (1 + output_rows) * gap + 12;
        let height = margin * 2 + controls_height + details_height + target_height + gap * 2;
        Self {
            width,
            height,
            margin,
            gap,
            target_height,
            controls_height,
            details_height,
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
}

/// Build the retained ScarletUI Control Center content.
///
/// # Arguments
///
/// * `presentation` - Laptop popover or tablet sheet.
/// * `snapshot` - Current provider snapshot.
/// * `volume` - Shared standard Slider value state.
/// * `brightness` - Shared display Slider value state, updated after readback.
/// * `action` - Typed action queue consumed by the StatusBar service loop.
/// * `armed_power` - Current two-step power confirmation state.
///
/// # Returns
///
/// A normal ScarletUI view tree. No drawing or input dispatch is implemented
/// by this module.
pub fn build_control_center_view(
    presentation: ControlCenterPresentation,
    snapshot: ControlCenterSnapshot,
    volume: State<f32>,
    brightness: State<f32>,
    action: State<Option<ControlCenterAction>>,
    armed_power: State<Option<ArmedPowerAction>>,
) -> impl View + Clone {
    let metrics = ControlCenterMetrics::resolve(presentation, snapshot.audio.outputs.len());
    let palette = ColorPalette::default();
    let content_width = metrics.width as f32 - metrics.margin as f32 * 2.0;
    let card_width = (content_width - metrics.gap as f32) / 2.0;
    let title_size = 14.0;
    let detail_size = 12.0;
    let target_height = metrics.target_height as f32;

    let mute_action = action.clone();
    let mute = Button::new(if snapshot.audio.muted == Some(true) {
        "Muted"
    } else {
        "Volume"
    })
    .header_style()
    .font_size(detail_size)
    .on_click(move || mute_action.set(Some(ControlCenterAction::ToggleMute)));

    let slider_action = action.clone();
    let slider = Slider::new(volume)
        .min(0.0)
        .max(100.0)
        .on_change(move |value| {
            slider_action.set(Some(ControlCenterAction::SetVolume(
                value.clamp(0.0, 100.0) as u8,
            )));
        });

    let mut audio_rows: Vec<Box<dyn View>> = vec![boxed(
        HStack::new(DynamicViews::new(vec![boxed(mute), boxed(slider)]))
            .spacing(metrics.gap as f32)
            .alignment(Alignment::Center)
            .frame(content_width - 12.0, target_height),
    )];
    if snapshot.audio.outputs.is_empty() {
        audio_rows.push(boxed(
            Text::new("Audio output unavailable")
                .font_size(detail_size)
                .color(palette.text_secondary()),
        ));
    } else if snapshot.audio.outputs.len() > 1 {
        for output in &snapshot.audio.outputs {
            let selected = snapshot.audio.current_output_id.as_ref() == Some(&output.id);
            let mut label = output.name.clone();
            if selected {
                label.push_str("  Selected");
            }
            let output_action = action.clone();
            let output_id = output.id.clone();
            let mut button = Button::new(label)
                .header_style()
                .font_size(detail_size)
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
            audio_rows.push(boxed(button));
        }
    }

    let brightness_row = if let Some(percent) = snapshot.brightness_percent {
        let brightness_action = action.clone();
        boxed(
            HStack::new(DynamicViews::new(vec![
                boxed(Text::new(alloc::format!("Brightness {}%", percent)).font_size(detail_size)),
                boxed(
                    Slider::new(brightness)
                        .min(0.0)
                        .max(100.0)
                        .on_change(move |value| {
                            brightness_action.set(Some(ControlCenterAction::SetBrightness(
                                value.clamp(0.0, 100.0).round() as u8,
                            )));
                        }),
                ),
            ]))
            .spacing(metrics.gap as f32)
            .alignment(Alignment::Center)
            .frame(content_width - 12.0, target_height),
        )
    } else {
        boxed(
            Text::new("Brightness unavailable")
                .font_size(detail_size)
                .color(palette.text_secondary())
                .frame(content_width - 12.0, target_height),
        )
    };
    audio_rows.insert(0, brightness_row);
    let controls = translucent_section(
        content_width,
        metrics.controls_height as f32,
        audio_rows,
        metrics.gap as f32,
    );

    let network_detail = network_label(&snapshot.network);
    let network_action = action.clone();
    let network = translucent_section(
        card_width,
        metrics.details_height as f32,
        vec![
            boxed(Text::new("Network").font_size(title_size)),
            boxed(
                Text::new(network_detail)
                    .font_size(detail_size)
                    .color(palette.text_secondary()),
            ),
            boxed(
                Button::new("Network settings")
                    .header_style()
                    .font_size(detail_size)
                    .on_click(move || {
                        network_action.set(Some(ControlCenterAction::OpenSettings(
                            ControlCenterSettingsLink::Network,
                        )));
                    }),
            ),
        ],
        4.0,
    );

    let system = translucent_section(
        card_width,
        metrics.details_height as f32,
        vec![
            boxed(Text::new("System").font_size(title_size)),
            boxed(Text::new(system_label(&snapshot.system)).font_size(detail_size)),
            boxed(Text::new(snapshot.power.detail()).font_size(11.0)),
            boxed(
                Text::new(input_label(&snapshot.input_environment))
                    .font_size(11.0)
                    .color(palette.text_secondary()),
            ),
        ],
        4.0,
    );

    let settings_action = action.clone();
    let settings = Button::icon_only(Icon::Settings)
        .header_style()
        .icon_size(IconSize::Medium)
        .icon_color(palette.text_secondary())
        .on_click(move || {
            settings_action.set(Some(ControlCenterAction::OpenSettings(
                ControlCenterSettingsLink::AllSettings,
            )));
        })
        .frame(target_height, target_height);

    let shutdown_action = action.clone();
    let shutdown_armed = armed_power.clone();
    let shutdown = Button::icon_only(Icon::Power)
        .header_style()
        .icon_size(IconSize::Medium)
        .icon_color(if armed_power.get() == Some(ArmedPowerAction::PowerOff) {
            palette.primary()
        } else {
            palette.text_secondary()
        })
        .on_click(move || {
            let next = if shutdown_armed.get() == Some(ArmedPowerAction::PowerOff) {
                shutdown_armed.set(None);
                ControlCenterAction::ConfirmPowerOff
            } else {
                shutdown_armed.set(Some(ArmedPowerAction::PowerOff));
                ControlCenterAction::ArmPowerOff
            };
            shutdown_action.set(Some(next));
        })
        .frame(target_height, target_height);

    let reboot_action = action;
    let reboot_armed = armed_power.clone();
    let reboot = Button::icon_only(Icon::Refresh)
        .header_style()
        .icon_size(IconSize::Medium)
        .icon_color(if armed_power.get() == Some(ArmedPowerAction::Reboot) {
            palette.primary()
        } else {
            palette.text_secondary()
        })
        .on_click(move || {
            let next = if reboot_armed.get() == Some(ArmedPowerAction::Reboot) {
                reboot_armed.set(None);
                ControlCenterAction::ConfirmReboot
            } else {
                reboot_armed.set(Some(ArmedPowerAction::Reboot));
                ControlCenterAction::ArmReboot
            };
            reboot_action.set(Some(next));
        })
        .frame(target_height, target_height);

    let details = HStack::new(DynamicViews::new(vec![network, system]))
        .spacing(metrics.gap as f32)
        .alignment(Alignment::Top);
    let footer = HStack::new(DynamicViews::new(vec![
        boxed(settings),
        boxed(Spacer::new()),
        boxed(shutdown),
        boxed(Spacer::new()),
        boxed(reboot),
    ]))
    .alignment(Alignment::Center)
    .frame(content_width, target_height);

    let content = VStack::new(DynamicViews::new(vec![
        controls,
        boxed(details),
        boxed(footer),
    ]))
    .spacing(metrics.gap as f32)
    .alignment(Alignment::Leading)
    .padding(metrics.margin as f32);

    Surface::new(content, SurfaceRole::Floating)
        .fill(palette.surface().with_opacity(0.88))
        .bordered(true)
        .corner_radius(ControlCenterMetrics::CORNER_RADIUS)
        .frame(metrics.width as f32, metrics.height as f32)
}

fn translucent_section(
    width: f32,
    height: f32,
    children: Vec<Box<dyn View>>,
    spacing: f32,
) -> Box<dyn View> {
    let palette = ColorPalette::default();
    boxed(
        Surface::new(
            VStack::new(DynamicViews::new(children))
                .spacing(spacing)
                .alignment(Alignment::Leading)
                .padding(6.0),
            SurfaceRole::Section,
        )
        .fill(palette.surface().with_opacity(0.45))
        .frame(width, height),
    )
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
                ControlCenterPresentation::LaptopPopover,
                snapshot,
                State::new(StateId::new(2020 + index as u32), 0.0),
                brightness.clone(),
                action.clone(),
                State::new(StateId::new(2030 + index as u32), None),
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
    fn brightness_row_fits_pointer_and_touch_metrics() {
        let laptop = ControlCenterMetrics::resolve(ControlCenterPresentation::LaptopPopover, 1);
        let tablet = ControlCenterMetrics::resolve(ControlCenterPresentation::TabletSheet, 1);
        assert_eq!(laptop.width, 304);
        assert_eq!(
            laptop.controls_height,
            2 * laptop.target_height + laptop.gap + 12
        );
        assert!(laptop.height <= 320);
        assert_eq!(tablet.width, 392);
        assert_eq!(
            tablet.controls_height,
            2 * tablet.target_height + tablet.gap + 12
        );
        assert!(tablet.target_height > laptop.target_height);
    }

    #[test]
    fn scene_declares_only_the_managed_control_center_body() {
        let metrics = ControlCenterMetrics::resolve(ControlCenterPresentation::LaptopPopover, 1);
        let body = metrics.body_size();

        assert_eq!(body.width, metrics.width as f32);
        assert_eq!(body.height, metrics.height as f32);
    }

    #[test]
    fn dynamic_audio_outputs_add_exactly_one_standard_row() {
        let two = ControlCenterMetrics::resolve(ControlCenterPresentation::LaptopPopover, 2);
        let three = ControlCenterMetrics::resolve(ControlCenterPresentation::LaptopPopover, 3);
        assert_eq!(three.height - two.height, two.target_height + two.gap);
    }

    #[test]
    fn one_audio_output_does_not_consume_a_selector_row() {
        let none = ControlCenterMetrics::resolve(ControlCenterPresentation::LaptopPopover, 0);
        let one = ControlCenterMetrics::resolve(ControlCenterPresentation::LaptopPopover, 1);
        let two = ControlCenterMetrics::resolve(ControlCenterPresentation::LaptopPopover, 2);
        assert_eq!(none.height - one.height, one.target_height + one.gap);
        assert_eq!(two.height - one.height, 2 * (one.target_height + one.gap));
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
