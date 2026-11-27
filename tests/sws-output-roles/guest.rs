//! Runs as the initial program in a disposable initramfs. The host acknowledges
//! each stage over the serial console after inspecting the physical scanouts.
use std::{
    io,
    process::Command,
    thread,
    time::{Duration, Instant},
};
use sws_client::Connection;
use sws_protocol::outputs::{OutputRole, OutputSnapshot, OutputTransaction};

#[allow(dead_code)]
#[path = "../../user/bin/src/settings_display_editor.rs"]
mod settings_display_editor;
use settings_display_editor::Editor;

fn wait_for_host(stage: &str) {
    let mut ack = String::new();
    io::stdin().read_line(&mut ack).unwrap();
    assert_eq!(ack.trim(), stage);
}

fn checkpoint(stage: &str) {
    println!("CHECK_PIXELS {stage}");
    wait_for_host(stage);
}

fn wait_outputs(conn: &Connection, count: usize) -> OutputSnapshot {
    let start = Instant::now();
    loop {
        let snapshot = conn.outputs().unwrap();
        if snapshot.outputs.len() == count {
            return snapshot;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "expected {count} outputs, got {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

// Use the same selection, draft, and Apply model as the Settings window.
fn edit_role(conn: &Connection, id: u32, role: OutputRole, revert: bool) {
    let before = conn.outputs().unwrap();
    let mut editor = Editor::new();
    editor.supports_roles = conn.get_capabilities().unwrap().supports_output_roles();
    editor.adopt(before.clone());
    editor.selected = before
        .outputs
        .iter()
        .position(|output| output.id == id)
        .unwrap();
    editor.set_role(id, role);
    assert_eq!(editor.selected_config().unwrap().role, role);
    if revert {
        editor.revert();
        assert_eq!(editor.draft, before.configuration);
        assert!(!editor.can_apply());
        assert_eq!(conn.outputs().unwrap(), before);
        return;
    }
    let pending = editor
        .begin_apply()
        .expect("Settings must allow this role change");
    assert!(pending.modes.is_empty());
    let reply = conn
        .configure_outputs(OutputTransaction {
            base_generation: pending.original.generation,
            configuration: pending.configuration.clone(),
        })
        .unwrap();
    assert_ne!(reply.generation, before.generation);
    assert_eq!(reply.configuration, pending.configuration);
    assert_eq!(reply, conn.outputs().unwrap());
    editor.finish_apply(Some(reply), String::new());
    assert_eq!(editor.selected_config().unwrap().role, role);
    assert!(!editor.dirty);
}

fn reject_stale(conn: &Connection, old: OutputSnapshot, current: &OutputSnapshot) {
    assert_ne!(old.generation, current.generation);
    let error = conn
        .configure_outputs(OutputTransaction {
            base_generation: old.generation,
            configuration: old.configuration,
        })
        .unwrap_err();
    assert_eq!(
        error,
        sws_client::Error::ServerError(sws_protocol::error_codes::STALE_OUTPUT_GENERATION)
    );
    assert_eq!(&conn.outputs().unwrap(), current);
}

fn reject_all_off(conn: &Connection) {
    let before = conn.outputs().unwrap();
    let mut layout = before.configuration.clone();
    for entry in &mut layout {
        entry.role = OutputRole::Disabled;
    }
    let error = conn
        .configure_outputs(OutputTransaction {
            base_generation: before.generation,
            configuration: layout,
        })
        .unwrap_err();
    assert_eq!(
        error,
        sws_client::Error::ServerError(sws_protocol::error_codes::INVALID_OUTPUT_CONFIGURATION)
    );
    assert_eq!(before, conn.outputs().unwrap());
}

fn paint(conn: &Connection, window: u32, rgb: [u8; 3]) {
    conn.with_surface_mut(window, |surface| surface.fill(rgb[2], rgb[1], rgb[0], 255))
        .unwrap();
    conn.commit(window).unwrap();
}

fn marker(conn: &Connection, output: u32, rgb: [u8; 3]) -> u32 {
    let context = conn.output_context(output).unwrap();
    let origin = |logical: i32| logical * context.scale_milli as i32 / 1000;
    let window = conn
        .create_surface_with_type_and_policies_at(
            "output-roles",
            "Output roles",
            "",
            120,
            80,
            sws_protocol::window_types::NORMAL,
            false,
            false,
            false,
            origin(context.rect.x) + if context.rect.x == 0 { 105 } else { 345 },
            origin(context.rect.y) + 85,
        )
        .unwrap();
    paint(conn, window, rgb);
    window
}

fn main() {
    use OutputRole::{Disabled, Extend, Mirror};
    println!("SELECT_BACKEND");
    let mut backend = String::new();
    io::stdin().read_line(&mut backend).unwrap();
    let backend = backend.trim();
    assert!(matches!(backend, "cpu" | "sgfx"));
    let mut sws = Command::new("/bin/sws")
        .env("SWS_BACKEND", backend)
        .spawn()
        .unwrap();
    let start = Instant::now();
    let conn = loop {
        if let Ok(conn) = Connection::connect_default()
            && conn.outputs().is_ok()
        {
            break conn;
        }
        assert!(
            sws.try_wait().unwrap().is_none(),
            "SWS exited during startup"
        );
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "SWS did not start"
        );
        thread::sleep(Duration::from_millis(100));
    };
    let actual = conn.get_capabilities().unwrap().compositor_backend;
    let expected = if backend == "sgfx" {
        sws_protocol::compositor_backends::SGFX
    } else {
        sws_protocol::compositor_backends::CPU
    };
    assert_eq!(actual, expected);
    let initial = wait_outputs(&conn, 2);
    let panel = initial.outputs.iter().find(|o| o.primary).unwrap().id;
    let external = initial.outputs.iter().find(|o| !o.primary).unwrap().id;
    for (id, size) in [(panel, (800, 600)), (external, (960, 540))] {
        let mode = &initial.outputs.iter().find(|o| o.id == id).unwrap().mode;
        assert_eq!((mode.pixel_width, mode.pixel_height), size);
    }
    println!("OUTPUT_FIXTURE {:?}", initial.outputs);
    // Bind Cocoa to the requested native scanout before the first marker paints.
    wait_for_host("outputs-ready");
    assert!(initial.configuration.iter().all(|c| c.role == Extend));
    edit_role(&conn, panel, Disabled, true);
    let red = [221, 43, 91];
    let green = [30, 197, 89];
    let blue = [53, 107, 213];

    let first = marker(&conn, panel, red);
    let second = marker(&conn, external, blue);
    checkpoint("extend");
    conn.destroy_surface(second).unwrap();
    edit_role(&conn, external, Mirror, false);
    reject_stale(&conn, initial.clone(), &conn.outputs().unwrap());
    checkpoint("mirror-external");
    paint(&conn, first, green);
    checkpoint("mirror-repaint");
    edit_role(&conn, external, Disabled, false);
    checkpoint("disabled-external");
    edit_role(&conn, external, Extend, false);
    assert_eq!(conn.outputs().unwrap().configuration, initial.configuration);
    let second = marker(&conn, external, blue);
    checkpoint("undo-external");
    // Keep both real application windows alive. Verify the previously local
    // window is on the surviving desktop and still accepts a fresh SHM frame.
    edit_role(&conn, panel, Disabled, false);
    let desktop = conn.output_workspace(external).unwrap();
    let active = desktop
        .workspaces
        .iter()
        .find(|w| w.id == desktop.active_workspace)
        .unwrap();
    assert!(active.window_ids.contains(&first) && active.window_ids.contains(&second));
    paint(&conn, first, red);
    checkpoint("migrated-application");
    conn.destroy_surface(first).unwrap();

    edit_role(&conn, panel, Mirror, false);
    checkpoint("mirror-primary");
    edit_role(&conn, panel, Disabled, false);
    reject_all_off(&conn);
    checkpoint("external-only");
    edit_role(&conn, panel, Extend, false);
    assert_eq!(conn.outputs().unwrap().configuration, initial.configuration);
    let first = marker(&conn, panel, green);
    checkpoint("undo-primary");
    if backend == "sgfx" {
        // QEMU's GL display excludes the VNC listener used for physical unplug.
        println!("SWS_OUTPUT_ROLES_CONNECTED_OK");
        return;
    }
    conn.destroy_surface(first).unwrap();
    conn.focus_window(second).unwrap();
    edit_role(&conn, panel, Disabled, false);
    let before_unplug = conn.outputs().unwrap();
    let previous = conn.output_workspace(external).unwrap();
    println!("BEFORE_UNPLUG {previous:?}");
    checkpoint("unplug-external");

    let single = wait_outputs(&conn, 1);
    assert_eq!(single.outputs[0].id, panel);
    assert_eq!(single.configuration[0].role, Extend);
    // A Settings Apply still in flight from the two-monitor layout must not
    // disable the sole surviving desktop after physical disconnect.
    reject_stale(&conn, before_unplug, &single);
    let recovered = conn.output_workspace(panel).unwrap();
    assert_eq!(recovered.active_workspace, previous.active_workspace);
    assert!(
        recovered
            .workspaces
            .iter()
            .any(|w| { w.id == recovered.active_workspace && w.window_ids.contains(&second) })
    );
    println!("AFTER_UNPLUG {recovered:?}");
    // The same live application surface must be reachable and repaintable.
    paint(&conn, second, red);
    checkpoint("recovered");
    // With both desktops extended, a second unplug must keep the already
    // selected workspace on the panel, even with a live external window.
    let extended = wait_outputs(&conn, 2);
    assert!(extended.configuration.iter().all(|c| c.role == Extend));
    let reattached = extended.outputs.iter().find(|o| !o.primary).unwrap().id;
    let _external_window = marker(&conn, reattached, blue);
    let selected = conn.output_workspace(panel).unwrap().active_workspace;
    checkpoint("unplug-extended");
    wait_outputs(&conn, 1);
    assert_eq!(
        conn.output_workspace(panel).unwrap().active_workspace,
        selected
    );
    checkpoint("kept-workspace");
    println!("SWS_OUTPUT_ROLES_OK");
}
