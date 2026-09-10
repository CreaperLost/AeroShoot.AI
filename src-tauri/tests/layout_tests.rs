#![cfg(unix)]
use aeroshoot_lib::{
    commands::*,
    export::SceneEvaluator,
    media::VideoFrame,
    playback::tracks_from_reader,
    project::{layout::ingest_wallpaper, EditLayout, ProjectBundle},
    render::{Compositor, LayerRole, Scene},
};
use std::fs;
use tempfile::tempdir;

fn open_layout_project() -> (tempfile::TempDir, AppState, aeroshoot_lib::project::OpenedProject) {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let mut bundle = ProjectBundle::create_new(dir.path(), "layout", "layout").unwrap();
    bundle.manifest_mut().duration_us = 1_000_000;
    bundle.manifest_mut().active_duration_us = 1_000_000;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    let path = bundle.root_path().to_path_buf();
    drop(bundle);
    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    (dir, state, opened)
}

#[test]
fn layout_persists_reopens_and_undoes() {
    let (_dir, state, opened) = open_layout_project();
    let handle = opened.project_handle.clone();
    let mut layout = opened.layout.clone();
    layout.aspect_ratio = "9:16".into();
    layout.padding_px = 32;
    layout.background_type = "solid".into();
    layout.color_start = "#ff3366".into();
    layout.webcam_mirror = false;
    layout.webcam_position = "top-left".into();
    layout.webcam_size = "lg".into();
    layout.corner_radius_px = 12;
    layout.shadow_blur_px = 10;
    layout.webcam_shape = "circle".into();
    layout.webcam_shadow = true;

    let updated =
        project_layout_update_impl(&state, handle.clone(), 0, layout.clone(), None).unwrap();
    assert_eq!(updated.revision, 1);
    assert_eq!(updated.layout.aspect_ratio, "9:16");
    assert_eq!(updated.layout.padding_px, 32);
    assert!(!updated.layout.webcam_mirror);
    assert_eq!(updated.layout.corner_radius_px, 12);
    assert_eq!(updated.layout.webcam_shape, "circle");
    assert!(updated.layout.webcam_shadow);

    let stale = project_layout_update_impl(&state, "missing".into(), 1, layout.clone(), None);
    assert!(stale.unwrap_err().contains("Stale"));
    let stale_rev = project_layout_update_impl(&state, handle.clone(), 0, layout.clone(), None);
    assert!(stale_rev.unwrap_err().contains("Stale"));

    let undone = project_undo_impl(&state, handle.clone(), 1).unwrap();
    assert_eq!(undone.layout.aspect_ratio, "16:9");
    assert_eq!(undone.layout.padding_px, 0);
    assert_eq!(undone.layout.corner_radius_px, 0);
    assert_eq!(undone.layout.webcam_shape, "rect");
    assert!(!undone.layout.webcam_shadow);
    let redone = project_redo_impl(&state, handle.clone(), 2).unwrap();
    assert_eq!(redone.layout.color_start, "#ff3366");

    let path = redone.project_path.clone().expect("path");
    close_project_impl(&state, handle).unwrap();
    let reopened = open_project_impl(&state, path).unwrap();
    assert_eq!(reopened.layout.aspect_ratio, "9:16");
    assert_eq!(reopened.layout.padding_px, 32);
    assert!(!reopened.layout.webcam_mirror);
    assert_eq!(reopened.layout.corner_radius_px, 12);
    assert_eq!(reopened.layout.shadow_blur_px, 10);
    assert_eq!(reopened.layout.webcam_shape, "circle");
    assert!(reopened.layout.webcam_shadow);
}

#[test]
fn wallpaper_is_copied_into_assets_not_an_external_url() {
    let (dir, state, opened) = open_layout_project();
    let handle = opened.project_handle.clone();
    let source = dir.path().join("bg.png");
    fs::write(&source, b"\x89PNG\r\n\x1a\n").unwrap();
    let mut layout = opened.layout.clone();
    layout.background_type = "gradient".into();
    let updated = project_layout_update_impl(
        &state,
        handle,
        0,
        layout,
        Some(source.to_string_lossy().into()),
    )
    .unwrap();
    assert_eq!(updated.layout.background_type, "wallpaper");
    let asset = updated.layout.wallpaper_asset.expect("asset");
    assert!(asset.starts_with("assets/"));
    let root = std::path::PathBuf::from(updated.project_path.unwrap());
    assert!(root.join(&asset).is_file());
    assert!(ingest_wallpaper(root.as_path(), std::path::Path::new("https://x/a.png"))
        .unwrap_err()
        .contains("URL"));
}

#[test]
fn preview_evaluators_match_and_layout_changes_pixels() {
    let (_dir, state, _opened) = open_layout_project();
    let root = {
        let reader = state.opened_project.lock();
        reader.as_ref().unwrap().root().to_path_buf()
    };
    let tracks = {
        let reader = state.opened_project.lock();
        tracks_from_reader(reader.as_ref().unwrap())
    };
    let document = {
        let reader = state.opened_project.lock();
        reader.as_ref().unwrap().history().current.clone()
    };
    let mut preview =
        SceneEvaluator::new_cpu(root.clone(), document.clone(), tracks.clone(), 64, 64).unwrap();
    let mut export =
        SceneEvaluator::new_cpu(root.clone(), document.clone(), tracks.clone(), 64, 64).unwrap();
    let a = preview.preview_at(0).unwrap();
    let b = export.preview_at(0).unwrap();
    assert_eq!(a.width, 64);
    assert_eq!(a.data, b.data);

    let mut styled = document;
    styled.layout.background_type = "solid".into();
    styled.layout.color_start = "#00ff00".into();
    styled.layout.padding_px = 8;
    let mut changed = SceneEvaluator::new_cpu(root, styled, tracks, 64, 64).unwrap();
    let c = changed.preview_at(0).unwrap();
    assert_ne!(a.data, c.data);
    assert!(c.data[1] > 200, "styled padding should show green background");
}

#[test]
fn screen_layer_aspect_and_webcam_mirror_contract() {
    let screen = VideoFrame::solid(16, 8, 0, 0, 255, 0).unwrap();
    let mut webcam = VideoFrame::solid(8, 8, 0, 255, 0, 0).unwrap();
    for y in 0..8u32 {
        for x in 4..8u32 {
            let i = (y * webcam.stride + x * 4) as usize;
            webcam.data[i] = 255;
            webcam.data[i + 1] = 255;
            webcam.data[i + 2] = 255;
        }
    }
    let mut layout = EditLayout::default();
    layout.background_type = "solid".into();
    layout.color_start = "#101010".into();
    layout.padding_px = 0;
    layout.webcam_enabled = true;
    layout.webcam_mirror = false;
    layout.webcam_position = "top-left".into();
    layout.webcam_size = "xl".into();
    let landscape = Scene::from_layout(16, 8, &layout, Some(screen.clone()), Some(webcam.clone()))
        .unwrap();
    let screen_l = landscape
        .layers
        .iter()
        .find(|l| l.role == LayerRole::Screen)
        .unwrap();
    assert_eq!(
        screen_l.width as f32 / screen_l.height as f32,
        16.0 / 8.0
    );

    layout.aspect_ratio = "9:16".into();
    let portrait = Scene::from_layout(8, 16, &layout, Some(screen), Some(webcam.clone())).unwrap();
    let screen_p = portrait
        .layers
        .iter()
        .find(|l| l.role == LayerRole::Screen)
        .unwrap();
    assert!((screen_p.width as f32 / screen_p.height as f32 - 2.0).abs() < 0.05);

    layout.webcam_mirror = true;
    layout.aspect_ratio = "1:1".into();
    let mirrored = Scene::from_layout(16, 16, &layout, None, Some(webcam)).unwrap();
    let cam = mirrored
        .layers
        .iter()
        .find(|l| l.role == LayerRole::Webcam)
        .unwrap();
    assert_eq!((cam.uv_x, cam.uv_w), (1.0, -1.0));
    let out = Compositor::composite_cpu(&mirrored).unwrap();
    let left = (cam.y * out.stride + cam.x * 4) as usize;
    assert!(out.data[left] > 200, "mirror must not leave screen pixels flipped; webcam left is white");
}

#[test]
fn wallpaper_preview_blits_ingested_asset() {
    let (dir, state, opened) = open_layout_project();
    let handle = opened.project_handle.clone();
    let source = dir.path().join("bg.png");
    let mut img = image::RgbaImage::new(8, 8);
    for p in img.pixels_mut() {
        *p = image::Rgba([255, 0, 255, 255]);
    }
    img.save(&source).unwrap();
    let mut layout = opened.layout.clone();
    layout.padding_px = 8;
    layout.webcam_enabled = false;
    let updated = project_layout_update_impl(
        &state,
        handle,
        0,
        layout,
        Some(source.to_string_lossy().into()),
    )
    .unwrap();
    assert_eq!(updated.layout.background_type, "wallpaper");
    let asset = updated.layout.wallpaper_asset.clone().expect("asset");
    assert!(asset.starts_with("assets/"));
    assert!(!asset.contains("://"));

    let root = {
        let reader = state.opened_project.lock();
        reader.as_ref().unwrap().root().to_path_buf()
    };
    let tracks = {
        let reader = state.opened_project.lock();
        tracks_from_reader(reader.as_ref().unwrap())
    };
    let document = {
        let reader = state.opened_project.lock();
        reader.as_ref().unwrap().history().current.clone()
    };
    let mut preview = SceneEvaluator::new_cpu(root, document, tracks, 32, 32).unwrap();
    let frame = preview.preview_at(0).unwrap();
    assert!(
        frame.data[0] > 200 && frame.data[2] > 200 && frame.data[1] < 40,
        "evaluator must blit the ingested magenta asset, not an external URL or the green fallback"
    );
}
