//! 動作確認用のサンプルシーン (階層とアニメーションを持つ簡単なロボット)
//!
//! ファイルに書き出すのはアプリ側の `dev::sample_files::write_samples` (手で実行するテスト)。

use glam::{DQuat, DVec3};

use super::{Animation, NodeTransform, Scene, SceneNode, Track, TriMesh};

/// 単位立方体 (中心が原点、辺 1)
pub fn cube() -> TriMesh {
    let positions = (0..8)
        .map(|i| {
            DVec3::new(
                if i & 1 == 0 { -0.5 } else { 0.5 },
                if i & 2 == 0 { -0.5 } else { 0.5 },
                if i & 4 == 0 { -0.5 } else { 0.5 },
            )
        })
        .collect();
    let triangles = vec![
        [0, 2, 1],
        [1, 2, 3],
        [4, 5, 6],
        [5, 7, 6],
        [0, 1, 4],
        [1, 5, 4],
        [2, 6, 3],
        [3, 6, 7],
        [0, 4, 2],
        [2, 4, 6],
        [1, 3, 5],
        [3, 7, 5],
    ];
    TriMesh {
        positions,
        triangles,
    }
}

/// 中心 `center`、寸法 `size` の箱のメッシュ
fn block(size: [f64; 3], center: [f64; 3]) -> TriMesh {
    let mut mesh = cube();
    for p in &mut mesh.positions {
        *p = *p * DVec3::from(size) + DVec3::from(center);
    }
    mesh
}

fn at(translation: [f64; 3]) -> NodeTransform {
    NodeTransform {
        translation: DVec3::from(translation),
        ..Default::default()
    }
}

/// 台座・旋回・アーム・ハンドの4軸ロボット (メートル)
pub fn robot() -> Scene {
    let mut nodes = Vec::new();
    let mut add = |name: &str, parent: Option<usize>, transform, mesh, color| {
        nodes.push(SceneNode {
            name: name.into(),
            parent,
            transform,
            mesh,
            color,
        });
        nodes.len() - 1
    };
    let root = add("ロボット", None, at([0.0; 3]), None, None);
    let gray = Some([0.45, 0.47, 0.5]);
    let orange = Some([0.95, 0.55, 0.15]);
    add(
        "台座",
        Some(root),
        at([0.0; 3]),
        Some(block([0.6, 0.2, 0.6], [0.0, 0.1, 0.0])),
        gray,
    );
    let turn = add(
        "旋回",
        Some(root),
        at([0.0, 0.2, 0.0]),
        Some(block([0.4, 0.3, 0.4], [0.0, 0.15, 0.0])),
        orange,
    );
    let arm = add(
        "アーム",
        Some(turn),
        at([0.0, 0.3, 0.0]),
        Some(block([0.15, 0.8, 0.15], [0.0, 0.4, 0.0])),
        orange,
    );
    let hand = add(
        "ハンド",
        Some(arm),
        at([0.0, 0.8, 0.0]),
        Some(block([0.3, 0.1, 0.2], [0.0, 0.05, 0.0])),
        gray,
    );

    let rot = |node: usize, axis: DVec3, degs: [f64; 5], base: NodeTransform| Track {
        node,
        keys: degs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let mut t = base;
                t.rotation = DQuat::from_axis_angle(axis, d.to_radians());
                (i as f64, t)
            })
            .collect(),
    };
    let base_turn = nodes[turn].transform;
    let base_arm = nodes[arm].transform;
    let base_hand = nodes[hand].transform;
    Scene {
        nodes,
        animations: vec![Animation {
            name: "ピック&プレース".into(),
            tracks: vec![
                rot(turn, DVec3::Y, [0.0, 0.0, 90.0, 90.0, 0.0], base_turn),
                rot(arm, DVec3::Z, [0.0, 45.0, 45.0, 0.0, 0.0], base_arm),
                rot(hand, DVec3::Y, [0.0, 0.0, 0.0, 90.0, 0.0], base_hand),
            ],
        }],
    }
}
