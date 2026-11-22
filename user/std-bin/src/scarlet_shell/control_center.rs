//! ScarletUI Control Center scene content.
//!
//! The shell owns provider sampling. This module only maps immutable provider
//! snapshots to ordinary ScarletUI views and typed actions. Rendering, layout,
//! hit testing, hover, press, and slider capture remain owned by ScarletUI.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;

use scarlet_ui::animation::{AnimatedValue, Animation, Transaction};
use scarlet_ui::color::ColorPalette;
use scarlet_ui::element::{
    ComponentElement, Element, ElementRenderObject, LayoutConstraints, RenderElement, UpdateResult,
};
use scarlet_ui::geometry::{Alignment, Point, Rect, Size};
use scarlet_ui::state::State;
use scarlet_ui::view::{View, ViewExt};
use scarlet_ui::views::containers::ViewTuple;
use scarlet_ui::views::{
    Button, ElevationRole, GlassMaterial, HStack, Slider, Spacer, Surface, SurfaceRole,
    SurfaceStyle, Text, TextStyle, VStack,
};
use scarlet_ui::{Icon, IconSize, IconView};

use super::{SessionAction, network_status::NetworkKind};

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
    /// Link type reported by the network provider.
    pub kind: NetworkKind,
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
    Network,
    System,
    Power,
    Session,
    Confirm(SessionAction),
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
    OpenSettings,
    /// Ask the provider worker to request a system power transition.
    Power(super::power_request::PowerAction),
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
    tile_height: u32,
    volume_height: u32,
    page: ControlCenterPage,
    details_height: u32,
    audio_pages: usize,
    audio_rows_per_page: usize,
}

impl ControlCenterMetrics {
    /// Corner radius shared by the managed window body and floating surface.
    pub const CORNER_RADIUS: f32 = 16.0;

    /// Fit the panel below the status bar and paginate its contents within that size.
    pub fn resolve(
        presentation: ControlCenterPresentation,
        available: Size,
        page: ControlCenterPage,
        output_count: usize,
    ) -> Self {
        let (preferred_width, preferred_margin, preferred_target) = match presentation {
            ControlCenterPresentation::LaptopPopover => (304, 8, 32),
            ControlCenterPresentation::TabletSheet => (392, 10, 44),
        };
        let width = preferred_width.min((available.width as u32).max(1));
        let height_limit = (available.height as u32).max(1);
        let margin = preferred_margin.min((width.min(height_limit) - 1) / 6);
        let inner_height = height_limit - 2 * margin;
        let gap = 6.min(inner_height / 12);
        let target_height = preferred_target.min(((inner_height - 3 * gap) / 4).max(1));
        let row_extent = target_height + gap;
        let controls_extra = inner_height.saturating_sub(4 * row_extent - gap).min(16);
        let tile_height = target_height + controls_extra / 2;
        let volume_height = target_height + controls_extra - controls_extra / 2;
        // Every page uses the Controls page's dimensions. The detail area
        // sits between one header row and the same pinned Settings footer.
        let content_height = (4 * row_extent - gap + controls_extra).min(inner_height);
        let details_height = content_height.saturating_sub(2 * row_extent);
        let audio_rows_per_page = ((details_height + gap) / row_extent).max(1) as usize;
        let audio_pages = output_count.div_ceil(audio_rows_per_page).max(1);
        let page = match page {
            ControlCenterPage::AudioOutputs(index) => {
                ControlCenterPage::AudioOutputs(index.min(audio_pages - 1))
            }
            page => page,
        };
        Self {
            width,
            height: content_height + 2 * margin,
            margin,
            gap,
            target_height,
            tile_height,
            volume_height,
            page,
            details_height,
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
    let rows = match metrics.page {
        ControlCenterPage::Controls => {
            controls_page(metrics, &snapshot, volume, brightness, &action, &session)
        }
        ControlCenterPage::AudioOutputs(index) => {
            audio_outputs_page(metrics, index, &snapshot, &action, &session)
        }
        ControlCenterPage::Network | ControlCenterPage::System => {
            details_page(metrics, &snapshot, &session)
        }
        ControlCenterPage::Power | ControlCenterPage::Session => session_page(metrics, &session),
        ControlCenterPage::Confirm(confirmed) => {
            confirmation_page(metrics, confirmed, &action, &session)
        }
    };
    let page = VStack::new(DynamicViews::new(rows))
        .spacing(gap)
        .alignment(Alignment::Leading);
    let content = VStack::new(DynamicViews::new(vec![
        boxed(PageFade {
            inner: page,
            page: metrics.page,
        }),
        footer_row(metrics, &snapshot, action, &session),
    ]))
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

/// Animate page changes on the UI frame clock while leaving the panel and footer in place.
#[derive(Clone)]
struct PageFade<V> {
    inner: V,
    page: ControlCenterPage,
}

impl<V: View + Clone + 'static> View for PageFade<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_view_children_and_updater(
            self.clone(),
            |view| PageFadeRenderObject::new(view.page),
            |render, view| render.show_page(view.page),
            |view| vec![view.inner.clone_view()],
        ))
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

struct PageFadeRenderObject {
    page: ControlCenterPage,
    opacity: AnimatedValue<f32>,
    size: Size,
}

impl PageFadeRenderObject {
    fn new(page: ControlCenterPage) -> Self {
        Self {
            page,
            opacity: AnimatedValue::new(1.0),
            size: Size::ZERO,
        }
    }

    fn show_page(&mut self, page: ControlCenterPage) -> UpdateResult {
        if self.page == page {
            return UpdateResult::NoChange;
        }
        self.page = page;
        if Transaction::current().disables_animations {
            self.opacity.finish();
        } else {
            self.opacity = AnimatedValue::new(0.65);
            self.opacity
                .retarget(1.0, Some(Animation::ease_out(Duration::from_millis(140))));
        }
        UpdateResult::Updated
    }
}

impl ElementRenderObject for PageFadeRenderObject {
    fn hit_test(&self, _point: Point) -> bool {
        false
    }
    fn presentation_affects_subtree_bounds(&self) -> bool {
        true
    }
    fn has_opacity_scope(&self) -> bool {
        true
    }
    fn update_needs_composite(&self) -> bool {
        true
    }
    fn update_needs_layout(&self) -> bool {
        false
    }
    fn update_needs_subtree_paint(&self) -> bool {
        false
    }
    fn subtree_opacity(&self) -> f32 {
        self.opacity.value()
    }
    fn has_visual_animation(&self) -> bool {
        self.opacity.is_active()
    }
    fn advance_visual_animation(&mut self, delta: Duration) -> bool {
        self.opacity.advance(delta)
    }
    fn finish_visual_animation(&mut self) -> bool {
        self.opacity.finish()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.size = constraints.constrain(Size::ZERO);
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        let Some(child) = children.first_mut() else {
            return self.layout(constraints);
        };
        self.size = child.layout(constraints);
        child.set_position(Point::ZERO);
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }
    fn render(&mut self) {}
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
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

fn details_header(
    title: &str,
    metrics: ControlCenterMetrics,
    session: &State<ControlCenterSession>,
) -> Box<dyn View> {
    let width = metrics.content_width();
    let height = metrics.target_height as f32;
    let gap = metrics.gap as f32;
    let back_width = 62.0_f32.min(width / 2.0);
    control_row(
        vec![
            back_button(Size::new(back_width, height), session),
            boxed(
                Text::new(title)
                    .font_size(13.0)
                    .alignment(Alignment::Leading)
                    .frame_width((width - back_width - gap).max(1.0)),
            ),
        ],
        width,
        height,
        gap,
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
    let width = metrics.content_width();
    let gap = metrics.gap as f32;
    let tile_size = Size::new(metrics.column_width(), metrics.tile_height as f32);
    let interface = snapshot.network.interfaces.first();
    let network_title = match interface.map(|link| link.kind) {
        Some(NetworkKind::Wifi) => "Wi-Fi",
        Some(NetworkKind::Ethernet) => "Wired network",
        None => "Network",
    };
    let network_icon = match interface.map(|link| link.kind) {
        Some(NetworkKind::Wifi) => Icon::Wifi,
        _ => Icon::Network,
    };
    let network = status_tile(
        network_icon,
        network_title,
        String::from(network_summary(&snapshot.network)),
        tile_size,
        Some(session),
    );
    let power = snapshot.power;
    let power_detail = match power.percent {
        Some(percent) => alloc::format!("{percent}% · {}", power.source_label()),
        None => String::from(power.source_label()),
    };
    let power_icon = if matches!(
        power.icon(),
        Some(super::power::PowerIcon::Charging | super::power::PowerIcon::Plugged)
    ) {
        Icon::BatteryCharging
    } else {
        Icon::Battery
    };
    let power = status_tile(
        power_icon,
        if power.battery_detected {
            "Battery"
        } else {
            "Power"
        },
        power_detail,
        tile_size,
        None,
    );
    let status = control_row(vec![network, power], width, tile_size.height, gap);
    let levels = control_levels(metrics, snapshot, volume, brightness, action, session);
    vec![status, levels.0, levels.1]
}

fn status_tile(
    icon: Icon,
    title: &str,
    detail: String,
    size: Size,
    network_page: Option<&State<ControlCenterSession>>,
) -> Box<dyn View> {
    let palette = ColorPalette::default();
    let gap = 4.0;
    let padding = 5.0;
    let icon_width = 16.0;
    let disclosure = if network_page.is_some() { 18.0 } else { 0.0 };
    let label_width = (size.width - 2.0 * padding - icon_width - gap - disclosure).max(1.0);
    let label = VStack::new((
        Text::new(title)
            .font_size(11.0)
            .line_limit(Some(1))
            .alignment(Alignment::Leading)
            .frame(label_width, 13.0),
        Text::new(detail)
            .font_size(10.0)
            .line_limit(Some(1))
            .color(palette.text_secondary())
            .alignment(Alignment::Leading)
            .frame(label_width, 13.0),
    ))
    .spacing(0.0)
    .alignment(Alignment::Leading);
    let mut children = vec![
        boxed(
            IconView::new(icon)
                .size(IconSize::Small)
                .frame_width(icon_width),
        ),
        boxed(label),
    ];
    if let Some(session) = network_page {
        children.push(page_button(
            "",
            Icon::ChevronRight,
            ControlCenterPage::Network,
            Size::new(disclosure - gap, size.height),
            session,
        ));
    }
    let tile = Surface::section(
        HStack::new(DynamicViews::new(children))
            .spacing(gap)
            .alignment(Alignment::Center)
            .padding_insets(scarlet_ui::geometry::EdgeInsets::symmetric(0.0, padding)),
    )
    .fill(palette.header_button_hover())
    .corner_radius(9.0)
    .frame(size.width, size.height);
    if let Some(session) = network_page {
        let session = session.clone();
        boxed(
            tile.on_click(move || {
                session.update(|current| current.page = ControlCenterPage::Network)
            }),
        )
    } else {
        boxed(tile)
    }
}

fn control_levels(
    metrics: ControlCenterMetrics,
    snapshot: &ControlCenterSnapshot,
    volume: State<f32>,
    brightness: State<f32>,
    action: &State<Option<ControlCenterAction>>,
    session: &State<ControlCenterSession>,
) -> (Box<dyn View>, Box<dyn View>) {
    let width = metrics.content_width();
    let gap = metrics.gap as f32;
    let brightness_height = metrics.target_height as f32;
    let volume_height = metrics.volume_height as f32;
    let leading_width = 26.0;
    let value_width = 31.0;
    let track_width = (width - leading_width - value_width - gap * 2.0).max(1.0);
    let brightness_icon = boxed(
        IconView::new(Icon::Sun)
            .size(IconSize::Small)
            .frame_width(leading_width),
    );

    let brightness_row = if snapshot.brightness_percent.is_some() {
        let request = action.clone();
        let value = brightness.clone();
        let slider = Slider::new(brightness.clone())
            .min(0.0)
            .max(100.0)
            .on_change(move |next| {
                request.set(Some(ControlCenterAction::SetBrightness(
                    next.clamp(0.0, 100.0).round() as u8,
                )));
                // Reassert the continuous value if a provider sample arrived during this input.
                if value.get() != next {
                    value.set(next);
                }
            });
        control_row(
            vec![
                brightness_icon,
                boxed(level_stack(
                    boxed(Text::new("Brightness").text_style(TextStyle::Caption)),
                    slider,
                    track_width,
                    brightness_height,
                )),
                boxed(LevelValueLabel(brightness).frame_width(value_width)),
            ],
            width,
            brightness_height,
            gap,
        )
    } else {
        unavailable_level(
            brightness_icon,
            "Brightness unavailable",
            width,
            brightness_height,
            gap,
        )
    };

    let muted = snapshot.audio.muted == Some(true);
    let mute_request = action.clone();
    let volume_icon = boxed(
        Button::icon_only(if muted {
            Icon::VolumeOff
        } else {
            Icon::Volume2
        })
        .header_style()
        .icon_size(IconSize::Small)
        .padding(0.0)
        .enabled(snapshot.audio.muted.is_some())
        .on_click(move || mute_request.set(Some(ControlCenterAction::ToggleMute)))
        .frame(leading_width, volume_height),
    );
    let current_output = snapshot
        .audio
        .outputs
        .iter()
        .find(|output| Some(&output.id) == snapshot.audio.current_output_id.as_ref());
    let output_name = current_output.map_or("Audio outputs", |output| output.name.as_str());
    let title = if muted {
        "Muted"
    } else if snapshot.audio.volume_percent.is_none() {
        "Audio unavailable"
    } else {
        "Volume"
    };
    let mut header = vec![boxed(
        Text::new(title)
            .text_style(TextStyle::Caption)
            .line_limit(Some(1)),
    )];
    if !snapshot.audio.outputs.is_empty() {
        let output_width = (track_width - 56.0).clamp(1.0, 144.0);
        header.push(boxed(Spacer::new()));
        header.push(output_disclosure(output_name, output_width, session));
    }
    let header = boxed(
        HStack::new(DynamicViews::new(header))
            .spacing(2.0)
            .alignment(Alignment::Center)
            .frame_width(track_width),
    );
    let volume_row = if snapshot.audio.volume_percent.is_some() {
        let request = action.clone();
        let slider = Slider::new(volume.clone())
            .min(0.0)
            .max(100.0)
            .on_change(move |next| {
                request.set(Some(ControlCenterAction::SetVolume(
                    next.clamp(0.0, 100.0).round() as u8,
                )));
            });
        control_row(
            vec![
                volume_icon,
                boxed(level_stack(header, slider, track_width, volume_height)),
                boxed(LevelValueLabel(volume).frame_width(value_width)),
            ],
            width,
            volume_height,
            gap,
        )
    } else {
        control_row(
            vec![
                volume_icon,
                header,
                boxed(Spacer::new().frame_width(value_width)),
            ],
            width,
            volume_height,
            gap,
        )
    };
    (brightness_row, volume_row)
}

fn output_disclosure(
    label: &str,
    width: f32,
    session: &State<ControlCenterSession>,
) -> Box<dyn View> {
    let arrow_width = 16.0;
    let height = 15.0;
    let arrow = page_button(
        "",
        Icon::ChevronRight,
        ControlCenterPage::AudioOutputs(0),
        Size::new(arrow_width, height),
        session,
    );
    let session = session.clone();
    boxed(
        HStack::new(DynamicViews::new(vec![
            boxed(
                Text::new(label)
                    .font_size(11.0)
                    .color(ColorPalette::default().text_secondary())
                    .alignment(Alignment::Trailing)
                    .frame_width((width - arrow_width).max(1.0)),
            ),
            arrow,
        ]))
        .spacing(0.0)
        .alignment(Alignment::Center)
        .frame(width, height)
        .on_click(move || {
            session.update(|current| current.page = ControlCenterPage::AudioOutputs(0))
        }),
    )
}

fn unavailable_level(
    icon: Box<dyn View>,
    label: &str,
    width: f32,
    height: f32,
    gap: f32,
) -> Box<dyn View> {
    control_row(
        vec![
            icon,
            boxed(
                Text::new(label)
                    .text_style(TextStyle::Caption)
                    .color(ColorPalette::default().text_secondary()),
            ),
            boxed(Spacer::new()),
        ],
        width,
        height,
        gap,
    )
}

fn level_stack(
    header: Box<dyn View>,
    slider: Slider,
    width: f32,
    height: f32,
) -> impl View + Clone {
    VStack::new(DynamicViews::new(vec![
        boxed(
            VStack::new(DynamicViews::new(vec![header]))
                .alignment(Alignment::Leading)
                .frame(width, 15.0),
        ),
        boxed(slider.frame(width, (height - 15.0).max(1.0))),
    ]))
    .alignment(Alignment::Leading)
    .spacing(0.0)
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
        details_header("Audio outputs", metrics, session)
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
    let mut rows = Vec::new();
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
    vec![
        header,
        content_section(width, metrics.details_height as f32, rows, gap),
    ]
}

fn details_page(
    metrics: ControlCenterMetrics,
    snapshot: &ControlCenterSnapshot,
    session: &State<ControlCenterSession>,
) -> Vec<Box<dyn View>> {
    let width = metrics.content_width();
    let height = metrics.details_height as f32;
    let (title, details) = if metrics.page == ControlCenterPage::Network {
        let mut rows = vec![(
            "Connection",
            String::from(network_summary(&snapshot.network)),
        )];
        if let Some(link) = snapshot
            .network
            .interfaces
            .first()
            .filter(|_| snapshot.network.available)
        {
            rows.push(("Interface", link.name.clone()));
            if let Some(address) = &link.detail {
                rows.push(("IPv4", address.clone()));
            }
        }
        ("Network", rows)
    } else {
        let cpu = snapshot.system.cpu_percent.map_or_else(
            || String::from("Unavailable"),
            |value| alloc::format!("{value}%"),
        );
        let tasks = snapshot.system.task_count.map_or_else(
            || String::from("Unavailable"),
            |value| alloc::format!("{value}"),
        );
        (
            "System status",
            vec![
                ("CPU", cpu),
                ("Tasks", tasks),
                ("Power", snapshot.power.detail().replace('\n', " · ")),
                (
                    "Input",
                    String::from(input_label(&snapshot.input_environment)),
                ),
            ],
        )
    };
    let gap = 3.0_f32.min(height / 16.0);
    let row_height = ((height + gap) / details.len() as f32 - gap).min(22.0);
    let label_width = 78.0_f32.min(width * 0.4);
    let details = details
        .into_iter()
        .map(|(label, value)| {
            control_row(
                vec![
                    boxed(
                        Text::new(label)
                            .font_size(11.0)
                            .color(ColorPalette::default().text_secondary())
                            .alignment(Alignment::Leading)
                            .frame_width(label_width),
                    ),
                    boxed(
                        Text::new(value)
                            .font_size(12.0)
                            .alignment(Alignment::Trailing)
                            .frame_width((width - label_width - gap).max(1.0)),
                    ),
                ],
                width,
                row_height,
                gap,
            )
        })
        .collect();
    vec![
        details_header(title, metrics, session),
        content_section(width, height, details, gap),
    ]
}

fn session_page(
    metrics: ControlCenterMetrics,
    session: &State<ControlCenterSession>,
) -> Vec<Box<dyn View>> {
    let (title, options): (_, &[_]) = if metrics.page == ControlCenterPage::Power {
        (
            "Power",
            &[
                (SessionAction::Restart, Icon::Refresh),
                (SessionAction::PowerOff, Icon::Power),
            ],
        )
    } else {
        ("Session", &[(SessionAction::LogOut, Icon::Logout)])
    };
    let width = metrics.content_width();
    let gap = (metrics.gap as f32).min(width / 4.0);
    let height = (metrics.details_height as f32)
        .min(metrics.target_height as f32)
        .max(1.0);
    let button_width = ((width - gap) / 2.0).max(1.0);
    let buttons = options
        .iter()
        .map(|&(command, icon)| {
            let session = session.clone();
            boxed(
                Button::new(alloc::format!("{}…", command.label()))
                    .icon(icon)
                    .icon_size(IconSize::Small)
                    .font_size(12.0)
                    .padding(0.0)
                    .enabled(command.available())
                    .on_click(move || {
                        session
                            .update(|session| session.page = ControlCenterPage::Confirm(command));
                    })
                    .frame(button_width, height)
                    .clip(),
            )
        })
        .collect();
    vec![
        details_header(title, metrics, session),
        content_section(
            width,
            metrics.details_height as f32,
            vec![control_row(buttons, width, height, gap)],
            gap,
        ),
    ]
}

fn confirmation_page(
    metrics: ControlCenterMetrics,
    confirmed: SessionAction,
    action: &State<Option<ControlCenterAction>>,
    session: &State<ControlCenterSession>,
) -> Vec<Box<dyn View>> {
    let width = metrics.content_width();
    let gap = metrics.gap as f32;
    let height = (metrics.details_height as f32 / 2.0)
        .min(metrics.target_height as f32)
        .max(1.0);
    let previous = if confirmed == SessionAction::LogOut {
        ControlCenterPage::Session
    } else {
        ControlCenterPage::Power
    };
    let session = session.clone();
    let action = action.clone();
    let buttons = control_row(
        vec![
            boxed(Spacer::new()),
            boxed(Button::new("Cancel").on_click(move || session.update(|s| s.page = previous))),
            boxed(Button::new(confirmed.label()).on_click(move || {
                confirmed.confirm(&action);
            })),
        ],
        width,
        height,
        gap,
    );
    vec![
        boxed(
            Text::new(confirmed.question())
                .font_size(14.0)
                .frame(width, metrics.target_height as f32),
        ),
        content_section(
            width,
            metrics.details_height as f32,
            vec![
                boxed(Text::new("Unsaved work may be lost.").text_style(TextStyle::Caption)),
                buttons,
            ],
            gap,
        ),
    ]
}

fn footer_row(
    metrics: ControlCenterMetrics,
    snapshot: &ControlCenterSnapshot,
    action: State<Option<ControlCenterAction>>,
    session: &State<ControlCenterSession>,
) -> Box<dyn View> {
    let width = metrics.content_width();
    let height = metrics.target_height as f32;
    let gap = (metrics.gap as f32).min(4.0).min(width / 12.0);
    let icon_width = height.min((width - 3.0 * gap) / 4.0).max(1.0);
    let icon = |icon, pressed| {
        let button = Button::icon_only(icon)
            .header_style()
            .icon_size(IconSize::Small)
            .padding(0.0);
        if pressed {
            button.background_color(ColorPalette::default().header_button_hover())
        } else {
            button
        }
    };
    let shortcut = |icon_id, page| {
        let session = session.clone();
        boxed(
            icon(icon_id, metrics.page == page)
                .on_click(move || {
                    session.update(|session| {
                        session.page = if session.page == page {
                            ControlCenterPage::Controls
                        } else {
                            page
                        };
                    });
                })
                .frame(icon_width, height),
        )
    };
    let power = shortcut(Icon::Power, ControlCenterPage::Power);
    let session_menu = shortcut(Icon::Lock, ControlCenterPage::Session);
    let settings = boxed(
        icon(Icon::Settings, false)
            .on_click(move || action.set(Some(ControlCenterAction::OpenSettings)))
            .frame(icon_width, height),
    );
    let system_width = (width - 3.0 * icon_width - 4.0 * gap).max(1.0);
    let status = system_label(&snapshot.system);
    let input = input_label(&snapshot.input_environment);
    let summary = alloc::format!("{status} · {input}");
    let label_width = scarlet_ui::measure_text_sized(&summary, 12.0).0 as f32 + 26.0;
    let system = if label_width <= system_width {
        page_button(
            &summary,
            Icon::ChartBar,
            ControlCenterPage::System,
            Size::new(label_width, height),
            session,
        )
    } else {
        let icon_width = 22.0_f32.min(system_width / 4.0);
        let text_width = (system_width - icon_width - gap).max(1.0);
        let line = |label: String| {
            Text::new(label)
                .font_size(11.0)
                .line_limit(Some(1))
                .alignment(Alignment::Trailing)
                .frame(text_width, height / 2.0)
        };
        let icon = page_button(
            "",
            Icon::ChartBar,
            ControlCenterPage::System,
            Size::new(icon_width, height),
            session,
        );
        let session = session.clone();
        boxed(
            HStack::new(DynamicViews::new(vec![
                icon,
                boxed(VStack::new((line(status), line(input.into()))).spacing(0.0)),
            ]))
            .spacing(gap)
            .alignment(Alignment::Center)
            .frame(system_width, height)
            .on_click(move || session.update(|current| current.page = ControlCenterPage::System)),
        )
    };
    control_row(
        vec![power, session_menu, settings, boxed(Spacer::new()), system],
        width,
        height,
        gap,
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
    let content = VStack::new(DynamicViews::new(children))
        .spacing(spacing)
        .alignment(Alignment::Leading);
    boxed(ViewExt::alignment(content, Alignment::TopLeading).frame(width, height))
}

/// Keep continuous slider updates out of the row's structural reconciliation.
/// Slider already subscribes to the same value to update its own render object.
#[derive(Clone)]
struct LevelValueLabel(State<f32>);

impl View for LevelValueLabel {
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

fn network_summary(snapshot: &NetworkSnapshot) -> &'static str {
    if !snapshot.available {
        return "Unavailable";
    }
    match snapshot.interfaces.first().map(|interface| interface.state) {
        Some(NetworkInterfaceState::Connected) => "Connected",
        Some(NetworkInterfaceState::Configuring) => "Obtaining IP",
        Some(NetworkInterfaceState::Disconnected) => "Disconnected",
        Some(NetworkInterfaceState::Unknown) => "Unknown",
        None => "No connection",
    }
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
    alloc::format!("CPU {cpu} · Tasks {tasks}")
}

fn input_label(snapshot: &InputEnvironmentSnapshot) -> &'static str {
    if !snapshot.available {
        "Input unavailable"
    } else if snapshot.tablet_mode == Some(true) {
        "Tablet"
    } else {
        "Laptop"
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
    fn controls_keep_visible_status_and_stable_tracks_across_percentages() {
        use scarlet_ui::element::LayoutConstraints;
        use scarlet_ui::geometry::{Point, Rect};
        use scarlet_ui::views::TextRenderObject;

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
            let metrics = ControlCenterMetrics::resolve(
                presentation,
                Size::new(284.0, 172.0),
                ControlCenterPage::Controls,
                0,
            );
            let mut previous_tracks = None;
            for percent in [0, 9, 10, 80, 99, 100] {
                let mut snapshot = ControlCenterSnapshot::unavailable();
                snapshot.audio = AudioSnapshot::from_status(25, false);
                snapshot.brightness_percent = Some(percent);
                snapshot.network = NetworkSnapshot {
                    available: true,
                    interfaces: vec![NetworkInterfaceSnapshot {
                        name: String::from("eth0"),
                        kind: NetworkKind::Ethernet,
                        detail: None,
                        state: NetworkInterfaceState::Connected,
                    }],
                };
                snapshot.power = super::super::power::PowerStatus {
                    battery_detected: true,
                    percent: Some(78),
                    charge_state: Some(scarlet_abi::power_supply::ChargeState::Charging),
                    external_power: Some(true),
                };
                let mut element = build_control_center_view(
                    metrics,
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
                for name in ["Wired network", "Connected", "Battery", "78% · Charging"] {
                    let rect = rects.iter().find(|(label, _)| label == name).unwrap().1;
                    assert!(
                        rect.origin.y >= metrics.margin as f32
                            && rect.origin.y + rect.size.height
                                <= (metrics.margin + metrics.tile_height) as f32,
                        "{name} falls outside its tile at {rect:?}"
                    );
                }
                let percent_label = alloc::format!("{}%", percent);
                let value = rects
                    .iter()
                    .find(|(label, _)| *label == percent_label)
                    .unwrap()
                    .1;
                assert!(value.origin.x >= tracks[0].origin.x + tracks[0].size.width);
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
            2 * laptop.margin
                + laptop.tile_height
                + laptop.volume_height
                + 2 * laptop.target_height
                + 3 * laptop.gap
        );
        assert!(laptop.height <= 400);
        assert_eq!(tablet.width, 392);
        assert_eq!(
            tablet.height,
            2 * tablet.margin
                + tablet.tile_height
                + tablet.volume_height
                + 2 * tablet.target_height
                + 3 * tablet.gap
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
    fn page_and_output_changes_keep_the_panel_size() {
        for presentation in [
            ControlCenterPresentation::LaptopPopover,
            ControlCenterPresentation::TabletSheet,
        ] {
            for available in [Size::new(284.0, 172.0), Size::new(800.0, 600.0)] {
                let body = ControlCenterMetrics::resolve(
                    presentation,
                    available,
                    ControlCenterPage::Controls,
                    0,
                )
                .body_size();
                for page in [
                    ControlCenterPage::Controls,
                    ControlCenterPage::Network,
                    ControlCenterPage::System,
                    ControlCenterPage::AudioOutputs(2),
                ] {
                    for outputs in [0, 1, 6, 12] {
                        let metrics =
                            ControlCenterMetrics::resolve(presentation, available, page, outputs);
                        assert_eq!(metrics.body_size(), body);
                    }
                }
            }
        }
    }

    #[test]
    fn page_fade_only_restarts_when_the_visible_page_changes() {
        let mut fade = PageFadeRenderObject::new(ControlCenterPage::Controls);
        assert!(!fade.has_visual_animation());
        fade.show_page(ControlCenterPage::Network);
        assert!(fade.has_visual_animation());
        let initial = fade.subtree_opacity();
        assert!(fade.advance_visual_animation(Duration::from_millis(70)));
        let current = fade.subtree_opacity();
        assert!(current > initial && current < 1.0);
        assert!(matches!(
            fade.show_page(ControlCenterPage::Network),
            UpdateResult::NoChange
        ));
        assert_eq!(fade.subtree_opacity(), current);
        fade.advance_visual_animation(Duration::from_millis(70));
        assert_eq!(fade.subtree_opacity(), 1.0);
        assert!(!fade.has_visual_animation());
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
        assert_eq!(metrics.audio_rows_per_page, 2);
        assert_eq!(metrics.audio_pages, 6);
        assert_eq!(metrics.page, ControlCenterPage::AudioOutputs(5));
        assert_eq!(
            metrics.height,
            2 * (metrics.margin + metrics.target_height + metrics.gap) + metrics.details_height
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
        assert_eq!(network_summary(&unavailable), "Unavailable");
        let connected = NetworkSnapshot {
            available: true,
            interfaces: vec![NetworkInterfaceSnapshot {
                name: String::from("veth0"),
                kind: NetworkKind::Ethernet,
                state: NetworkInterfaceState::Connected,
                detail: None,
            }],
        };
        assert_eq!(network_summary(&connected), "Connected");
    }
}
