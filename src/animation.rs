//! ファイルから取り込んだアニメーション (glTF のノードの動き) の再生
//!
//! 再生中は対象ノードの配置 (`placement`) を毎フレーム書き換える。止めると最後の姿勢のまま残る。
//! 書き出し時はモデルのノード番号をシーンの番号に付け替えて、そのまま書き戻す。

use std::collections::HashMap;

use bevy::math::DMat4;
use bevy::prelude::*;

use crate::formats::chart_record::{
    AnimationRecord, ClipData, animations_from_records, animations_to_records,
};
use crate::interchange::{Animation, Track, model_io::meters_per_unit};
use crate::model::{Model, NodeId};

pub struct AnimationPlugin;

impl Plugin for AnimationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Animations>()
            // 表示の同期 (Update) より前に姿勢を決め、メッシュと稜線が同じフレームでそろうようにする
            .add_systems(PreUpdate, play_animations);
    }
}

/// 1つのアニメーション。トラックはモデルのノードを指す (位置はモデル単位)
pub struct Clip {
    pub name: String,
    pub tracks: Vec<(NodeId, Track)>,
    pub duration: f64,
}

#[derive(Resource)]
pub struct Animations {
    pub clips: Vec<Clip>,
    /// 再生・表示中のクリップ
    pub current: usize,
    pub playing: bool,
    pub looping: bool,
    /// 再生位置 [秒]
    pub time: f64,
    /// UI で再生位置を動かしたので、止まっていても姿勢を当てはめる
    pub pose_changed: bool,
}

impl Default for Animations {
    fn default() -> Self {
        Self {
            clips: Vec::new(),
            current: 0,
            playing: false,
            looping: true,
            time: 0.0,
            pose_changed: false,
        }
    }
}

impl Animations {
    /// 取り込んだアニメーションを加える。`ids[i]` はトラックの番号 i が指すノード
    pub fn add_clips(&mut self, clips: Vec<Animation>, ids: &[NodeId]) {
        for clip in clips {
            let duration = clip.duration();
            let tracks: Vec<(NodeId, Track)> = clip
                .tracks
                .into_iter()
                .filter_map(|t| Some((*ids.get(t.node)?, t)))
                .collect();
            if !tracks.is_empty() {
                self.clips.push(Clip {
                    name: clip.name,
                    tracks,
                    duration,
                });
            }
        }
    }

    /// プロジェクトファイルに書く形にする。`node_path` はノード ID を木の中の位置にする
    pub fn to_records(
        &self,
        node_path: impl Fn(NodeId) -> Option<Vec<usize>>,
    ) -> Vec<AnimationRecord> {
        animations_to_records(
            self.clips.iter().map(|clip| ClipData {
                name: &clip.name,
                tracks: clip.tracks.iter().map(|(id, t)| (*id, t)).collect(),
            }),
            node_path,
        )
    }

    /// プロジェクトファイルから作り直す。2つ目は見つからなかったノードのトラックの数
    pub fn from_records(
        records: &[AnimationRecord],
        find_node: impl Fn(&[usize]) -> Option<NodeId>,
    ) -> (Self, usize) {
        let (clips, missing) = animations_from_records(records, find_node);
        let clips = clips
            .into_iter()
            .filter(|(_, tracks)| !tracks.is_empty())
            .map(|(name, tracks)| Clip {
                duration: tracks
                    .iter()
                    .filter_map(|(_, t)| t.keys.last().map(|k| k.0))
                    .fold(0.0, f64::max),
                name,
                tracks,
            })
            .collect();
        (
            Self {
                clips,
                ..Default::default()
            },
            missing,
        )
    }

    pub fn current_clip(&self) -> Option<&Clip> {
        self.clips.get(self.current)
    }

    /// 書き出し用に、シーンのノード番号とメートルに直す。モデルから消えたノードのトラックは捨てる
    pub fn to_scene(&self, index_of: &HashMap<NodeId, usize>, unit: &str) -> Vec<Animation> {
        let to_meters = meters_per_unit(unit);
        self.clips
            .iter()
            .map(|clip| Animation {
                name: clip.name.clone(),
                tracks: clip
                    .tracks
                    .iter()
                    .filter_map(|(id, track)| {
                        let mut track = track.clone();
                        track.node = *index_of.get(id)?;
                        for key in &mut track.keys {
                            key.1.translation *= to_meters;
                        }
                        Some(track)
                    })
                    .collect(),
            })
            .filter(|a| !a.tracks.is_empty())
            .collect()
    }

    /// モデルから消えたノードのトラックと、空になったクリップを捨てる
    fn prune(&mut self, model: &Model) {
        for clip in &mut self.clips {
            clip.tracks.retain(|(id, _)| model.get(*id).is_some());
        }
        self.clips.retain(|c| !c.tracks.is_empty());
        if self.current >= self.clips.len() {
            self.current = 0;
            self.playing = false;
        }
    }
}

fn play_animations(time: Res<Time>, mut animations: ResMut<Animations>, mut model: ResMut<Model>) {
    if animations.clips.is_empty() {
        return;
    }
    // 部品を消したり「すべて閉じる」をしたりしたら、そのトラックを捨てる
    animations.prune(&model);
    if !animations.playing && !std::mem::take(&mut animations.pose_changed) {
        return;
    }
    let Some(duration) = animations.current_clip().map(|c| c.duration) else {
        return;
    };
    if animations.playing {
        animations.time += time.delta_secs_f64();
        if animations.time > duration {
            if animations.looping && duration > 0.0 {
                animations.time %= duration;
            } else {
                animations.time = duration;
                animations.playing = false;
            }
        }
    }
    let t = animations.time;
    let clip = &animations.clips[animations.current];
    for (id, track) in &clip.tracks {
        if let (Some(pose), Some(node)) = (track.sample(t), model.get_mut(*id)) {
            node.placement = DMat4::from(pose.to_affine());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interchange::{model_io::scene_to_import, test_util::sample_scene};
    use crate::model::StepSource;

    #[test]
    fn plays_imported_clip_and_exports_it_back() {
        let file = scene_to_import(&sample_scene(), "line.glb", "mm");
        let mut model = Model::default();
        let clips = file.animations;
        let root = model.add_imported(file.root, file.unit, StepSource::File("line.glb".into()));
        let ids = model.subtree(root);

        let mut app = App::new();
        app.add_plugins(AnimationPlugin)
            .insert_resource(model)
            .init_resource::<Time>();
        app.world_mut()
            .resource_mut::<Animations>()
            .add_clips(clips, &ids);
        {
            let mut anim = app.world_mut().resource_mut::<Animations>();
            anim.time = 1.0;
            anim.pose_changed = true;
        }
        app.update();

        // 1 秒で Y = 2 m = 2000 mm まで上がる
        let model = app.world().resource::<Model>();
        let y = model.get(ids[2]).unwrap().placement.w_axis.y;
        assert!((y - 2000.0).abs() < 1e-6, "{y}");

        let (_, index_of) = crate::interchange::model_io::model_to_scene(model, &[]);
        let anims = app
            .world()
            .resource::<Animations>()
            .to_scene(&index_of, &model.unit);
        let key = anims[0].tracks[0].sample(1.0).unwrap();
        assert!((key.translation.y - 2.0).abs() < 1e-9);
    }
}
