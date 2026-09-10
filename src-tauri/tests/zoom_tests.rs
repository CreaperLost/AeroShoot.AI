#![cfg(unix)]
use aeroshoot_lib::{
    commands::*,
    project::{OpenedProject, ProjectBundle},
    timeline::{SourceInterval, TimelineMapper},
    zoom::{evaluate_at_edited, evaluate_at_source, eval_config_for, CameraTransform, ZoomSource},
};
use std::fs;
use tempfile::tempdir;

fn write_click_telemetry(root: &std::path::Path) {
    let telemetry = root.join("telemetry");
    fs::write(
        telemetry.join("geometry.jsonl"),
        r#"{"version":2,"geometry_id":"g1","t_us":0,"coordinate_space":"quartz_global","source_id":"display:1","bounds":{"x":0,"y":0,"width":1920,"height":1080},"output_width":1920,"output_height":1080,"sampling_interval_us":100000,"cursor_mode":"baked","physical_width":1920,"physical_height":1080}
"#,
    )
    .unwrap();
    fs::write(
        telemetry.join("events.jsonl"),
        r#"{"version":2,"seq":0,"t_us":1000000,"geometry_id":"g1","norm_x":0.4,"norm_y":0.4,"inside_source":true,"payload":{"kind":"move"}}
{"version":2,"seq":1,"t_us":1100000,"geometry_id":"g1","norm_x":0.41,"norm_y":0.39,"inside_source":true,"payload":{"kind":"button_down","button":0}}
{"version":2,"seq":2,"t_us":1150000,"geometry_id":"g1","norm_x":0.41,"norm_y":0.39,"inside_source":true,"payload":{"kind":"button_up","button":0}}
{"version":2,"seq":3,"t_us":2000000,"payload":{"kind":"gap","reason":"input_monitoring_revoked","start_us":1800000,"end_us":2000000,"dropped_events":0}}
"#,
    )
    .unwrap();
}

fn open_zoom_project() -> (tempfile::TempDir, AppState, OpenedProject) {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let mut bundle = ProjectBundle::create_new(dir.path(), "zoom", "zoom").unwrap();
    bundle.manifest_mut().duration_us = 10_000_000;
    bundle.manifest_mut().active_duration_us = 10_000_000;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    write_click_telemetry(bundle.root_path());
    let path = bundle.root_path().to_path_buf();
    drop(bundle);
    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    (dir, state, opened)
}

#[test]
fn zoom_command_reads_v2_disk_records_and_serializes_camel_case() {
    let (_dir, state, opened) = open_zoom_project();
    let generation =
        project_zoom_suggestions_impl(&state, opened.project_handle.clone(), None).unwrap();
    let json = serde_json::to_value(&generation).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["config"]["generationVersion"], 1);
    assert_eq!(json["config"]["primaryButtons"], serde_json::json!([0]));
    assert_eq!(generation.suggestions.len(), 1);
    assert_eq!(
        generation.suggestions[0].origin,
        aeroshoot_lib::zoom::ZoomOrigin::Click
    );
    assert_eq!(
        generation.suggestions[0].edited_ranges[0].start_us,
        generation.suggestions[0].source_start_us
    );
    assert!(!generation.suggestions[0]
        .contributing_event_seqs
        .contains(&3));

    let denied = project_zoom_suggestions_impl(&state, "stale".into(), None).unwrap_err();
    assert!(denied.contains("Stale"));
}

#[test]
fn denied_telemetry_does_not_fabricate_zoom_suggestions() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let mut bundle = ProjectBundle::create_new(dir.path(), "denied", "denied").unwrap();
    bundle.manifest_mut().duration_us = 5_000_000;
    bundle.manifest_mut().active_duration_us = 5_000_000;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    let telemetry = bundle.root_path().join("telemetry");
    fs::write(
        telemetry.join("geometry.jsonl"),
        r#"{"version":2,"geometry_id":"g1","t_us":0,"coordinate_space":"quartz_global","source_id":"display:1","bounds":{"x":0,"y":0,"width":1920,"height":1080},"output_width":1920,"output_height":1080,"sampling_interval_us":100000,"cursor_mode":"baked","physical_width":1920,"physical_height":1080}
"#,
    )
    .unwrap();
    fs::write(
        telemetry.join("events.jsonl"),
        r#"{"version":2,"seq":0,"t_us":0,"payload":{"kind":"gap","reason":"initial_button_state_unknown","start_us":0,"end_us":0,"dropped_events":0}}
{"version":2,"seq":1,"t_us":1000,"payload":{"kind":"gap","reason":"input_monitoring_unavailable","start_us":1000,"end_us":1000,"dropped_events":0}}
"#,
    )
    .unwrap();
    let path = bundle.root_path().to_path_buf();
    drop(bundle);
    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    let generation =
        project_zoom_suggestions_impl(&state, opened.project_handle, None).unwrap();
    assert!(generation.suggestions.is_empty());
}

#[test]
fn accept_dismiss_and_manual_edits_persist_without_overwriting() {
    let (_dir, state, opened) = open_zoom_project();
    let handle = opened.project_handle.clone();
    let generation = project_zoom_suggestions_impl(&state, handle.clone(), None).unwrap();
    assert_eq!(generation.suggestions.len(), 1);
    let id = generation.suggestions[0].id.clone();

    let stale = project_zoom_accept_impl(&state, "stale".into(), 0, vec![id.clone()]).unwrap_err();
    assert!(stale.contains("Stale"));

    let accepted =
        project_zoom_accept_impl(&state, handle.clone(), 0, vec![id.clone()]).unwrap();
    assert_eq!(accepted.revision, 1);
    assert_eq!(accepted.zooms.len(), 1);
    assert_eq!(accepted.zooms[0].id, id);
    assert_eq!(accepted.zooms[0].source, ZoomSource::Generated);
    let pending = project_zoom_suggestions_impl(&state, handle.clone(), None).unwrap();
    assert!(pending.suggestions.is_empty());

    let mut moved = accepted.zooms[0].clone();
    moved.source_start_us += 50_000;
    moved.source_end_us += 50_000;
    let updated = project_zoom_update_impl(&state, handle.clone(), 1, moved).unwrap();
    assert_eq!(updated.zooms[0].source, ZoomSource::Manual);
    let still_pending = project_zoom_suggestions_impl(&state, handle.clone(), None).unwrap();
    assert!(still_pending.suggestions.is_empty());
    assert!(project_zoom_accept_impl(&state, handle.clone(), 2, vec![id.clone()])
        .unwrap_err()
        .contains("already"));

    let dismissed = project_zoom_dismiss_impl(&state, handle.clone(), 2, vec![id.clone()]).unwrap();
    assert!(dismissed.zooms.is_empty());
    assert!(dismissed.dismissed_zoom_ids.contains(&id));
    assert!(project_zoom_suggestions_impl(&state, handle.clone(), None)
        .unwrap()
        .suggestions
        .is_empty());

    let undone = project_undo_impl(&state, handle.clone(), 3).unwrap();
    assert_eq!(undone.zooms.len(), 1);
    assert_eq!(undone.zooms[0].source, ZoomSource::Manual);

    let deleted = project_zoom_delete_impl(&state, handle.clone(), 4, undone.zooms[0].id.clone())
        .unwrap();
    assert!(deleted.zooms.is_empty());

    let manual = project_zoom_add_impl(
        &state,
        handle.clone(),
        5,
        ManualZoomInput {
            edited_start_us: 3_000_000,
            edited_end_us: 5_000_000,
            center_x: 0.55,
            center_y: 0.45,
            scale: 1.8,
        },
    )
    .unwrap();
    assert_eq!(manual.zooms.len(), 1);
    assert_eq!(manual.zooms[0].source, ZoomSource::Manual);
    assert!(manual.zooms[0].id.starts_with("m-"));
    let json = serde_json::to_value(&manual).unwrap();
    assert_eq!(json["zooms"][0]["source"], "manual");
}

#[test]
fn cut_through_persisted_zoom_matches_source_evaluator() {
    let (_dir, state, opened) = open_zoom_project();
    let handle = opened.project_handle.clone();
    let generation = project_zoom_suggestions_impl(&state, handle.clone(), None).unwrap();
    let accepted = project_zoom_accept_impl(
        &state,
        handle.clone(),
        0,
        vec![generation.suggestions[0].id.clone()],
    )
    .unwrap();
    let cut = project_ripple_cuts_impl(
        &state,
        handle,
        accepted.revision,
        vec![EditCut {
            start_us: 900_000,
            end_us: 1_500_000,
        }],
    )
    .unwrap();
    let mapper = TimelineMapper::try_new(
        cut.retained_intervals
            .iter()
            .enumerate()
            .map(|(i, interval)| {
                SourceInterval::new(
                    format!("ret-{i}"),
                    interval.start_us,
                    interval.end_us,
                )
            })
            .collect(),
    )
    .unwrap();
    let suggestions: Vec<_> = cut
        .zooms
        .iter()
        .map(aeroshoot_lib::zoom::ZoomKeyframe::as_suggestion)
        .collect();
    let config = eval_config_for(&cut.zooms);
    let before = evaluate_at_edited(&suggestions, &mapper, 899_999, &config).unwrap();
    let after = evaluate_at_edited(&suggestions, &mapper, 900_000, &config).unwrap();
    let source_before = evaluate_at_source(&suggestions, 899_999, &config);
    let source_after = evaluate_at_source(&suggestions, 1_500_000, &config);
    assert_eq!(before, source_before);
    assert_eq!(after, source_after);
    assert_ne!(before.scale, after.scale);
    assert_eq!(
        CameraTransform::identity().uv_rect(),
        (0.0, 0.0, 1.0, 1.0)
    );
    assert_ne!(before.uv_rect(), after.uv_rect());
}
