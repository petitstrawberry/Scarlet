//! Shared production Settings pages; platform services stay in the parent.
use super::*;

const SETTINGS_NOTE_FONT_SIZE: f32 = 12.0;
static NETWORK_ENTRY_SELECTED: AtomicBool = AtomicBool::new(false);

fn wrap_settings_note(value: impl Into<String>) -> String {
    let value = value.into();
    let mut wrapped = String::new();
    for paragraph in value.split('\n') {
        if !wrapped.is_empty() {
            wrapped.push('\n');
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                String::from(word)
            } else {
                format!("{} {}", line, word)
            };
            if !line.is_empty()
                && scarlet_ui::measure_text_sized(&candidate, SETTINGS_NOTE_FONT_SIZE).0 > 540
            {
                wrapped.push_str(&line);
                wrapped.push('\n');
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            for ch in word.chars() {
                let mut candidate = line.clone();
                candidate.push(ch);
                if !line.is_empty()
                    && scarlet_ui::measure_text_sized(&candidate, SETTINGS_NOTE_FONT_SIZE).0 > 540
                {
                    wrapped.push_str(&line);
                    wrapped.push('\n');
                    line.clear();
                }
                line.push(ch);
            }
        }
        wrapped.push_str(&line);
    }
    wrapped
}
fn settings_note(value: impl Into<String>) -> impl View + Clone {
    Text::new(wrap_settings_note(value))
        .font_size(SETTINGS_NOTE_FONT_SIZE)
        .color(Color::rgb(88, 94, 104))
}
fn settings_row(content: impl View + Clone, label: &str) -> impl View + Clone {
    hstack! { Text::new(label).font_size(13.0).frame_width(112.0), content, Spacer::new() }
        .spacing(12.0)
}
fn settings_section(content: impl View + Clone, title: &str, title_size: f32) -> impl View + Clone {
    vstack! { Text::new(title).font_size(title_size), content }
        .spacing(12.0)
        .alignment(Alignment::Leading)
        .padding(16.0)
        .frame_width(f32::INFINITY)
        .background(Color::rgb(247, 248, 250))
}
fn settings_page(content: impl View + Clone) -> impl View + Clone {
    ScrollView::new(content.padding(24.0))
        .vertical()
        .frame(f32::INFINITY, f32::INFINITY)
}

fn long_settings_page(content: impl View + Clone, height: f32) -> impl View + Clone {
    ScrollView::new(content.padding(24.0))
        .vertical()
        .content_size(0.0, height)
        .frame(f32::INFINITY, f32::INFINITY)
}
fn extra_note_height(value: String) -> f32 {
    let wrapped = wrap_settings_note(value);
    let height = scarlet_ui::measure_text_sized(&wrapped, SETTINGS_NOTE_FONT_SIZE).1;
    let first_line_height = scarlet_ui::measure_text_sized(
        wrapped.lines().next().unwrap_or_default(),
        SETTINGS_NOTE_FONT_SIZE,
    )
    .1;
    height.saturating_sub(first_line_height) as f32
}

pub(super) fn appearance_page(app: SettingsApp) -> impl View + Clone {
    let style_app = app.clone();
    let preset_app = app.clone();
    let red_app = app.clone();
    let green_app = app.clone();
    let blue_app = app.clone();
    let mut presets = PRESET_COLORS
        .iter()
        .map(|preset| String::from(preset.name))
        .collect::<Vec<_>>();
    presets.push(String::from("Custom"));
    let color = app.current_color();
    let height = 920.0
        + extra_note_height(app.background_image_label.get())
        + extra_note_height(app.background_status.get());
    long_settings_page(vstack! {
        settings_section(vstack! {
            settings_row(Select::new(vec![String::from("Gradient + Lines"), String::from("Gradient"), String::from("Solid")], app.background_style_index.clone())
                .width(300.0).on_change(move |index| style_app.background_style.set(match index { 1 => BackgroundStyle::Gradient, 2 => BackgroundStyle::Solid, _ => BackgroundStyle::GradientLines })), "Style"),
            settings_row(Select::new(presets, app.background_preset_index.clone()).width(300.0).on_change(move |index| {
                if let Some(preset) = PRESET_COLORS.get(index) {
                    preset_app.red_value.set(preset.color[0] as f32); preset_app.green_value.set(preset.color[1] as f32); preset_app.blue_value.set(preset.color[2] as f32);
                }
            }), "Color preset"),
            Rectangle::new().fill(Color::rgb(color[0], color[1], color[2])).corner_radius(6.0).frame(f32::INFINITY, 64.0),
            settings_row(hstack! { Slider::new(app.red_value.clone()).min(0.0).max(255.0).on_change(move |_| red_app.sync_background_preset()).frame(270.0, 24.0), Text::new(format!("{}", color[0])).font_size(12.0).frame_width(36.0) }.spacing(12.0), "Red"),
            settings_row(hstack! { Slider::new(app.green_value.clone()).min(0.0).max(255.0).on_change(move |_| green_app.sync_background_preset()).frame(270.0, 24.0), Text::new(format!("{}", color[1])).font_size(12.0).frame_width(36.0) }.spacing(12.0), "Green"),
            settings_row(hstack! { Slider::new(app.blue_value.clone()).min(0.0).max(255.0).on_change(move |_| blue_app.sync_background_preset()).frame(270.0, 24.0), Text::new(format!("{}", color[2])).font_size(12.0).frame_width(36.0) }.spacing(12.0), "Blue"),
        }.spacing(12.0).alignment(Alignment::Leading), "Desktop background", 16.0),
        settings_section(vstack! {
            settings_note(app.background_image_label.get()),
            hstack! {
                Button::new("Choose image…").on_click({ let app = app.clone(); move || app.open_background_picker() }),
                Button::new("Clear image").enabled(app.background_image.get().is_some()).on_click({ let app = app.clone(); move || app.clear_background_image() }),
            }.spacing(8.0),
            settings_note("An image replaces the generated background. Clear it to use your color and pattern."),
        }.spacing(10.0).alignment(Alignment::Leading), "Wallpaper", 14.0),
        settings_note(app.background_status.get()),
        cursor_section(app.clone()),
        Button::new("Save again").on_click(move || app.save_config()),
    }.spacing(18.0).alignment(Alignment::Leading), height)
}
pub(super) fn about_page() -> impl View + Clone {
    let (distribution, version, kernel, target) = system_description();
    settings_page(
        vstack! {
            Text::new(distribution).font_size(16.0),
            settings_note("System information reported by the operating system"),
            settings_section(vstack! {
                settings_row(settings_note(version), "OS version"),
                settings_row(settings_note(kernel), "Kernel"),
                settings_row(settings_note(target), "Build target"),
            }.spacing(16.0).alignment(Alignment::Leading), "Software", 14.0),
        }
        .spacing(18.0)
        .alignment(Alignment::Leading),
    )
}
pub(super) fn display_page(app: SettingsApp) -> impl View + Clone {
    let brightness_app = app.clone();
    let scale_app = app.clone();
    settings_page(vstack! {
        settings_section(vstack! {
            settings_row(hstack! {
                Slider::new(app.display_brightness_percent.clone()).min(0.0).max(100.0).on_changed(move |value| brightness_app.set_display_brightness_percent(f32_to_percent(value))).frame(270.0, 24.0),
                Text::new(format!("{}%", f32_to_percent(app.display_brightness_percent.get()))).font_size(12.0).frame_width(48.0),
            }.spacing(12.0), "Brightness"),
            settings_note(app.display_brightness_status.get()),
        }.spacing(12.0).alignment(Alignment::Leading), "Screen brightness", 14.0),
        settings_section(vstack! {
            settings_row(Select::new(vec![String::from("100%"),String::from("200%")],app.output_scale_index.clone()).width(240.0).placeholder("Custom saved scale").on_change(move |index| scale_app.set_output_scale(index)), "Scale"),
            settings_note(app.output_scale_status.get()),
        }.spacing(12.0).alignment(Alignment::Leading), "Text and interface size", 14.0),
    }.spacing(18.0).alignment(Alignment::Leading))
}
fn cursor_section(app: SettingsApp) -> impl View + Clone {
    let themes = app.cursor_themes.get();
    let select_app = app.clone();
    let refresh_app = app.clone();
    let height = 180.0 + extra_note_height(app.cursor_theme_status.get());
    settings_section(vstack! {
        settings_row(hstack! {
            Select::new(themes.iter().map(|theme|theme.name.clone()).collect(),app.cursor_theme_index.clone()).width(310.0).placeholder("No cursor themes installed").on_change(move |index|select_app.select_cursor_theme(index)),
            Button::new("Refresh").on_click(move ||refresh_app.refresh_cursor_themes()),
        }.spacing(8.0), "Theme"),
        settings_note(app.cursor_theme_status.get()),
        settings_note("Cursor themes apply immediately. Installed themes are read from /share/cursors."),
    }.spacing(12.0).alignment(Alignment::Leading), "Mouse pointer", 14.0).frame_height(height)
}
pub(super) fn audio_page(app: SettingsApp) -> impl View + Clone {
    let outputs = app.audio_outputs.get();
    let available = !outputs.is_empty();
    let select_app = app.clone();
    let refresh_app = app.clone();
    let volume_app = app.clone();
    let mute_app = app.clone();
    settings_page(vstack! {
        settings_section(vstack! {
            settings_row(hstack! {
                Select::new(audio_output_labels(&outputs),app.audio_output_index.clone()).width(310.0).placeholder("No audio output available").on_change(move |index|select_app.select_audio_output(index)),
                Button::new("Refresh").on_click(move ||refresh_app.refresh_audio()),
            }.spacing(8.0), "Device"),
            settings_note(app.audio_status.get()),
        }.spacing(12.0).alignment(Alignment::Leading), "Sound output", 14.0),
        settings_section(vstack! {
            settings_row(hstack! {
                Slider::new(app.audio_volume_percent.clone()).enabled(available).min(0.0).max(100.0).on_changed(move |value|volume_app.set_audio_volume_percent(f32_to_percent(value))).frame(270.0,24.0),
                Text::new(format!("{}%",f32_to_percent(app.audio_volume_percent.get()))).font_size(12.0).frame_width(48.0),
            }.spacing(12.0), "Volume"),
            settings_row(Toggle::new(app.audio_muted.clone()).enabled(available).on_change(move |muted|mute_app.set_audio_muted(muted)), "Mute"),
        }.spacing(18.0).alignment(Alignment::Leading), "Master volume", 14.0),
    }.spacing(18.0).alignment(Alignment::Leading))
}
pub(super) fn input_page(app: SettingsApp) -> impl View + Clone {
    let methods = app.input_methods.get();
    let select_app = app.clone();
    let selected = methods
        .get(app.selected_ime_index.get())
        .map(|m| m.name.clone())
        .unwrap_or_else(|| String::from("None"));
    let height = 150.0 + extra_note_height(app.input_status.get());
    long_settings_page(settings_section(vstack! {
        settings_row(Select::new(input_method_labels(&methods),app.selected_ime_index.clone()).width(330.0).placeholder("No input methods registered").on_change(move |index|select_app.select_input_method(index)), "Input method"),
        settings_note(format!("Active: {} · {} registered methods",selected,methods.len())),
        settings_note(app.input_status.get()),
    }.spacing(12.0).alignment(Alignment::Leading), "Typing", 14.0).frame_height(height), height + 48.0)
}
fn status_item_label(item: StatusItemId) -> &'static str {
    match item {
        StatusItemId::Cpu => "CPU",
        StatusItemId::Audio => "Audio",
    }
}
fn status_item_row(
    app: SettingsApp,
    item: StatusItemId,
    index: usize,
    count: usize,
) -> impl View + Clone {
    let visible = match item {
        StatusItemId::Cpu => app.cpu_visible.clone(),
        StatusItemId::Audio => app.audio_visible.clone(),
    };
    let visibility = app.clone();
    let up = app.clone();
    let down = app;
    settings_row(hstack! {
        Toggle::new(visible).on_change(move |visible|visibility.set_status_item_visible(item,visible)), Spacer::new().frame_width(24.0),
        Button::new("Move up").enabled(index>0).on_click(move || {if index>0{up.move_status_item(item,-1);}}),
        Button::new("Move down").enabled(index+1<count).on_click(move || {if index+1<count{down.move_status_item(item,1);}}),
    }.spacing(8.0),status_item_label(item))
}
pub(super) fn status_page(app: SettingsApp) -> impl View + Clone {
    let preferences = app.status_preferences.get();
    let count = preferences.order.len();
    let first = preferences
        .order
        .first()
        .copied()
        .unwrap_or(StatusItemId::Cpu);
    let second = preferences
        .order
        .get(1)
        .copied()
        .unwrap_or(StatusItemId::Audio);
    let refresh = app.clone();
    let reset = app.clone();
    let clock = app.clone();
    let region = app.clone();
    let timezone = app.clone();
    let height = 780.0
        + extra_note_height(app.timezone_status.get())
        + extra_note_height(app.status_preferences_status.get());
    long_settings_page(vstack! {
        settings_section(vstack! {
            settings_note("Show items in the status bar and choose their order."),
            status_item_row(app.clone(),first,0,count),status_item_row(app.clone(),second,1,count),
        }.spacing(12.0).alignment(Alignment::Leading),"Status bar", 14.0),
        settings_section(vstack! {
            settings_row(Select::new(app.timezone_regions.get(),app.timezone_region_index.clone()).width(300.0).placeholder("No time zones installed").on_change(move |index|{
                if let Some(name)=region.timezone_regions.get().get(index){region.timezone_cities.set(enumerate_cities(name));region.timezone_city_index.set(usize::MAX);}
            }),"Region"),
            settings_row(Select::new(app.timezone_cities.get(),app.timezone_city_index.clone()).width(300.0).placeholder("Choose a city"),"City"),
            settings_note(format!("Active time zone: {}",app.timezone_active.get())),
            settings_note(app.timezone_status.get()),
            Button::new("Apply time zone").enabled(app.timezone_cities.get().get(app.timezone_city_index.get()).is_some()).on_click(move ||timezone.apply_timezone()),
        }.spacing(12.0).alignment(Alignment::Leading),"Time zone", 14.0),
        settings_section(vstack! {
            settings_row(Select::new(vec![String::from("24-hour"),String::from("12-hour")],app.clock_format_index.clone()).width(300.0).on_change(move |index|clock.set_status_clock_format(if index==1 {ClockFormat::TwelveHour}else{ClockFormat::TwentyFourHour})),"Clock format"),
            settings_note("The clock stays visible at the far right of the status bar."),
        }.spacing(12.0).alignment(Alignment::Leading),"Clock", 14.0),
        settings_note(app.status_preferences_status.get()),
        hstack! { Button::new("Refresh").on_click(move||refresh.refresh_status_preferences()), Button::new("Reset to defaults").on_click(move||reset.reset_status_preferences()), }.spacing(8.0),
    }.spacing(18.0).alignment(Alignment::Leading), height)
}

pub(super) fn settings_navigation(original: SettingsApp) -> impl View + Clone {
    let network_app = original.clone();
    let app = original.clone();
    let display_app = original.clone();
    let audio_app = original.clone();
    let input_app = original.clone();
    let status_app = original.clone();
    let navigation_title = original.navigation_title.clone();
    let navigation = navigation! {
        NavigationLink::new("Appearance", move || {
            appearance_page(app.clone())
        })
        .on_select({
            let title = navigation_title.clone();
            move || title.set(String::from("Appearance"))
        }),
        NavigationLink::new("Display", move || display_page(display_app.clone())).on_select({
            let title = navigation_title.clone();
            move || title.set(String::from("Display"))
        }),
        NavigationLink::new("Audio", move || audio_page(audio_app.clone())).on_select({
            let title = navigation_title.clone();
            move || title.set(String::from("Audio"))
        }),
        NavigationLink::new("Network", move || network_page(network_app.clone())).on_select({
            let title = navigation_title.clone();
            move || title.set(String::from("Network"))
        }),
        NavigationLink::new("Input", move || input_page(input_app.clone())).on_select({
            let title = navigation_title.clone();
            move || title.set(String::from("Input"))
        }),
        NavigationLink::new("Shell & Status", move || {
            status_page(status_app.clone())
        }).on_select({
            let title = navigation_title.clone();
            let app = original.clone();
            move || {
                app.refresh_status_preferences();
                title.set(String::from("Shell & Status"));
            }
        }),
        NavigationLink::new("About", about_page).on_select({
            let title = navigation_title.clone();
            move || title.set(String::from("About This System"))
        }),
    };
    if std::env::args().any(|arg| arg == "--network")
        && !NETWORK_ENTRY_SELECTED.swap(true, AtomicOrdering::AcqRel)
    {
        navigation.selected_index_state().set(3);
        navigation_title.set(String::from("Network"));
    }
    navigation
        .header(move || {
            HeaderBar::new(
                hstack! {
                    Text::from_state(navigation_title.clone()).font_size(20.0),
                    Spacer::new(),
                }
                .alignment(Alignment::Center)
                .padding_insets(scarlet_ui::geometry::EdgeInsets::new(
                    24.0, 10.0, 24.0, 10.0,
                )),
            )
            .height(48.0)
        })
        .sidebar_width(160.0)
        .frame(f32::INFINITY, f32::INFINITY)
}

pub(super) fn network_page(app: SettingsApp) -> impl View + Clone {
    let scan = app.clone();
    let connect = app.clone();
    let disconnect = app.clone();
    let forget = app.clone();
    let select = app.clone();
    let interface_select = app.clone();
    let apply = app.clone();
    let interfaces = app.network_interfaces.get();
    let interface_labels = interfaces.iter().map(|i| i.label.clone()).collect();
    let detail = interfaces
        .get(app.network_index.get())
        .map(|i| i.detail.clone())
        .unwrap_or_else(|| String::from("No network interfaces detected"));
    let networks = app.wifi_networks.get();
    let labels = networks
        .iter()
        .map(|n| {
            format!(
                "{} · {} dBm · {} MHz",
                String::from_utf8_lossy(&n.ssid),
                n.signal / 100,
                n.frequency
            )
        })
        .collect();
    let shown = app.wifi_show_password.get();
    let mask = if shown {
        String::new()
    } else {
        "•".repeat(app.wifi_password.get().chars().count().min(40))
    };
    let height = 1200.0
        + extra_note_height(detail.clone())
        + extra_note_height(app.wifi_status.get())
        + extra_note_height(app.network_status.get())
        + extra_note_height(app.network_routes.get());
    let content=vstack! {
        settings_section(vstack! {
            vstack! {
                settings_note(app.network_routes.get()),
                settings_note("Matching local networks use their own interface. Other IPv4 destinations use the lowest route metric. Bound applications and existing connections keep their selected interface."),
            }.spacing(10.0).alignment(Alignment::Leading),
            settings_row(Select::new(interface_labels,app.network_index.clone()).width(360.0).placeholder("No interfaces").on_change(move|i|interface_select.select_network(i)),"Interface"),
            settings_note(detail),
            vstack! {
                settings_row(Select::new(vec![String::from("Automatic (DHCP)"),String::from("Manual IPv4"),String::from("Disabled")],app.network_method.clone()).width(280.0),"IPv4 mode"),
                settings_row(TextField::new(app.network_address.clone()).placeholder("192.168.1.20/24").frame(340.0,34.0),"IPv4 / prefix"),
                settings_row(TextField::new(app.network_gateway.clone()).placeholder("Optional; Manual IPv4 only").frame(340.0,34.0),"Manual gateway"),
                settings_row(TextField::new(app.network_dns.clone()).placeholder("Automatic if blank").frame(340.0,34.0),"DNS servers"),
                settings_row(TextField::new(app.network_metric.clone()).placeholder("100").frame(340.0,34.0),"Route metric"),
            }.spacing(14.0).alignment(Alignment::Leading),
            settings_note("Lower metric is preferred when routes have the same prefix. For example, Ethernet 20 and Wi-Fi 50 prefer Ethernet for external IPv4 traffic."),
            Button::new("Apply network settings").enabled(!interfaces.is_empty()).on_click(move||apply.apply_network()),
            settings_note(app.network_status.get()),
            settings_note("Settings apply to the selected Ethernet, USB Ethernet or Wi-Fi interface and are saved for the next boot. Applying changes can interrupt its connections."),
        }.spacing(14.0).alignment(Alignment::Leading),"Network interfaces",16.0),
        settings_section(vstack! {
            settings_note(app.wifi_status.get()),
            Button::new("Scan for networks").enabled(app.wifi_can_scan.get()).on_click(move||scan.wifi_action(scarlet_os::wireless_service::SCAN)),
            settings_row(Select::new(labels,app.wifi_index.clone()).width(360.0).placeholder("No networks found").on_change(move|i|{
                if let Some(network)=select.wifi_networks.get().get(i) {select.wifi_ssid.set(String::from_utf8_lossy(&network.ssid).into_owned());}
            }),"Network"),
            settings_row(TextField::new(app.wifi_ssid.clone()).placeholder("SSID").frame(340.0,34.0),"SSID"),
            settings_row(ZStack::new((
                TextField::new(app.wifi_password.clone()).placeholder("Use saved password if blank")
                    .text_color(if shown {Color::rgb(30,30,35)}else{Color::TRANSPARENT}).frame(340.0,34.0),
                Text::new(mask).font_size(13.0).padding(8.0),
            )).alignment(Alignment::Leading).frame(340.0,34.0),"Password"),
            settings_row(Toggle::new(app.wifi_show_password.clone()),"Show password"),
            hstack! {
                Button::new("Connect").on_click(move||connect.wifi_action(scarlet_os::wireless_service::CONNECT)),
                Button::new("Disconnect").on_click(move||disconnect.wifi_action(scarlet_os::wireless_service::DISCONNECT)),
                Button::new("Forget network").on_click(move||forget.wifi_action(scarlet_os::wireless_service::FORGET)),
            }.spacing(10.0),
            settings_note("WPA2-Personal networks are supported. Disconnect before scanning. A successfully connected network is saved for automatic reconnection."),
        }.spacing(14.0).alignment(Alignment::Leading),"Wi-Fi",16.0),
    }.spacing(18.0).alignment(Alignment::Leading);
    ScrollView::new(content.padding(24.0))
        .vertical()
        .content_size(0.0, height)
        .scrollbar_visibility(scarlet_ui::ScrollbarVisibility::Always)
        .frame(f32::INFINITY, f32::INFINITY)
}
