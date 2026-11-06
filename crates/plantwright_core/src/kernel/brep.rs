//! SMLib の B-rep を Rust から持つ型 [`Brep`] と、形の作成・変形・ブーリアン・
//! テッセレーション・保存と読み込み・面からの組み立て
//!
//! SMLib はプロセス全体で1つの状態を持ち、同時に複数のスレッドから呼べない。
//! そこで呼び出しはすべて [`lock`] を取ってから行う (別スレッドからも安全に使えるが、同時には動かない)。

use std::ffi::CString;
use std::ptr;
use std::sync::{Mutex, MutexGuard, OnceLock};

use glam::{DMat4, DVec3};

use super::ffi::{self, BrepPtr, MeshPtr, PwCurve, PwSurface, SUCCESS};
use super::model::{EdgeCurve, FaceMesh, FaceSpec, MeshedBrep, MeshedFace, Surface};

/// SMLib を呼ぶ前に取るロック。初めて取るときに SMLib の状態を作る
pub(super) fn lock() -> MutexGuard<'static, ()> {
    static KERNEL: OnceLock<Mutex<()>> = OnceLock::new();
    let mutex = KERNEL.get_or_init(|| {
        // SAFETY: 引数なし。OnceLock の中なので一度だけ呼ばれる
        unsafe { ffi::pw_init() };
        Mutex::new(())
    });
    // 別のスレッドがパニックしても SMLib の状態は壊れていないので使い続ける
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn check(status: i32, what: &str) -> Result<(), String> {
    if status == SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}に失敗しました (SMLib のコード {status})"))
    }
}

/// SMLib の B-rep (f64)。複製すると形の中身ごと写す
pub struct Brep {
    pub(super) ptr: BrepPtr,
}

// SAFETY: SMLib の呼び出しはすべてロックの中で行うので、どのスレッドから持っても使ってもよい
unsafe impl Send for Brep {}
unsafe impl Sync for Brep {}

impl Drop for Brep {
    fn drop(&mut self) {
        let _guard = lock();
        // SAFETY: ptr はこの値だけが持つ SmBrep*
        unsafe { ffi::pw_free(self.ptr) };
    }
}

impl Clone for Brep {
    fn clone(&self) -> Self {
        let _guard = lock();
        // SAFETY: ptr は有効な SmBrep*
        let ptr = unsafe { ffi::pw_copy(self.ptr) };
        assert!(!ptr.is_null(), "B-rep を複製できませんでした");
        Brep { ptr }
    }
}

impl std::fmt::Debug for Brep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Brep").finish_non_exhaustive()
    }
}

/// ブーリアン演算の種類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BooleanKind {
    Union,
    Difference,
    Intersection,
}

/// 外接箱 (最小と最大の角)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: DVec3,
    pub max: DVec3,
}

impl Brep {
    /// ロックの中で SmBrep* を返す関数を呼び、結果を受け取る
    pub(super) fn create(what: &str, f: impl FnOnce(*mut BrepPtr) -> i32) -> Result<Brep, String> {
        let _guard = lock();
        let mut out: BrepPtr = ptr::null_mut();
        let status = f(&mut out);
        if status != SUCCESS {
            if !out.is_null() {
                // SAFETY: 失敗しても作られていたものは自分で消す
                unsafe { ffi::pw_free(out) };
            }
            return Err(format!("{what}に失敗しました (SMLib のコード {status})"));
        }
        if out.is_null() {
            return Err(format!("{what}に失敗しました"));
        }
        Ok(Brep { ptr: out })
    }

    /// 原点を最小の角にした直方体 (寸法 size)
    pub fn cuboid(size: DVec3) -> Result<Brep, String> {
        if size.min_element() <= 0.0 {
            return Err("寸法は 0 より大きくしてください".into());
        }
        // SAFETY: out は書き込める
        Self::create("直方体の作成", |out| unsafe {
            ffi::pw_box(size.x, size.y, size.z, out)
        })
    }

    /// 底面の中心を原点に置いた、+Z 方向に立つ円柱
    pub fn cylinder_z(radius: f64, height: f64) -> Result<Brep, String> {
        if radius <= 0.0 || height <= 0.0 {
            return Err("半径と高さは 0 より大きくしてください".into());
        }
        // SAFETY: out は書き込める
        Self::create("円柱の作成", |out| unsafe {
            ffi::pw_cylinder(radius, height, out)
        })
    }

    /// 閉じた平らな多角形 `points` を `direction` (長さ込み) に押し出す
    pub fn extrude(points: &[DVec3], direction: DVec3) -> Result<Brep, String> {
        if points.len() < 3 {
            return Err("頂点を3つ以上指定してください".into());
        }
        let flat: Vec<f64> = points.iter().flat_map(|p| p.to_array()).collect();
        let dir = direction.to_array();
        // SAFETY: flat は points.len() 個の xyz、dir は 3 つの f64
        Self::create("押し出し", |out| unsafe {
            ffi::pw_extrude(flat.as_ptr(), points.len() as i32, dir.as_ptr(), out)
        })
    }

    /// 行列 `m` で動かす (回転・平行移動と、各軸方向の拡大縮小)
    pub fn transform(&mut self, m: DMat4) -> Result<(), String> {
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        let _guard = lock();
        if (scale - DVec3::ONE).abs().max_element() > 1e-12 {
            // SAFETY: ptr は有効な SmBrep*
            let status = unsafe { ffi::pw_scale(self.ptr, scale.x, scale.y, scale.z) };
            check(status, "形の拡大縮小")?;
        }
        let rigid = DMat4::from_rotation_translation(rotation, translation).to_cols_array();
        // SAFETY: rigid は 16 個の f64
        let status = unsafe { ffi::pw_transform(self.ptr, rigid.as_ptr()) };
        check(status, "形の移動")
    }

    /// 行列 `m` で動かした複製
    pub fn transformed(&self, m: DMat4) -> Result<Brep, String> {
        let mut out = self.clone();
        out.transform(m)?;
        Ok(out)
    }

    /// `self` と `other` のブーリアン。どちらも変えずに新しい形を返す
    pub fn boolean(&self, other: &Brep, kind: BooleanKind) -> Result<Brep, String> {
        let op = match kind {
            BooleanKind::Union => 0,
            BooleanKind::Difference => 1,
            BooleanKind::Intersection => 2,
        };
        // SAFETY: どちらも有効な SmBrep*。C++ 側で複製してから演算する
        Self::create("ブーリアン演算", |out| unsafe {
            ffi::pw_boolean(self.ptr, other.ptr, op, out)
        })
    }

    /// 閉じたソリッドか
    pub fn is_solid(&self) -> bool {
        let _guard = lock();
        // SAFETY: ptr は有効な SmBrep*
        unsafe { ffi::pw_is_solid(self.ptr) == 1 }
    }

    /// 許容誤差ぶん少し広い外接箱 (軽い)
    pub fn loose_bounds(&self) -> Option<Bounds> {
        let _guard = lock();
        let mut out = [0.0; 6];
        // SAFETY: out は 6 つの f64
        let status = unsafe { ffi::pw_bounding_box(self.ptr, out.as_mut_ptr()) };
        let b = Bounds {
            min: DVec3::new(out[0], out[1], out[2]),
            max: DVec3::new(out[3], out[4], out[5]),
        };
        (status == SUCCESS && b.min.cmple(b.max).all() && b.max.is_finite() && b.min.is_finite())
            .then_some(b)
    }

    /// 外接箱。SMLib の正確な外接箱は重いので、粗いテッセレーションの点から求める
    /// (曲面では弦の誤差ぶん内側になりうる)
    pub fn bounds(&self) -> Option<Bounds> {
        let loose = self.loose_bounds()?;
        let chord = (loose.max - loose.min).length() * 1e-3;
        let meshed = self.tessellate(chord, 15.0);
        let points = meshed
            .faces
            .iter()
            .filter_map(|f| f.mesh.as_ref())
            .flat_map(|m| m.positions.iter())
            .chain(meshed.edges.iter().flatten());
        let (min, max) = points.fold(
            (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        Some(if min.cmple(max).all() {
            Bounds { min, max }
        } else {
            loose
        })
    }

    pub fn face_count(&self) -> usize {
        let _guard = lock();
        // SAFETY: ptr は有効な SmBrep*
        unsafe { ffi::pw_face_count(self.ptr).max(0) as usize }
    }

    /// 面ごとのメッシュ・辺のポリライン・面の輪郭にする。
    /// `chord` は曲面からのずれの上限 (長さ)、`angle_deg` は1つの三角形で曲がってよい角度
    pub fn tessellate(&self, chord: f64, angle_deg: f64) -> MeshedBrep {
        let _guard = lock();
        // SAFETY: ptr は有効な SmBrep*。結果は read_mesh で写してから消す
        unsafe {
            let mesh = ffi::pw_tessellate(self.ptr, chord, angle_deg);
            read_mesh(mesh)
        }
    }

    /// SMLib の .smb のテキスト形式にする
    pub fn to_text(&self) -> Result<String, String> {
        let path = TempPath::new()?;
        {
            let _guard = lock();
            // SAFETY: path は NUL 終端の文字列
            let status = unsafe { ffi::pw_write_file(self.ptr, path.c.as_ptr()) };
            check(status, "形の書き出し")?;
        }
        std::fs::read_to_string(&path.path).map_err(|e| format!("形を書き出せません: {e}"))
    }

    /// [`Brep::to_text`] の文字列から読む
    pub fn from_text(text: &str) -> Result<Brep, String> {
        // SMLib の読み込みは壊れた入力で落ちることがあるので、形式の見出しだけは先に確かめる
        let mut lines = text.lines();
        let header_ok = lines.next().is_some_and(|l| l.starts_with("Version "))
            && lines
                .next()
                .is_some_and(|l| l.starts_with("Healer Version "))
            && lines.next() == Some("//Brep Starts");
        if !header_ok {
            return Err("形を読み込めません: SMLib の形式ではありません".into());
        }
        let path = TempPath::new()?;
        std::fs::write(&path.path, text).map_err(|e| format!("形を読み込めません: {e}"))?;
        // SAFETY: path は NUL 終端の文字列、out は書き込める
        Self::create("形の読み込み", |out| unsafe {
            ffi::pw_read_file(path.c.as_ptr(), out)
        })
    }

    /// 面を1つずつ足して B-rep を組み立てる (STEP などの読み込み)。
    /// 足せなかった面の数も返す。面どうしは縫い合わせ、閉じていればソリッドにする
    pub fn from_faces(faces: &[FaceSpec], tolerance: f64) -> Result<(Brep, usize), String> {
        let _guard = lock();
        // SAFETY: 新しい空の SmBrep* を作る
        let ptr = unsafe { ffi::pw_builder_new() };
        if ptr.is_null() {
            return Err("形を組み立てられません".into());
        }
        let brep = Brep { ptr };
        let mut failed = 0;
        for face in faces {
            if add_face(ptr, face, tolerance) != SUCCESS {
                failed += 1;
            }
        }
        if failed == faces.len() {
            drop(_guard);
            return Err("面を1つも作れませんでした".into());
        }
        let mut solid = 0;
        // SAFETY: ptr は有効な SmBrep*、solid は書き込める
        let status = unsafe { ffi::pw_builder_finish(ptr, &mut solid) };
        drop(_guard);
        check(status, "面の縫い合わせ")?;
        Ok((brep, failed))
    }
}

/// 平面上の多角形 (輪郭ごとの点の列、最初が外周、ほかは穴) を三角形にする。
/// 三角形の向きは外周の回る向き (右ねじ) に合う
pub fn triangulate_planar(loops: &[Vec<DVec3>]) -> Option<FaceMesh> {
    let counts: Vec<i32> = loops.iter().map(|l| l.len() as i32).collect();
    let flat: Vec<f64> = loops.iter().flatten().flat_map(|p| p.to_array()).collect();
    let _guard = lock();
    // SAFETY: flat は counts の合計の数の xyz
    let meshed = unsafe {
        let mesh = ffi::pw_planar_polygon(flat.as_ptr(), counts.as_ptr(), counts.len() as i32);
        read_mesh(mesh)
    };
    let mut out = FaceMesh::default();
    for face in meshed.faces.into_iter().filter_map(|f| f.mesh) {
        let base = out.positions.len() as u32;
        out.positions.extend(face.positions);
        out.normals.extend(face.normals);
        out.triangles
            .extend(face.triangles.iter().map(|t| t.map(|i| i + base)));
    }
    if out.triangles.is_empty() {
        return None;
    }
    // 外周の回る向きの法線にそろえる
    let outer = loops.first()?;
    let mut want = DVec3::ZERO;
    for i in 0..outer.len() {
        want += outer[i].cross(outer[(i + 1) % outer.len()]);
    }
    let mut have = DVec3::ZERO;
    for [a, b, c] in out.triangle_points() {
        have += (b - a).cross(c - a);
    }
    if have.dot(want) < 0.0 {
        out.invert();
    }
    Some(out)
}

/// 一時ファイルのパス。消すとファイルも消す
struct TempPath {
    path: std::path::PathBuf,
    c: CString,
}

impl TempPath {
    fn new() -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("plantwright-brep-{}-{n}.smb", std::process::id()));
        let c = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| "一時ファイルの名前に使えない文字があります".to_owned())?;
        Ok(Self { path, c })
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 曲線・曲面の記述が指す配列を、C に渡すあいだ持っておく
#[derive(Default)]
struct Arena {
    floats: Vec<Vec<f64>>,
    ints: Vec<Vec<i32>>,
    // 曲面の記述が母線の PwCurve を指すので、Vec が伸びても動かないように Box で持つ
    #[allow(clippy::vec_box)]
    curves: Vec<Box<PwCurve>>,
}

impl Arena {
    fn floats(&mut self, v: Vec<f64>) -> *const f64 {
        self.floats.push(v);
        self.floats.last().map_or(ptr::null(), |v| v.as_ptr())
    }

    fn ints(&mut self, v: Vec<i32>) -> *const i32 {
        self.ints.push(v);
        self.ints.last().map_or(ptr::null(), |v| v.as_ptr())
    }

    fn curve(&mut self, curve: &EdgeCurve) -> PwCurve {
        let mut c = empty_curve();
        match curve {
            EdgeCurve::Line { start, end } => {
                c.kind = 0;
                c.p0 = start.to_array();
                c.p1 = end.to_array();
            }
            EdgeCurve::Arc {
                center,
                x,
                y,
                radius,
                start_deg,
                end_deg,
            } => {
                c.kind = 1;
                c.center = center.to_array();
                c.xaxis = x.to_array();
                c.yaxis = y.to_array();
                c.r1 = *radius;
                c.a0 = *start_deg;
                c.a1 = *end_deg;
            }
            EdgeCurve::Ellipse {
                center,
                x,
                y,
                rx,
                ry,
                start_deg,
                end_deg,
            } => {
                c.kind = 2;
                c.center = center.to_array();
                c.xaxis = x.to_array();
                c.yaxis = y.to_array();
                c.r1 = *rx;
                c.r2 = *ry;
                c.a0 = *start_deg;
                c.a1 = *end_deg;
            }
            EdgeCurve::BSpline {
                curve,
                start,
                end,
                full,
                reverse,
            } => {
                c.kind = 3;
                c.p0 = start.to_array();
                c.p1 = end.to_array();
                c.degree = curve.degree as i32;
                c.ncp = curve.points.len() as i32;
                c.cp = self.floats(curve.points.iter().flat_map(|p| p.to_array()).collect());
                c.weights = match &curve.weights {
                    Some(w) => self.floats(w.clone()),
                    None => ptr::null(),
                };
                c.nknots = curve.knots.len() as i32;
                c.knots = self.floats(curve.knots.clone());
                c.mults = self.ints(curve.multiplicities.iter().map(|&m| m as i32).collect());
                c.full = i32::from(*full);
                c.reverse = i32::from(*reverse);
            }
        }
        c
    }

    fn surface(&mut self, surface: &Surface) -> PwSurface {
        let mut s = empty_surface();
        let frame = |s: &mut PwSurface, kind: i32, f: &super::model::Frame| {
            s.kind = kind;
            s.origin = f.origin.to_array();
            s.zaxis = f.z.to_array();
            s.xaxis = f.x.to_array();
        };
        match surface {
            Surface::Plane(f) => frame(&mut s, 0, f),
            Surface::Cylinder { frame: f, radius } => {
                frame(&mut s, 1, f);
                s.r1 = *radius;
            }
            Surface::Cone {
                frame: f,
                radius,
                semi_angle_deg,
            } => {
                frame(&mut s, 2, f);
                s.r1 = *radius;
                s.r2 = *semi_angle_deg;
            }
            Surface::Sphere { frame: f, radius } => {
                frame(&mut s, 3, f);
                s.r1 = *radius;
            }
            Surface::Torus {
                frame: f,
                major,
                minor,
            } => {
                frame(&mut s, 4, f);
                s.r1 = *major;
                s.r2 = *minor;
            }
            Surface::BSpline(b) => {
                s.kind = 5;
                s.udeg = b.u_degree as i32;
                s.vdeg = b.v_degree as i32;
                s.nu = b.nu as i32;
                s.nv = b.nv as i32;
                s.cp = self.floats(b.points.iter().flat_map(|p| p.to_array()).collect());
                s.weights = match &b.weights {
                    Some(w) => self.floats(w.clone()),
                    None => ptr::null(),
                };
                s.nuk = b.u_knots.len() as i32;
                s.nvk = b.v_knots.len() as i32;
                s.uknots = self.floats(b.u_knots.clone());
                s.vknots = self.floats(b.v_knots.clone());
                s.umults = self.ints(b.u_multiplicities.iter().map(|&m| m as i32).collect());
                s.vmults = self.ints(b.v_multiplicities.iter().map(|&m| m as i32).collect());
            }
            Surface::Revolution {
                profile,
                origin,
                axis,
            } => {
                s.kind = 6;
                s.origin = origin.to_array();
                s.zaxis = axis.to_array();
                let c = self.curve(profile);
                self.curves.push(Box::new(c));
                s.profile = self.curves.last().map_or(ptr::null(), |c| &**c);
            }
            Surface::Extrusion { profile, direction } => {
                s.kind = 7;
                s.dir = direction.to_array();
                let c = self.curve(profile);
                self.curves.push(Box::new(c));
                s.profile = self.curves.last().map_or(ptr::null(), |c| &**c);
            }
        }
        s
    }
}

fn empty_curve() -> PwCurve {
    PwCurve {
        kind: 0,
        p0: [0.0; 3],
        p1: [0.0; 3],
        center: [0.0; 3],
        xaxis: [0.0; 3],
        yaxis: [0.0; 3],
        r1: 0.0,
        r2: 0.0,
        a0: 0.0,
        a1: 0.0,
        degree: 0,
        ncp: 0,
        cp: ptr::null(),
        weights: ptr::null(),
        nknots: 0,
        knots: ptr::null(),
        mults: ptr::null(),
        full: 0,
        reverse: 0,
    }
}

fn empty_surface() -> PwSurface {
    PwSurface {
        kind: 0,
        origin: [0.0; 3],
        zaxis: [0.0, 0.0, 1.0],
        xaxis: [1.0, 0.0, 0.0],
        r1: 0.0,
        r2: 0.0,
        udeg: 0,
        vdeg: 0,
        nu: 0,
        nv: 0,
        cp: ptr::null(),
        weights: ptr::null(),
        nuk: 0,
        nvk: 0,
        uknots: ptr::null(),
        vknots: ptr::null(),
        umults: ptr::null(),
        vmults: ptr::null(),
        profile: ptr::null(),
        dir: [0.0; 3],
    }
}

/// 1つの面を足す (ロックを取ってから呼ぶ)
fn add_face(brep: BrepPtr, face: &FaceSpec, tolerance: f64) -> i32 {
    let mut arena = Arena::default();
    let surface = arena.surface(&face.surface);
    let counts: Vec<i32> = face.loops.iter().map(|l| l.len() as i32).collect();
    let curves: Vec<PwCurve> = face
        .loops
        .iter()
        .flatten()
        .map(|c| arena.curve(c))
        .collect();
    // SAFETY: surface・curves の指す配列は arena が持っている
    unsafe {
        ffi::pw_builder_add_face(
            brep,
            &surface,
            i32::from(face.same_sense),
            counts.as_ptr(),
            counts.len() as i32,
            curves.as_ptr(),
            curves.len() as i32,
            tolerance,
        )
    }
}

/// C++ のテッセレーションの結果を写して消す (ロックを取ってから呼ぶ)
///
/// # Safety
/// `mesh` は pw_tessellate などが返したもの (NULL でもよい)
unsafe fn read_mesh(mesh: MeshPtr) -> MeshedBrep {
    if mesh.is_null() {
        return MeshedBrep::default();
    }
    let mut out = MeshedBrep::default();
    // SAFETY: mesh は有効。返る配列は mesh を消すまで有効
    unsafe {
        let nfaces = ffi::pw_mesh_face_count(mesh);
        let nedges = ffi::pw_mesh_edge_count(mesh);
        for e in 0..nedges {
            let mut points = ptr::null();
            let mut n = 0;
            ffi::pw_mesh_edge(mesh, e, &mut points, &mut n);
            out.edges.push(read_points(points, n));
        }
        for f in 0..nfaces {
            let (mut positions, mut normals, mut triangles) =
                (ptr::null(), ptr::null(), ptr::null());
            let (mut nv, mut nt) = (0, 0);
            ffi::pw_mesh_face(
                mesh,
                f,
                &mut positions,
                &mut normals,
                &mut nv,
                &mut triangles,
                &mut nt,
            );
            let (mut counts, mut edges) = (ptr::null(), ptr::null());
            let (mut nloops, mut nloop_edges) = (0, 0);
            ffi::pw_mesh_face_loops(
                mesh,
                f,
                &mut counts,
                &mut nloops,
                &mut edges,
                &mut nloop_edges,
            );
            let counts = read_u32(counts, nloops);
            let edges = read_u32(edges, nloop_edges);
            let mut loops = Vec::new();
            let mut next = 0;
            for count in counts {
                let end = (next + count as usize).min(edges.len());
                loops.push(
                    edges[next..end]
                        .iter()
                        .filter(|&&e| (e as usize) < out.edges.len())
                        .map(|&e| e as usize)
                        .collect(),
                );
                next = end;
            }
            let tris = read_u32(triangles, nt * 3);
            let mesh = (nt > 0).then(|| {
                FaceMesh::new(
                    read_points(positions, nv),
                    read_points(normals, nv),
                    tris.as_chunks::<3>().0.to_vec(),
                )
            });
            out.faces.push(MeshedFace { mesh, loops });
        }
        ffi::pw_mesh_free(mesh);
    }
    out
}

/// # Safety
/// `p` は `n` 個の xyz を指す (n が 0 なら何でもよい)
unsafe fn read_points(p: *const f64, n: i32) -> Vec<DVec3> {
    if p.is_null() || n <= 0 {
        return Vec::new();
    }
    // SAFETY: 呼び出し側が保証する
    let s = unsafe { std::slice::from_raw_parts(p, n as usize * 3) };
    s.as_chunks::<3>()
        .0
        .iter()
        .map(|&c| DVec3::from_array(c))
        .collect()
}

/// # Safety
/// `p` は `n` 個の u32 を指す (n が 0 なら何でもよい)
unsafe fn read_u32(p: *const u32, n: i32) -> Vec<u32> {
    if p.is_null() || n <= 0 {
        return Vec::new();
    }
    // SAFETY: 呼び出し側が保証する
    unsafe { std::slice::from_raw_parts(p, n as usize) }.to_vec()
}
