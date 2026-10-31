//! 干渉チェック用の三角形メッシュと、その検索木 (BVH)
//!
//! 形状は部品ローカル座標の三角形の集まりとして持つ。BVH (外接箱の二分木) を
//! 一度作っておけば、部品が動いても作り直さずに「近くの三角形だけ」を素早く探せる。

use std::cell::Cell;

use bevy::math::DVec3;

use super::geometry::{
    Aabb, Pose, RayHit, Triangle, closest_point_on_triangle, ray_triangle, triangle_aabb,
    triangle_centroid, triangle_normal,
};

/// 葉に入れる三角形の最大数
const LEAF_SIZE: usize = 4;

/// 閉じた (水密な) 三角形メッシュ。表 = 外向き。
pub struct TriMesh {
    triangles: Vec<Triangle>,
    nodes: Vec<Node>,
}

/// BVH のノード。`children` が None なら葉で、`triangles[start..end]` を持つ。
struct Node {
    aabb: Aabb,
    children: Option<[usize; 2]>,
    start: usize,
    end: usize,
}

impl TriMesh {
    pub fn new(mut triangles: Vec<Triangle>) -> Self {
        let mut nodes = Vec::new();
        if !triangles.is_empty() {
            let len = triangles.len();
            build_node(&mut triangles, 0, len, &mut nodes);
        }
        Self { triangles, nodes }
    }

    pub fn triangles(&self) -> &[Triangle] {
        &self.triangles
    }

    /// ローカル座標での外接箱
    pub fn aabb(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| n.aabb)
    }

    /// 外接箱 `region` と重なりうる三角形を列挙する。
    pub fn for_each_in(&self, region: &Aabb, mut f: impl FnMut(&Triangle)) {
        self.walk(|aabb| aabb.overlaps(region), |t| f(t));
    }

    /// 汎用の木の探索。`enter` が false を返した枝は降りない。
    fn walk(&self, mut enter: impl FnMut(&Aabb) -> bool, mut leaf: impl FnMut(&Triangle)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0];
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index];
            if !enter(&node.aabb) {
                continue;
            }
            match node.children {
                Some(children) => stack.extend(children),
                None => self.triangles[node.start..node.end]
                    .iter()
                    .for_each(&mut leaf),
            }
        }
    }

    /// 点 `p` に最も近いメッシュ上の点と、その三角形の法線。
    pub fn closest_point(&self, p: DVec3) -> Option<(DVec3, DVec3)> {
        let mut best: Option<(DVec3, DVec3)> = None;
        // 2つのクロージャから読み書きするので Cell に入れる
        let best_d = Cell::new(f64::INFINITY);
        let point_box = Aabb::from_points([p]);
        // 外接箱までの距離が今の最良値より遠い枝は見なくてよい
        self.walk(
            |aabb| aabb.gap(&point_box) < best_d.get(),
            |t| {
                let q = closest_point_on_triangle(p, t);
                let d = q.distance(p);
                if d < best_d.get() {
                    best_d.set(d);
                    best = Some((q, triangle_normal(t)));
                }
            },
        );
        best
    }

    /// 点 `p` がメッシュの内側にあるか (レイを飛ばして交差数の偶奇で判定)。
    ///
    /// レイが辺や頂点をかすめると数え間違えるので、そのときは向きを変えて撃ち直す。
    pub fn contains(&self, p: DVec3) -> bool {
        // 座標軸とそろわない向きをいくつか用意しておく
        const DIRECTIONS: [[f64; 3]; 4] = [
            [0.5773, 0.6123, 0.5401],
            [-0.3124, 0.8172, -0.4842],
            [0.7027, -0.2236, 0.6755],
            [-0.6201, -0.5345, 0.5743],
        ];
        let mut inside = false;
        for dir in DIRECTIONS {
            let dir = DVec3::from(dir).normalize();
            let mut crossings = 0;
            let mut ambiguous = false;
            self.for_each_ray_hit(p, dir, |hit| {
                if hit.t > 1e-12 {
                    crossings += 1;
                    ambiguous |= hit.is_near_edge(1e-9);
                }
            });
            inside = crossings % 2 == 1;
            if !ambiguous {
                break;
            }
        }
        inside
    }

    /// `origin` から `dir` 方向へ、`min_t` より先に面があるか (図面のかくれ線の判定に使う)
    pub fn ray_blocked(&self, origin: DVec3, dir: DVec3, min_t: f64) -> bool {
        // 2つのクロージャから読み書きするので Cell に入れる
        let blocked = Cell::new(false);
        self.walk(
            |aabb| !blocked.get() && ray_hits_aabb(origin, dir, aabb),
            |t| {
                if ray_triangle(origin, dir, t).is_some_and(|hit| hit.t > min_t) {
                    blocked.set(true);
                }
            },
        );
        blocked.get()
    }

    fn for_each_ray_hit(&self, origin: DVec3, dir: DVec3, mut f: impl FnMut(RayHit)) {
        self.walk(
            |aabb| ray_hits_aabb(origin, dir, aabb),
            |t| {
                if let Some(hit) = ray_triangle(origin, dir, t) {
                    f(hit);
                }
            },
        );
    }
}

/// 2つのメッシュの三角形の組を、外接箱が近いものだけ列挙する。
///
/// `b_to_a` は b のローカル座標を a のローカル座標に移す変換。
/// `enter(a_box, b_box)` が false を返した組み合わせは、その下を調べない。
/// `visit(ta, tb)` には a のローカル座標にそろえた三角形が渡る。
pub fn for_each_triangle_pair(
    a: &TriMesh,
    b: &TriMesh,
    b_to_a: &Pose,
    mut enter: impl FnMut(&Aabb, &Aabb) -> bool,
    mut visit: impl FnMut(&Triangle, &Triangle),
) {
    if a.nodes.is_empty() || b.nodes.is_empty() {
        return;
    }
    let mut stack = vec![(0, 0)];
    while let Some((ia, ib)) = stack.pop() {
        let (na, nb) = (&a.nodes[ia], &b.nodes[ib]);
        if !enter(&na.aabb, &nb.aabb.transformed(b_to_a)) {
            continue;
        }
        match (na.children, nb.children) {
            (None, None) => {
                for ta in &a.triangles[na.start..na.end] {
                    for tb in &b.triangles[nb.start..nb.end] {
                        visit(ta, &b_to_a.apply_triangle(tb));
                    }
                }
            }
            // 大きい方のノードを分割して降りる
            (Some([l, r]), None) => stack.extend([(l, ib), (r, ib)]),
            (None, Some([l, r])) => stack.extend([(ia, l), (ia, r)]),
            (Some([al, ar]), Some([bl, br])) => {
                if na.aabb.diagonal() >= nb.aabb.diagonal() {
                    stack.extend([(al, ib), (ar, ib)]);
                } else {
                    stack.extend([(ia, bl), (ia, br)]);
                }
            }
        }
    }
}

/// 三角形を重心の位置で半分ずつに分けながら木を作る。
fn build_node(
    triangles: &mut [Triangle],
    start: usize,
    end: usize,
    nodes: &mut Vec<Node>,
) -> usize {
    let slice = &mut triangles[start..end];
    let aabb = slice
        .iter()
        .map(triangle_aabb)
        .fold(Aabb::EMPTY, |acc, b| acc.union(&b));
    let index = nodes.len();
    nodes.push(Node {
        aabb,
        children: None,
        start,
        end,
    });
    if slice.len() <= LEAF_SIZE {
        return index;
    }
    // いちばん長い軸で、重心の中央値で分ける
    let extent = aabb.max - aabb.min;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let mid = slice.len() / 2;
    slice.select_nth_unstable_by(mid, |a, b| {
        triangle_centroid(a)[axis].total_cmp(&triangle_centroid(b)[axis])
    });
    let left = build_node(triangles, start, start + mid, nodes);
    let right = build_node(triangles, start + mid, end, nodes);
    nodes[index].children = Some([left, right]);
    index
}

/// スラブ法によるレイと箱の交差判定
fn ray_hits_aabb(origin: DVec3, dir: DVec3, aabb: &Aabb) -> bool {
    let inv = dir.recip();
    let t1 = (aabb.min - origin) * inv;
    let t2 = (aabb.max - origin) * inv;
    let near = t1.min(t2).max_element();
    let far = t1.max(t2).min_element();
    far >= near.max(0.0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// テスト用: 中心 `center`, 寸法 `size` の箱 (外向き)
    pub fn box_mesh(center: [f64; 3], size: [f64; 3]) -> TriMesh {
        let c = DVec3::from(center);
        let h = DVec3::from(size) * 0.5;
        let corner = |x: f64, y: f64, z: f64| c + DVec3::new(x, y, z) * h;
        let quads = [
            // -X, +X, -Y, +Y, -Z, +Z の各面 (外から見て反時計回り)
            [
                (-1., -1., -1.),
                (-1., -1., 1.),
                (-1., 1., 1.),
                (-1., 1., -1.),
            ],
            [(1., -1., -1.), (1., 1., -1.), (1., 1., 1.), (1., -1., 1.)],
            [
                (-1., -1., -1.),
                (1., -1., -1.),
                (1., -1., 1.),
                (-1., -1., 1.),
            ],
            [(-1., 1., -1.), (-1., 1., 1.), (1., 1., 1.), (1., 1., -1.)],
            [
                (-1., -1., -1.),
                (-1., 1., -1.),
                (1., 1., -1.),
                (1., -1., -1.),
            ],
            [(-1., -1., 1.), (1., -1., 1.), (1., 1., 1.), (-1., 1., 1.)],
        ];
        let mut triangles = Vec::new();
        for q in quads {
            let [a, b, c, d] = q.map(|(x, y, z)| corner(x, y, z));
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
        TriMesh::new(triangles)
    }

    #[test]
    fn contains_inside_and_outside() {
        let mesh = box_mesh([0.0; 3], [2.0, 2.0, 2.0]);
        assert!(mesh.contains(DVec3::ZERO));
        assert!(mesh.contains(DVec3::new(0.9, -0.9, 0.5)));
        assert!(!mesh.contains(DVec3::new(1.1, 0.0, 0.0)));
    }

    #[test]
    fn closest_point_on_box() {
        let mesh = box_mesh([0.0; 3], [2.0, 2.0, 2.0]);
        let (q, n) = mesh.closest_point(DVec3::new(3.0, 0.2, 0.1)).unwrap();
        assert!(q.distance(DVec3::new(1.0, 0.2, 0.1)) < 1e-12);
        assert!(n.distance(DVec3::X) < 1e-12);
    }
}
