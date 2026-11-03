use super::*;

fn observe(progress: &mut Progress, edit: impl FnOnce(&mut Observation)) -> Vec<MissionId> {
    let mut now = progress.last.clone().unwrap_or_default();
    edit(&mut now);
    progress
        .observe(&now)
        .into_iter()
        .map(|a| a.mission)
        .collect()
}

#[test]
fn first_observation_only_sets_baseline() {
    let mut progress = Progress::default();
    let start = Observation {
        selected: true,
        interfering_pairs: 3,
        ..Default::default()
    };
    assert!(progress.observe(&start).is_empty());
    assert_eq!(progress.done_count(), 0);
}

#[test]
fn missions_complete_on_change() {
    let mut progress = Progress::default();
    observe(&mut progress, |_| {});
    assert_eq!(
        observe(&mut progress, |o| o.selected = true),
        [MissionId::SelectPart]
    );
    assert_eq!(
        observe(&mut progress, |o| o.measurement_count = 1),
        [MissionId::Measure]
    );
    assert_eq!(
        observe(&mut progress, |o| o.orthographic = true),
        [MissionId::SwitchProjection]
    );
    // 2回目は数えない
    assert!(observe(&mut progress, |o| o.selected = false).is_empty());
    assert!(observe(&mut progress, |o| o.selected = true).is_empty());
    assert_eq!(progress.xp(), 50 + 80 + 50);
}

#[test]
fn loading_a_file_is_not_an_edit() {
    let mut progress = Progress::default();
    observe(&mut progress, |_| {});
    assert!(
        observe(&mut progress, |o| o.last_undo_label =
            Some("a.step を読み込み".into()))
        .is_empty()
    );
    assert_eq!(
        observe(&mut progress, |o| o.last_undo_label =
            Some("箱1 を編集".into())),
        [MissionId::EditPart]
    );
}

#[test]
fn orbit_needs_enough_rotation() {
    let mut progress = Progress::default();
    observe(&mut progress, |_| {});
    // 全体表示などでカメラが自動で動いても数えない
    assert!(observe(&mut progress, |o| o.camera_angles = [2.0, 1.0]).is_empty());
    observe(&mut progress, |o| o.dragging_view = true);
    assert!(observe(&mut progress, |o| o.camera_angles = [2.3, 1.0]).is_empty());
    assert_eq!(
        observe(&mut progress, |o| o.camera_angles = [2.3, 1.4]),
        [MissionId::OrbitView]
    );
}

#[test]
fn interference_counts_only_after_editing() {
    let mut progress = Progress::default();
    observe(&mut progress, |_| {});
    // 読み込んだ直後からある干渉は数えない
    assert!(observe(&mut progress, |o| o.interfering_pairs = 2).is_empty());
    assert_eq!(
        observe(&mut progress, |o| o.last_undo_label =
            Some("箱1 を編集".into())),
        [MissionId::EditPart]
    );
    assert_eq!(
        observe(&mut progress, |o| o.interfering_pairs = 3),
        [MissionId::FindInterference]
    );
}

#[test]
fn level_up_and_all_complete() {
    let mut progress = Progress::default();
    let mut last = None;
    for m in MISSIONS {
        last = Some(progress.complete(m.id));
    }
    let last = last.unwrap();
    assert!(last.all_complete);
    assert!(progress.next().is_none());
    let total: u32 = MISSIONS.iter().map(|m| m.xp).sum();
    assert_eq!(progress.xp(), total);
    assert_eq!(progress.level().level, 1 + total / XP_PER_LEVEL);

    let mut fresh = Progress::default();
    let first = fresh.complete(MissionId::PlayChart);
    assert_eq!(first.level_up, None);
    let second = fresh.complete(MissionId::FindInterference);
    assert_eq!(second.level_up, Some(2));
}

#[test]
fn restore_round_trip() {
    let mut progress = Progress::default();
    progress.complete(MissionId::Save);
    progress.complete(MissionId::Undo);
    let keys = progress.completed_keys();
    let restored = Progress::restore(keys.iter().copied().chain(["unknown"]), progress.xp());
    assert!(restored.is_done(MissionId::Save));
    assert!(restored.is_done(MissionId::Undo));
    assert_eq!(restored.done_count(), 2);
    assert_eq!(restored.next().unwrap().id, MissionId::SelectPart);
}

#[test]
fn rebase_ignores_what_a_loaded_project_brings() {
    let mut progress = Progress::default();
    observe(&mut progress, |_| {});
    progress.rebase();
    // 開いたプロジェクトに寸法・標準部品があり、再生も始まった
    let loaded = Observation {
        annotation_count: 3,
        library_part_count: 5,
        playing: true,
        ..Default::default()
    };
    assert!(progress.observe(&loaded).is_empty());
    assert_eq!(
        observe(&mut progress, |o| o.annotation_count = 4),
        [MissionId::Annotate]
    );
}

#[test]
fn skip_moves_on_and_can_still_be_achieved() {
    let mut progress = Progress::default();
    progress.skip(MissionId::SelectPart);
    assert_eq!(progress.next().unwrap().id, MissionId::OrbitView);
    assert_eq!(progress.settled_count(), 1);
    assert_eq!(progress.done_count(), 0);
    assert_eq!(progress.xp(), 0);
    // とばしたあとでも、やれば達成になって経験値がもらえる
    observe(&mut progress, |_| {});
    assert_eq!(
        observe(&mut progress, |o| o.selected = true),
        [MissionId::SelectPart]
    );
    assert!(!progress.is_skipped(MissionId::SelectPart));
    assert_eq!(progress.xp(), 50);

    progress.skip(MissionId::OrbitView);
    progress.retry(MissionId::OrbitView);
    assert_eq!(progress.next().unwrap().id, MissionId::OrbitView);
}

#[test]
fn skipped_missions_are_not_all_complete() {
    let mut progress = Progress::default();
    for m in MISSIONS {
        progress.skip(m.id);
    }
    assert!(progress.next().is_none());
    assert!(!progress.all_complete());
}

#[test]
fn skipped_round_trip() {
    let mut progress = Progress::default();
    progress.complete(MissionId::Save);
    progress.skip(MissionId::RunFlow);
    let restored = Progress::restore(progress.completed_keys(), progress.xp())
        .with_skipped(progress.skipped_keys().into_iter().chain(["save", "x"]));
    assert!(restored.is_skipped(MissionId::RunFlow));
    // 達成済みのものは、とばしたことにしない
    assert!(!restored.is_skipped(MissionId::Save));
    assert_eq!(restored.settled_count(), 2);
}

#[test]
fn chapter_complete_is_reported_once() {
    let mut progress = Progress::default();
    let look: Vec<MissionId> = Chapter::Look.missions().map(|m| m.id).collect();
    let (last, rest) = look.split_last().unwrap();
    for &id in rest {
        assert_eq!(progress.complete(id).chapter_complete, None);
    }
    assert_eq!(
        progress.complete(*last).chapter_complete,
        Some(Chapter::Look)
    );
    assert_eq!(
        progress.chapter_progress(Chapter::Look),
        (look.len(), look.len())
    );
    assert_eq!(progress.chapter_progress(Chapter::Touch).0, 0);
}

#[test]
fn every_chapter_has_missions_in_order() {
    // 章はミッション表の並びどおりに進む (章が行ったり来たりしない)
    let mut chapter = 0;
    for m in MISSIONS {
        let n = m.chapter.number();
        assert!(n >= chapter, "{} の章が前に戻っている", m.key);
        chapter = n;
        assert!(!m.steps.is_empty(), "{} に手順がない", m.key);
    }
    for c in Chapter::ALL {
        assert!(c.missions().count() > 0);
    }
}

#[test]
fn library_and_flow_missions() {
    let mut progress = Progress::default();
    observe(&mut progress, |o| o.library_part_count = 2);
    assert_eq!(
        observe(&mut progress, |o| o.library_part_count = 3),
        [MissionId::PlaceLibraryPart]
    );
    assert_eq!(
        observe(&mut progress, |o| o.flow_has_result = true),
        [MissionId::RunFlow]
    );
}

#[test]
fn loading_a_sample_chart_is_not_adding() {
    let mut progress = Progress::default();
    observe(&mut progress, |_| {});
    // サンプル装置を読み込むと、アクチュエータ・ステップ・センサーがまとめて増える
    assert!(
        observe(&mut progress, |o| {
            o.actuated_part_count = 6;
            o.step_count = 9;
            o.sensor_count = 2;
        })
        .is_empty()
    );
    assert_eq!(
        observe(&mut progress, |o| o.step_count = 10),
        [MissionId::AddStep]
    );
}

#[test]
fn advanced_missions_complete_on_change() {
    let mut progress = Progress::default();
    // 読み込んだ直後からある数や状態は数えない
    observe(&mut progress, |o| {
        o.actuated_part_count = 4;
        o.step_count = 6;
        o.sensor_count = 2;
    });
    type Edit = fn(&mut Observation);
    let cases: [(MissionId, Edit); 10] = [
        (MissionId::AddActuator, |o| o.actuated_part_count = 5),
        (MissionId::AddStep, |o| o.step_count = 7),
        (MissionId::AddSensor, |o| o.sensor_count = 3),
        (MissionId::EmergencyStop, |o| o.safety_tripped = true),
        (MissionId::RunPlc, |o| o.plc_running = true),
        (MissionId::RunLineLink, |o| o.line_running = true),
        (MissionId::ComposeLine, |o| o.composed_count = 1),
        (MissionId::ExportLadder, |o| o.ladder_export_count = 1),
        (MissionId::WalkAround, |o| o.walking = true),
        (MissionId::ExportVideo, |o| o.video_export_count = 1),
    ];
    for (id, edit) in cases {
        assert_eq!(observe(&mut progress, edit), [id]);
    }
    // 減っても、止めても達成にはならない
    assert!(
        observe(&mut progress, |o| {
            o.step_count = 1;
            o.walking = false;
            o.plc_running = false;
        })
        .is_empty()
    );
}

#[test]
fn missions_with_a_rail_place_have_a_tool() {
    for m in MISSIONS {
        if m.place == Place::Rail {
            assert!(m.tool.is_some(), "{} の道具がない", m.key);
        }
    }
}

#[test]
fn parts_split_chapters_basic_first() {
    let basic: Vec<Chapter> = Part::Basic.chapters().collect();
    let advanced: Vec<Chapter> = Part::Advanced.chapters().collect();
    assert_eq!(basic.len() + advanced.len(), Chapter::ALL.len());
    // 基本編の章が先に並ぶ
    let last_basic = basic.iter().map(|c| c.number()).max().unwrap();
    let first_advanced = advanced.iter().map(|c| c.number()).min().unwrap();
    assert!(last_basic < first_advanced);
    // 基本編だけを終えると、最後の1つで基本編クリアになり、応用編の最初のミッションが次になる
    let mut progress = Progress::default();
    let mut last = None;
    for m in MISSIONS.iter().filter(|m| m.chapter.part() == Part::Basic) {
        let achievement = progress.complete(m.id);
        if let Some(prev) = last.replace(achievement.part_complete) {
            assert_eq!(prev, None);
        }
    }
    assert_eq!(last, Some(Some(Part::Basic)));
    assert_eq!(progress.next().unwrap().chapter, advanced[0]);
    assert!(!progress.all_complete());
}

#[test]
fn focus_jumps_to_a_chosen_mission() {
    let mut progress = Progress::default();
    progress.focus(MissionId::RunPlc);
    assert_eq!(progress.next().unwrap().id, MissionId::RunPlc);
    assert_eq!(progress.focused(), Some(MissionId::RunPlc));
    // 達成するとおすすめ順に戻る
    let done = progress.complete(MissionId::RunPlc);
    assert_eq!(done.mission, MissionId::RunPlc);
    assert_eq!(progress.focused(), None);
    assert_eq!(progress.next().unwrap().id, MissionId::SelectPart);

    // とばしていたミッションもえらべる。えらんだものをとばすとおすすめ順に戻る
    progress.skip(MissionId::Save);
    progress.focus(MissionId::Save);
    assert!(!progress.is_skipped(MissionId::Save));
    assert_eq!(progress.next().unwrap().id, MissionId::Save);
    progress.skip(MissionId::Save);
    assert_eq!(progress.next().unwrap().id, MissionId::SelectPart);

    // 達成済みのものはえらべない
    progress.focus(MissionId::RunPlc);
    assert_eq!(progress.focused(), None);
}

#[test]
fn mission_keys_are_unique() {
    for (i, a) in MISSIONS.iter().enumerate() {
        for b in &MISSIONS[i + 1..] {
            assert_ne!(a.key, b.key);
            assert_ne!(a.id, b.id);
        }
    }
}
