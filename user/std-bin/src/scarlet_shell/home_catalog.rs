//! Publish launchable entries before preparing their artwork on the catalog worker.

use super::app_artwork::{AppArtwork, ArtworkCache, BackgroundBlur};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeApplication {
    pub app_id: String,
    pub name: String,
    pub icon: String,
    pub artwork: AppArtwork,
}

pub struct ApplicationEntry {
    pub app_id: String,
    pub name: String,
    pub icon: String,
    pub icon_path: String,
    pub background: String,
    pub blur: BackgroundBlur,
}

pub fn refresh(
    entries: &[ApplicationEntry],
    cache: &mut ArtworkCache,
    previous: &mut Vec<HomeApplication>,
    mut publish: impl FnMut(Vec<HomeApplication>),
) {
    let mut applications = entries
        .iter()
        .map(|entry| HomeApplication {
            app_id: entry.app_id.clone(),
            name: entry.name.clone(),
            icon: entry.icon.clone(),
            // Keep existing artwork while checking for updates. New entries
            // are immediately launchable using their theme icon.
            artwork: previous
                .iter()
                .find(|app| app.app_id == entry.app_id)
                .map(|app| app.artwork.clone())
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    if applications != *previous {
        publish(applications.clone());
        *previous = applications.clone();
    }

    let mut last_publish = Instant::now();
    let mut artwork_changed = false;
    for (index, entry) in entries.iter().enumerate() {
        let artwork = cache.load(
            &entry.app_id,
            &entry.icon_path,
            &entry.background,
            entry.blur,
        );
        artwork_changed |= applications[index].artwork != artwork;
        applications[index].artwork = artwork;
        // Bound scene rebuilds when several small images finish together.
        // Slow decodes still become visible before the rest of the catalog.
        if artwork_changed && last_publish.elapsed() >= Duration::from_millis(50) {
            publish(applications.clone());
            *previous = applications.clone();
            artwork_changed = false;
            last_publish = Instant::now();
        }
    }
    if artwork_changed {
        publish(applications.clone());
        *previous = applications;
    }
    cache.retain(
        &entries
            .iter()
            .map(|entry| entry.app_id.clone())
            .collect::<Vec<_>>(),
    );
}

pub fn refreshed_selection(
    previous: &[HomeApplication],
    next: &[HomeApplication],
    selected: Option<usize>,
    query_changed: bool,
) -> Option<usize> {
    if !query_changed
        && let Some(app) = selected.and_then(|index| previous.get(index))
        && let Some(index) = next.iter().position(|next| next.app_id == app.app_id)
    {
        return Some(index);
    }
    (!next.is_empty()).then_some(0)
}
