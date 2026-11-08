//! Exercise the production background catalog without the native shell services.
#[allow(dead_code)]
#[path = "../../../user/std-bin/src/scarlet_shell/app_artwork.rs"]
mod app_artwork;
#[path = "../../../user/std-bin/src/scarlet_shell/home_catalog.rs"]
mod home_catalog;

use app_artwork::{ArtworkCache, BackgroundBlur};
use home_catalog::{ApplicationEntry, HomeApplication, refresh, refreshed_selection};

#[test]
fn entries_are_available_before_image_io_and_cached_across_refreshes() {
    let directory =
        std::env::temp_dir().join(format!("scarlet-home-catalog-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("icon.png");
    let mut entries = vec![ApplicationEntry {
        app_id: "test-app".into(),
        name: "Test".into(),
        icon: "apps".into(),
        icon_path: path.to_str().unwrap().into(),
        background: String::new(),
        blur: BackgroundBlur::None,
    }];
    let mut cache = ArtworkCache::default();
    let mut previous = Vec::new();
    let mut snapshots = Vec::new();
    refresh(&entries, &mut cache, &mut previous, |applications| {
        if snapshots.is_empty() {
            assert_eq!(applications[0].app_id, "test-app");
            assert!(applications[0].artwork.icon().is_none());
            // The file only becomes available after the launchable entry is
            // published. Loading images before publishing would miss it.
            let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), 2, 2);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[80; 12])
                .unwrap();
        }
        snapshots.push(applications);
    });
    assert_eq!(snapshots.len(), 2);
    let loaded = snapshots.last().unwrap()[0].artwork.clone();
    assert!(loaded.icon().is_some());

    refresh(&entries, &mut cache, &mut previous, |_| {
        panic!("unchanged catalog was republished")
    });
    entries[0].name = "Renamed".into();
    refresh(&entries, &mut cache, &mut previous, |applications| {
        assert_eq!(applications[0].name, "Renamed");
        assert_eq!(applications[0].artwork, loaded);
    });
    refresh(&[], &mut cache, &mut previous, |applications| {
        assert!(applications.is_empty())
    });
    assert!(previous.is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn image_updates_and_catalog_reordering_keep_the_selected_app() {
    let app = |id: &str| HomeApplication {
        app_id: id.into(),
        name: id.into(),
        icon: "apps".into(),
        artwork: Default::default(),
    };
    let previous = vec![app("a"), app("b")];
    // Different artwork identities model a background decode completing.
    let updated = vec![app("a"), app("b")];
    assert_eq!(
        refreshed_selection(&previous, &updated, Some(1), false),
        Some(1)
    );
    let reordered = vec![app("b"), app("a")];
    assert_eq!(
        refreshed_selection(&previous, &reordered, Some(1), false),
        Some(0)
    );
    assert_eq!(
        refreshed_selection(&previous, &updated, Some(1), true),
        Some(0)
    );
    assert_eq!(
        refreshed_selection(&previous, &updated[..1], Some(1), false),
        Some(0)
    );
    assert_eq!(refreshed_selection(&previous, &[], Some(1), false), None);
}
